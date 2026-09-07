//! alisp —— M4 玩具 DSL 前端（验收「新语法零成本接入」）。
//!
//! 极简 Lisp → Aluka ISA 字节码。**只依赖 `aluka-bytecode` 公共 API**
//! （指令集 + 序列化 + 校验器），零改后端（aluka-vm）——这正是 M4 的
//! 验收点：任何符合 ISA 契约的前端产物都可在 aluvm 上执行。
//!
//! # 语言（v1 子集）
//!
//! ```lisp
//! (defn add (a b) (+ a b))          ; 顶层函数定义（全局绑定，支持递归）
//! (def name expr)                    ; 顶层变量
//! (print expr)                       ; console.log 输出
//! (if c a b)                         ; 条件（值产生）
//! (+ a b) (- a b) (* a b) (/ a b)    ; 算术（左结合，支持多目）
//! (< a b) (> a b) (= a b)            ; 比较
//! (fn (x) body...)                   ; 匿名函数
//! 42 "字符串" symbol                  ; 字面量与引用
//! ```
//!
//! 语义：函数尾表达式为返回值；参数占槽 1..=n（槽 0 为 this，对齐调用
//! 约定）；顶层函数经 `LoadGlobal/StoreGlobal` 绑定（支持递归）。

use aluka_bytecode::{
    ALUC_CONTAINER_VERSION, Constant, FuncTemplate, Instr, Op, TryEntry, UpvalueCapture,
};

/// 源程序 S 表达式。
#[derive(Debug, Clone)]
pub enum Sexp {
    /// 整数字面量
    Int(i64),
    /// 字符串字面量
    Str(String),
    /// 符号（变量名/特殊形式名/运算符）
    Sym(String),
    /// 列表
    List(Vec<Sexp>),
}

/// 编译错误（带定位信息的最小集合）。
#[derive(Debug)]
pub struct CompileError(pub String);

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "alisp: {}", self.0)
    }
}

/// 词法 + 语法分析：源码 → S 表达式序列。
///
/// # Errors
/// 括号不匹配、字符串未闭合、空列表时返回 [`CompileError`]。
pub fn parse_program(src: &str) -> Result<Vec<Sexp>, CompileError> {
    let mut tokens: Vec<Sexp> = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b';' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'(' | b'[' => {
                let (sexp, next) = read_list(src, i + 1)?;
                tokens.push(sexp);
                i = next;
            }
            b')' | b']' => return Err(CompileError("多余的右括号".to_owned())),
            c if c.is_ascii_whitespace() => i += 1,
            b'"' => {
                let (s, next) = read_string(src, i + 1)?;
                tokens.push(Sexp::Str(s));
                i = next;
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !matches!(bytes[i], b'(' | b')' | b'[' | b']' | b';' | b'"')
                {
                    i += 1;
                }
                let text = &src[start..i];
                tokens.push(match text.parse::<i64>() {
                    Ok(v) => Sexp::Int(v),
                    Err(_) => Sexp::Sym(text.to_owned()),
                });
            }
        }
    }
    Ok(tokens)
}

/// 读取列表（含嵌套）。
fn read_list(src: &str, mut i: usize) -> Result<(Sexp, usize), CompileError> {
    let bytes = src.as_bytes();
    let mut items = Vec::new();
    loop {
        let Some(&c) = bytes.get(i) else {
            return Err(CompileError("列表未闭合".to_owned()));
        };
        match c {
            b')' | b']' => return Ok((Sexp::List(items), i + 1)),
            c if c.is_ascii_whitespace() => i += 1,
            b';' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                let (s, next) = read_string(src, i + 1)?;
                items.push(Sexp::Str(s));
                i = next;
            }
            b'(' | b'[' => {
                let (inner, next) = read_list(src, i + 1)?;
                items.push(inner);
                i = next;
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !matches!(bytes[i], b'(' | b')' | b'[' | b']' | b';' | b'"')
                {
                    i += 1;
                }
                let text = &src[start..i];
                items.push(match text.parse::<i64>() {
                    Ok(v) => Sexp::Int(v),
                    Err(_) => Sexp::Sym(text.to_owned()),
                });
            }
        }
    }
}

/// 读取字符串字面量（`\\n` / `\\t` / `\\\"` / `\\\\` 转义）。
fn read_string(src: &str, mut i: usize) -> Result<(String, usize), CompileError> {
    let bytes = src.as_bytes();
    let mut out = String::new();
    loop {
        let Some(&c) = bytes.get(i) else {
            return Err(CompileError("字符串未闭合".to_owned()));
        };
        match c {
            b'"' => return Ok((out, i + 1)),
            b'\\' => {
                i += 1;
                match bytes.get(i) {
                    Some(b'n') => out.push('\n'),
                    Some(b't') => out.push('\t'),
                    Some(b'"') => out.push('"'),
                    Some(b'\\') => out.push('\\'),
                    _ => return Err(CompileError("未知转义".to_owned())),
                }
                i += 1;
            }
            _ => {
                let ch = src[i..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
}

/// 常量池去重（线性查重，Lisp 程序常量量级小）。
#[derive(Default)]
struct Pool {
    entries: Vec<Constant>,
}

impl Pool {
    fn intern(&mut self, c: Constant) -> u32 {
        if let Some(i) = self.entries.iter().position(|e| *e == c) {
            return i as u32;
        }
        let i = self.entries.len();
        self.entries.push(c);
        i as u32
    }
}

/// 函数编译上下文。
struct FnCtx {
    /// 函数名（调试用）
    name: String,
    /// 是否主函数（顶层 def 走 StoreGlobal，供函数体 LoadGlobal 递归解析）
    is_main: bool,
    /// 参数个数
    num_params: usize,
    /// 已分配槽位总数（槽 0 = this，1..=n = 参数，其后为 def 局部）
    slots: usize,
    /// 槽位 → 名（局部查表）
    locals: Vec<(String, usize)>,
    /// 顶层函数名集（函数体内经 LoadGlobal 解析）
    globals: std::collections::HashSet<String>,
    code: Vec<Instr>,
}

/// 编译器：S 表达式程序 → 模块（函数 0 为主函数）。
#[derive(Default)]
pub struct Compiler {
    pool: Pool,
    pending_fns: Vec<FnCtx>,
    /// 顶层全局名（defn/def）
    globals: std::collections::HashSet<String>,
}

impl Compiler {
    /// 创建编译器。
    #[must_use]
    pub fn new() -> Self {
        Self {
            pool: Pool::default(),
            pending_fns: Vec::new(),
            globals: std::collections::HashSet::new(),
        }
    }

    /// 编译整个程序为字节码模块（函数 0 = 主函数）。
    ///
    /// # Errors
    /// 未知符号、特殊形式参数错误时返回 [`CompileError`]。
    pub fn compile_module(&mut self, forms: &[Sexp]) -> Result<BytecodeModuleOut, CompileError> {
        // 第一遍：收集顶层 defn/def 名（递归可解析）
        for form in forms {
            if let Sexp::List(items) = form {
                if matches!(items.first(), Some(Sexp::Sym(k)) if k == "defn") {
                    if let Some(Sexp::Sym(name)) = items.get(1) {
                        self.globals.insert(name.clone());
                    }
                }
                if matches!(items.first(), Some(Sexp::Sym(k)) if k == "def") {
                    if let Some(Sexp::Sym(name)) = items.get(1) {
                        self.globals.insert(name.clone());
                    }
                }
            }
        }

        // 第二遍：defn → 独立函数模板；其余进主函数
        let mut main = FnCtx {
            name: "main".to_owned(),
            is_main: true,
            num_params: 0,
            slots: 1,
            locals: Vec::new(),
            globals: self.globals.clone(),
            code: Vec::new(),
        };
        for form in forms {
            self.compile_form(&mut main, form, true)?;
        }
        // 主函数末值：最后一条留栈者即返回值；无值则补 null
        if !leaves_value_last(forms) {
            let null = main.code.len();
            emit(
                &mut main.code,
                Op::PushConst,
                self.pool.intern(Constant::Null),
            );
            let _ = null;
        }
        // 主函数以 Return 收尾（栈上恰一个值）
        emit(&mut main.code, Op::Return, 0);
        // 统一冻结：全部函数共享最终常量池快照（常量索引全局分配）
        self.pending_fns.insert(0, main);
        let final_pool = self.pool.entries.clone();
        let mut functions = Vec::with_capacity(self.pending_fns.len());
        let pending = std::mem::take(&mut self.pending_fns);
        for ctx in pending {
            functions.push(self.finish_fn(ctx, &final_pool, 0));
        }

        Ok(BytecodeModuleOut {
            module: aluka_bytecode::BytecodeModule {
                header_extras: Vec::new(),
                version: 30, // ISA 语义版本
                functions,
                classes: Vec::new(),
            },
            container_version: ALUC_CONTAINER_VERSION,
        })
    }

    /// 编译单个 form 到 `fn`（`value_producing`：是否以值结算调用点的
    /// 栈不变量）。
    fn compile_form(
        &mut self,
        f: &mut FnCtx,
        form: &Sexp,
        _value_producing: bool,
    ) -> Result<bool, CompileError> {
        match form {
            Sexp::Int(v) => {
                let idx = self.pool.intern(Constant::Number(*v as f64));
                emit(&mut f.code, Op::PushConst, idx);
                Ok(true)
            }
            Sexp::Str(s) => {
                let idx = self.pool.intern(Constant::String(s.clone()));
                emit(&mut f.code, Op::PushConst, idx);
                Ok(true)
            }
            Sexp::Sym(name) => {
                if let Some((_, slot)) = f.locals.iter().rev().find(|(n, _)| n == name) {
                    emit(&mut f.code, Op::LoadLocal, *slot as u32);
                    return Ok(true);
                }
                if f.globals.contains(name) {
                    emit(
                        &mut f.code,
                        Op::LoadGlobal,
                        self.pool.intern(Constant::String(name.clone())),
                    );
                    return Ok(true);
                }
                Err(CompileError(format!("未知符号: {name}")))
            }
            Sexp::List(items) => {
                let Some(Sexp::Sym(head)) = items.first() else {
                    return Err(CompileError("列表首项必须是符号".to_owned()));
                };
                match head.as_str() {
                    "def" | "defn" => {
                        let Some(Sexp::Sym(name)) = items.get(1) else {
                            return Err(CompileError("def 缺少名字".to_owned()));
                        };
                        // defn 是 (def name (fn params body...)) 的糖
                        let value = if head == "defn" {
                            let params = items
                                .get(2)
                                .ok_or_else(|| CompileError("defn 缺少参数表".to_owned()))?;
                            if items.len() < 4 {
                                return Err(CompileError("defn 缺少函数体".to_owned()));
                            }
                            let mut fn_form = vec![Sexp::Sym("fn".to_owned()), params.clone()];
                            fn_form.extend(items[3..].iter().cloned());
                            Sexp::List(fn_form)
                        } else {
                            items
                                .get(2)
                                .ok_or_else(|| CompileError("def 缺少值".to_owned()))?
                                .clone()
                        };
                        let leaves = self.compile_form(f, &value, true)?;
                        if !leaves {
                            emit(&mut f.code, Op::PushConst, self.pool.intern(Constant::Null));
                        }
                        if f.is_main {
                            // 顶层绑定走全局（不占局部槽）：函数体内的同名符号
                            // 经 LoadGlobal 解析（递归与跨函数调用）；主函数自身
                            // 的后续引用同样落 LoadGlobal
                            emit(
                                &mut f.code,
                                Op::StoreGlobal,
                                self.pool.intern(Constant::String(name.clone())),
                            );
                        } else {
                            let slot = match f.locals.iter().rev().find(|(n, _)| n == name) {
                                Some((_, s)) => *s,
                                None => {
                                    let s = f.slots;
                                    f.slots += 1;
                                    f.locals.push((name.clone(), s));
                                    s
                                }
                            };
                            emit(&mut f.code, Op::StoreLocal, slot as u32);
                        }
                        // def 表达式本身不产生值
                        emit(&mut f.code, Op::PushConst, self.pool.intern(Constant::Null));
                        Ok(true)
                    }
                    "fn" => {
                        let params = items
                            .get(1)
                            .ok_or_else(|| CompileError("fn 缺少参数表".to_owned()))?;
                        let Sexp::List(param_list) = params else {
                            return Err(CompileError("fn 参数表必须是列表".to_owned()));
                        };
                        let mut param_names = Vec::new();
                        for p in param_list {
                            let Sexp::Sym(n) = p else {
                                return Err(CompileError("fn 参数必须是符号".to_owned()));
                            };
                            param_names.push(n.clone());
                        }
                        let body = &items[2..];
                        let func_idx = self.pending_fns.len() as u32 + 1;
                        // 主函数占 0；嵌套函数追加进 self.functions
                        let mut sub = FnCtx {
                            name: format!("lisp:anon{}", func_idx),
                            is_main: false,
                            num_params: param_names.len(),
                            slots: 1 + param_names.len(),
                            locals: param_names
                                .iter()
                                .enumerate()
                                .map(|(i, n)| (n.clone(), i + 1))
                                .collect(),
                            globals: f.globals.clone(),
                            code: Vec::new(),
                        };
                        for (i, b) in body.iter().enumerate() {
                            let last = i + 1 == body.len();
                            let leaves = self.compile_form(&mut sub, b, true)?;
                            if !last {
                                if leaves {
                                    emit(&mut sub.code, Op::Pop, 0);
                                }
                            } else if !leaves {
                                emit(
                                    &mut sub.code,
                                    Op::PushConst,
                                    self.pool.intern(Constant::Null),
                                );
                            }
                        }
                        emit(&mut sub.code, Op::Return, 0);
                        self.pending_fns.push(sub);
                        emit(&mut f.code, Op::MakeClosure, func_idx);
                        Ok(true)
                    }
                    "if" => {
                        let cond = items
                            .get(1)
                            .ok_or_else(|| CompileError("if 缺少条件".to_owned()))?;
                        let then_f = items
                            .get(2)
                            .ok_or_else(|| CompileError("if 缺少 then".to_owned()))?;
                        let else_f = items
                            .get(3)
                            .ok_or_else(|| CompileError("if 缺少 else".to_owned()))?;
                        self.compile_form(f, cond, true)?;
                        let jmp_false_at = f.code.len();
                        emit(&mut f.code, Op::JmpFalsePop, 0);
                        let leaves_then = self.compile_form(f, then_f, true)?;
                        let jmp_end_at = f.code.len();
                        emit(&mut f.code, Op::Jmp, 0);
                        // else 入口 = Jmp 之后（JmpFalsePop 为假时跳到 else）
                        let else_pc = jmp_end_at + 1;
                        patch_jump(&mut f.code, jmp_false_at, else_pc);
                        let leaves_else = self.compile_form(f, else_f, true)?;
                        if !leaves_then {
                            if leaves_else {
                                // then 无值：对齐栈（弹 else 值补 null）
                                return Err(CompileError(
                                    "if 两分支的值产生性不一致（then 无值）".to_owned(),
                                ));
                            }
                        } else if !leaves_else {
                            return Err(CompileError(
                                "if 两分支的值产生性不一致（else 无值）".to_owned(),
                            ));
                        }
                        let end_pc = f.code.len();
                        patch_jump(&mut f.code, jmp_end_at, end_pc);
                        Ok(true)
                    }
                    "print" => {
                        let arg = items
                            .get(1)
                            .ok_or_else(|| CompileError("print 缺少参数".to_owned()))?;
                        emit(
                            &mut f.code,
                            Op::LoadGlobal,
                            self.pool.intern(Constant::String("console".to_owned())),
                        );
                        self.compile_form(f, arg, true)?;
                        let log_idx = self.pool.intern(Constant::String("log".to_owned()));
                        // 方法名经 CALL_METHOD 的常量索引读取（不进操作数栈）
                        emit(&mut f.code, Op::CallMethod, (1 << 16) | log_idx);
                        // print 无值
                        emit(&mut f.code, Op::PushConst, self.pool.intern(Constant::Null));
                        Ok(true)
                    }
                    "+" | "-" | "*" | "/" | "<" | ">" | "=" => {
                        if items.len() < 3 {
                            return Err(CompileError(format!("{head} 至少两个参数")));
                        }
                        self.compile_form(f, &items[1], true)?;
                        for r in &items[2..] {
                            self.compile_form(f, r, true)?;
                            let op = match head.as_str() {
                                "+" => Op::Add,
                                "-" => Op::Sub,
                                "*" => Op::Mul,
                                "/" => Op::Div,
                                "<" => Op::Lt,
                                ">" => Op::Gt,
                                _ => Op::Eq,
                            };
                            emit(&mut f.code, op, 0);
                        }
                        Ok(true)
                    }
                    _ => {
                        // 通用调用：head 解析（局部/全局），参数依次，Call n
                        let head_val = Sexp::Sym(head.clone());
                        self.compile_form(f, &head_val, true)?;
                        let n = items.len() - 1;
                        for a in &items[1..] {
                            self.compile_form(f, a, true)?;
                        }
                        emit(&mut f.code, Op::Call, n as u32);
                        Ok(true)
                    }
                }
            }
        }
    }

    /// 冻结函数上下文为模板（`pool` 为最终常量池快照）。
    fn finish_fn(&self, ctx: FnCtx, pool: &[Constant], _idx: u32) -> FuncTemplate {
        FuncTemplate {
            name: ctx.name,
            num_params: ctx.num_params as u32,
            num_locals: ctx.slots as u32,
            is_var_args: false,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code: ctx.code,
            max_stack: 256,
            source_file: String::new(),
            constants: pool.to_vec(),
            upvalues: Vec::new(),
            try_table: Vec::<TryEntry>::new(),
        }
    }
}

/// 写指令。
fn emit(code: &mut Vec<Instr>, op: Op, operand: u32) {
    code.push(Instr::new(op, operand));
}

/// 回填跳转：`at` 处的 Jmp/JmpFalsePop 目标改为 `target_pc`（相对下一指令
/// 的 24 位有符号字节偏移，对齐 VM `compute_jump_target`）。
fn patch_jump(code: &mut [Instr], at: usize, target_pc: usize) {
    let signed = (target_pc as i32 * 4) - ((at as i32 * 4) + 4);
    code[at].operand = (signed as i64 & 0xFF_FFFF) as u32;
}

/// 判断程序最后一个 form 是否产生值（主函数 Return 前的补栈依据）。
fn leaves_value_last(forms: &[Sexp]) -> bool {
    let Some(last) = forms.last() else {
        return false;
    };
    match last {
        Sexp::List(items) => !matches!(
            items.first(),
            Some(Sexp::Sym(k)) if k == "def" || k == "defn" || k == "print"
        ),
        _ => true,
    }
}

/// 编译产物：模块 + 容器版本。
pub struct BytecodeModuleOut {
    /// 字节码模块
    pub module: aluka_bytecode::BytecodeModule,
    /// 容器版本（当前 1）
    pub container_version: u32,
}

// UpvalueCapture 在 v1 Lisp 中未用到（无嵌套闭包捕获），显式引用防未用警告
const _: fn() = || {
    let _ = std::mem::size_of::<UpvalueCapture>();
};
