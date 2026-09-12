//! 语法树节点。
//!
//! 节点保持"贴近源码"的形状（不做提前 desugar），使 `aluka-compiler` 能
//! 自行决定 lowering 策略——装饰器、可选链都在编译期展开。

/// 表达式。
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// 数值字面量
    Number(f64),
    /// 大整数字面量
    BigInt(String),
    /// 布尔字面量
    Boolean(bool),
    /// 空值字面量
    Null,
    /// 未定义字面量
    Undefined,
    /// this 表达式引用
    This,
    /// 字符串字面量
    String(String),
    /// 标识符引用
    Ident(String),
    /// 一元运算：`op expr`
    Unary {
        /// 运算符原文（如 "-", "!", "~", "+"）
        op: String,
        /// 操作数
        expr: Box<Expr>,
    },
    /// 二元运算：`left op right`
    Binary {
        /// 运算符原文
        op: String,
        /// 左操作数
        left: Box<Expr>,
        /// 右操作数
        right: Box<Expr>,
    },
    /// 赋值表达式：`name = value`
    Assign {
        /// 目标变量名
        name: String,
        /// 赋予的值表达式
        value: Box<Expr>,
    },
    /// 更新表达式（自增自减）：`x++`, `++x`, `x--`, `--x`
    Update {
        /// 运算符："++" 或 "--"
        op: String,
        /// 目标表达式
        target: Box<Expr>,
        /// 是否为前缀运算
        prefix: bool,
    },
    /// 条件三元表达式：`cond ? then_expr : else_expr`
    Conditional {
        /// 条件表达式
        cond: Box<Expr>,
        /// 条件为真表达式
        then_expr: Box<Expr>,
        /// 条件为假表达式
        else_expr: Box<Expr>,
    },
    /// 对象字面量：`{ key: val, [computed]: val, get x() {}, set x(v) {} }`
    Object(Vec<ObjectProp>),
    /// 数组字面量：`[elem1, elem2, ...]`
    Array(Vec<Expr>),
    /// 展开表达式：`...expr`
    Spread(Box<Expr>),
    /// 属性读取：`obj.prop`
    Member {
        /// 目标对象表达式
        obj: Box<Expr>,
        /// 属性名
        prop: String,
    },
    /// 下标读取：`obj[index]`
    Index {
        /// 目标对象表达式
        obj: Box<Expr>,
        /// 下标表达式
        index: Box<Expr>,
    },
    /// 属性赋值：`obj.prop = value`
    MemberAssign {
        /// 目标对象表达式
        obj: Box<Expr>,
        /// 属性名
        prop: String,
        /// 赋予的新值
        value: Box<Expr>,
    },
    /// 下标赋值：`obj[index] = value`
    IndexAssign {
        /// 目标对象表达式
        obj: Box<Expr>,
        /// 下标表达式
        index: Box<Expr>,
        /// 赋予的新值
        value: Box<Expr>,
    },
    /// 函数调用：`callee(arg1, arg2, ...)`
    Call {
        /// 被调函数表达式
        callee: Box<Expr>,
        /// 实参表达式列表
        args: Vec<Expr>,
    },
    /// 方法调用：`receiver.method(arg1, arg2, ...)`
    MethodCall {
        /// 接收者对象表达式
        receiver: Box<Expr>,
        /// 方法名
        method: String,
        /// 实参表达式列表
        args: Vec<Expr>,
    },
    /// 构造表达式：`new callee(arg1, arg2, ...)`
    New {
        /// 被调构造函数表达式
        callee: Box<Expr>,
        /// 实参表达式列表
        args: Vec<Expr>,
    },
    /// 可选链成员访问：`obj?.prop`
    OptionalMember {
        /// 目标对象表达式
        obj: Box<Expr>,
        /// 属性名
        prop: String,
    },
    /// 可选链下标访问：`obj?.[index]`
    OptionalIndex {
        /// 目标对象表达式
        obj: Box<Expr>,
        /// 下标表达式
        index: Box<Expr>,
    },
    /// 可选链调用：`callee?.(args)`
    OptionalCall {
        /// 被调函数表达式
        callee: Box<Expr>,
        /// 实参表达式列表
        args: Vec<Expr>,
    },
    /// 函数表达式或箭头函数：`function(params) { body }` 或 `(params) => expr`
    Function(FunctionDef),
    /// 正则表达式字面量：`/pattern/flags`
    RegExp {
        /// 正则表达式模式字符串
        pattern: String,
        /// 正则修饰符字符串
        flags: String,
    },
    /// 父类引用：`super`
    Super,
    /// Yield 表达式：`yield expr` 或 `yield* expr`
    Yield {
        /// yield 参数表达式
        value: Option<Box<Expr>>,
        /// 是否为委托 yield*
        delegate: bool,
    },
    /// Await 表达式：`await expr`
    Await(Box<Expr>),
    /// 模板字符串字面量：`quasis[0] ${exprs[0]} quasis[1] ...`
    TemplateLiteral {
        /// 静态文本片段列表
        quasis: Vec<String>,
        /// 插值表达式列表
        exprs: Vec<Expr>,
    },
    /// 逗号序列表达式：`a, b, c`（for 子句等语境；求值取末位）
    Sequence {
        /// 依次求值的子表达式
        exprs: Vec<Expr>,
    },
    /// 标记模板字面量：`tag`...${expr}...``
    TaggedTemplate {
        /// 标记（被调函数）
        tag: Box<Expr>,
        /// cooked 静态片段
        quasis: Vec<String>,
        /// 原始片段（转义保留原文，`strings.raw`）
        raws: Vec<String>,
        /// 插值表达式列表
        exprs: Vec<Expr>,
    },
}

/// 变量声明种类：var 函数作用域；let/const 块级作用域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    /// 函数作用域（提升、可重复声明）
    Var,
    /// 块级作用域、可更新
    Let,
    /// 块级作用域、声明时绑定
    Const,
}

/// 带源码行号的语句（LCOV 行覆盖：解析期记录语句起始行）。
#[derive(Debug, Clone, PartialEq)]
pub struct SpannedStmt {
    /// 语句本体
    pub stmt: Stmt,
    /// 语句起始行号（1 起）
    pub line: u32,
}

impl SpannedStmt {
    /// 构造辅助。
    pub fn new(stmt: Stmt, line: u32) -> Self {
        Self { stmt, line }
    }

    /// 合成占位块（死代码消除把不可达分支替换为此形态；行号沿用外层语句，
    /// 无独立可执行语义）。
    pub fn unreachable_block() -> Self {
        Self {
            stmt: Stmt::Block(Vec::new()),
            line: 0,
        }
    }
}

/// 语句。
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// 表达式语句
    Expr(Expr),
    /// 局部变量声明：`var name = init` / `let name = init` / `const name = init`
    VarDecl {
        /// 变量名
        name: String,
        /// 初始值表达式
        init: Option<Expr>,
        /// 声明种类：var 函数作用域；let/const 块级作用域
        kind: VarKind,
    },
    /// 多声明符变量语句：`var i = 0, len = expr;`（for-init 常见）
    MultiVarDecl {
        /// 声明种类
        kind: VarKind,
        /// 声明符列表：(变量名, 初始值)
        decls: Vec<(String, Option<Expr>)>,
    },
    /// 解构声明语句：`const [a, b, ...rest] = expr;`
    DestructureDecl {
        /// 解构模式
        pattern: VarPattern,
        /// 初始化表达式
        init: Expr,
    },
    /// 代码块：`{ stmts... }`
    Block(Vec<SpannedStmt>),
    /// 条件分支语句：`if (cond) then_branch else else_branch`
    If {
        /// 条件表达式
        cond: Expr,
        /// 条件为真执行的语句
        then_branch: Box<SpannedStmt>,
        /// 条件为假执行的语句（可选）
        else_branch: Option<Box<SpannedStmt>>,
    },
    /// While 循环语句：`while (cond) body`
    While {
        /// 循环条件表达式
        cond: Expr,
        /// 循环体语句
        body: Box<SpannedStmt>,
    },
    /// Do-While 循环语句：`do body while (cond)`
    DoWhile {
        /// 循环体语句
        body: Box<SpannedStmt>,
        /// 循环条件表达式
        cond: Expr,
    },
    /// 函数显式返回语句：`return expr` 或 `return`
    Return(Option<Expr>),
    /// For 循环：`for (init; cond; update) body`
    For {
        /// 初始化
        init: Option<Box<SpannedStmt>>,
        /// 条件
        cond: Option<Expr>,
        /// 步进更新
        update: Option<Expr>,
        /// 循环体
        body: Box<SpannedStmt>,
    },
    /// For-In 循环：`for (const/let/var k in expr) body`
    ForIn {
        /// 解构模式或目标变量
        pattern: VarPattern,
        /// 遍历对象
        right: Expr,
        /// 循环体
        body: Box<SpannedStmt>,
    },
    /// For-Of 循环：`for [await] (const/let/var v of expr) body`
    ForOf {
        /// 是否是 for await 异步迭代循环
        is_await: bool,
        /// 解构模式或目标变量
        pattern: VarPattern,
        /// 遍历可迭代对象
        right: Expr,
        /// 循环体
        body: Box<SpannedStmt>,
    },
    /// Break 语句
    Break,
    /// Continue 语句
    Continue,
    /// 标签语句：`label: Statement`（标签仅作跳转目标标记，执行体为 body）
    Labeled {
        /// 标签名
        label: String,
        /// 标签标注的语句
        body: Box<SpannedStmt>,
    },
    /// Throw 抛出异常语句：`throw expr`
    Throw(Expr),
    /// 异常捕获语句：`try { body } catch (e) { catch_body } finally { finally_body }`
    Try {
        /// 保护代码块
        body: Box<SpannedStmt>,
        /// Catch 块形参名（可选，ES2019 支持省略）
        catch_param: Option<String>,
        /// Catch 处理块（可选）
        catch_body: Option<Box<SpannedStmt>>,
        /// Finally 块（可选）
        finally_body: Option<Box<SpannedStmt>>,
    },
    /// 函数声明：`function name(params) { body }`
    Function(FunctionDef),
    /// Switch 分支语句：`switch (discriminant) { cases... }`
    Switch {
        /// 判别式表达式
        discriminant: Expr,
        /// Case / Default 分支列表
        cases: Vec<SwitchCase>,
    },
    /// 类声明：`class Name extends Super { ... }`
    Class {
        /// 类名
        name: String,
        /// 父类表达式（可选）
        super_class: Option<Expr>,
        /// 构造函数定义（可选）
        constructor: Option<FunctionDef>,
        /// 类方法列表
        methods: Vec<ClassMethodDef>,
    },
    /// ESM 模块导入语句：`import ... from 'mod';`
    Import(ImportDecl),
    /// ESM 模块导出语句：`export ...`
    Export(ExportDecl),
}

/// Switch 语句中的分支：`case test: stmts...` 或 `default: stmts...`
#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCase {
    /// 匹配条件表达式（为 None 时表示 default 分支）
    pub test: Option<Expr>,
    /// 分支执行体语句列表
    pub consequent: Vec<SpannedStmt>,
}

/// 函数定义结构体
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDef {
    /// 函数名称
    pub name: String,
    /// 形参列表
    pub params: Vec<String>,
    /// 是否为变长/Rest 参数
    pub is_var_args: bool,
    /// 函数体语句
    pub body: Vec<SpannedStmt>,
    /// 是否为异步函数
    pub is_async: bool,
    /// 是否为生成器函数
    pub is_generator: bool,
    /// 是否为箭头函数（不绑定自己的 `arguments` / `this`）
    pub is_arrow: bool,
}

impl FunctionDef {
    /// 创建普通函数定义辅助方法
    pub fn new(
        name: String,
        params: Vec<String>,
        is_var_args: bool,
        body: Vec<SpannedStmt>,
    ) -> Self {
        Self {
            name,
            params,
            is_var_args,
            body,
            is_async: false,
            is_generator: false,
            is_arrow: false,
        }
    }
}

/// 类成员方法定义
#[derive(Debug, Clone, PartialEq)]
pub struct ClassMethodDef {
    /// 方法名称
    pub name: String,
    /// 形参列表
    pub params: Vec<String>,
    /// 方法体语句
    pub body: Vec<SpannedStmt>,
    /// 是否为静态方法
    pub is_static: bool,
    /// 方法类型（0=普通方法, 1=Getter, 2=Setter）
    pub kind: u32,
}

/// 对象属性键
#[derive(Debug, Clone, PartialEq)]
pub enum PropKey {
    /// 字符串字面或标识符键
    Literal(String),
    /// 运行时计算键：`[expr]`
    Computed(Expr),
}

/// 对象属性值
#[derive(Debug, Clone, PartialEq)]
pub enum PropValue {
    /// 普通表达式值
    Expr(Expr),
    /// Getter 访问器函数
    Getter(FunctionDef),
    /// Setter 访问器函数
    Setter(FunctionDef),
    /// 对象展开表达式：`...expr`
    Spread(Expr),
}

/// 对象属性条目
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectProp {
    /// 属性键
    pub key: PropKey,
    /// 属性值
    pub value: PropValue,
}

/// 数组解构项
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayPatternElem {
    /// 目标变量名
    pub name: String,
    /// 是否为 ...rest 收集
    pub is_rest: bool,
    /// 缺省默认值表达式（可选）
    pub default_value: Option<Expr>,
}

/// 对象解构属性项
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectPatternProp {
    /// 属性键名
    pub key: String,
    /// 绑定目标模式
    pub value: VarPattern,
    /// 缺省默认值表达式（可选）
    pub default_value: Option<Expr>,
}

/// 变量解构模式
#[derive(Debug, Clone, PartialEq)]
pub enum VarPattern {
    /// 标识符模式：`name`
    Ident(String),
    /// 数组解构：`[a, b, ...rest]`
    Array(Vec<ArrayPatternElem>),
    /// 对象解构：`{ a, b: c, d: { e } }`
    Object(Vec<ObjectPatternProp>),
}

/// 一个模块或脚本的顶层。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Program {
    /// 顶层语句序列
    pub body: Vec<SpannedStmt>,
}

/// ESM 导入声明
#[derive(Debug, Clone, PartialEq)]
pub struct ImportDecl {
    /// 导入的模块路径
    pub source: String,
    /// 导入的符号列表
    pub specifiers: Vec<ImportSpecifier>,
}

/// ESM 导入符号项
#[derive(Debug, Clone, PartialEq)]
pub enum ImportSpecifier {
    /// 默认导入：`import x from 'mod'`
    Default(String),
    /// 命名导入：`import { a as b } from 'mod'`
    Named {
        /// 本地符号名
        local: String,
        /// 导入的源符号名
        imported: String,
    },
    /// 命名空间整包导入：`import * as ns from 'mod'`
    Namespace(String),
}

/// ESM 导出声明
#[derive(Debug, Clone, PartialEq)]
pub enum ExportDecl {
    /// 命名导出：`export const x = 1` 或 `export { a, b as c } [from 'mod']`
    Named {
        /// 内嵌声明语句（如 `const x = 1`）
        decl: Option<Box<SpannedStmt>>,
        /// 导出符号列表
        specifiers: Vec<ExportSpecifier>,
        /// 重导出来源模块路径（可选）
        source: Option<String>,
    },
    /// 默认导出：`export default expr;`
    Default(Box<Expr>),
    /// 命名空间全量重导出：`export * from 'mod'` 或 `export * as ns from 'mod'`
    All {
        /// 模块源路径
        source: String,
        /// 命名空间别名（可选，如 `as ns`）
        alias: Option<String>,
    },
}

/// ESM 导出符号项
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSpecifier {
    /// 本地符号名
    pub local: String,
    /// 导出符号名
    pub exported: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_defaults_to_empty_body() {
        assert!(Program::default().body.is_empty());
    }

    #[test]
    fn binary_expr_nests_operands() {
        let expr = Expr::Binary {
            op: "+".to_owned(),
            left: Box::new(Expr::Number(1.0)),
            right: Box::new(Expr::Ident("x".to_owned())),
        };
        match expr {
            Expr::Binary { op, left, .. } => {
                assert_eq!(op, "+");
                assert_eq!(*left, Expr::Number(1.0));
            }
            other => panic!("expected binary expression, got {other:?}"),
        }
    }
}
