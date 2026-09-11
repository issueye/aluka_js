//! 编译产物单元与层级符号作用域定义。

use aluka_bytecode::{Constant, FuncTemplate, Instr, TryEntry, UpvalueCapture};
use aluka_parser::ast::FunctionDef;
use std::collections::HashMap;

/// 作用域类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScopeKind {
    /// 函数级作用域
    #[default]
    Function,
    /// 块级作用域（let/const）
    Block,
}

/// 单个词法作用域。
#[derive(Debug, Clone, Default)]
pub struct Scope {
    /// 作用域种类
    pub kind: ScopeKind,
    /// 当前作用域声明的变量到局部槽位的映射
    pub bindings: HashMap<String, usize>,
}

/// 层级作用域树。
#[derive(Debug, Clone, Default)]
pub struct ScopeTree {
    /// 作用域栈（栈顶为当前最内层作用域）
    pub scopes: Vec<Scope>,
    /// 下一个可用的局部变量槽位
    pub next_slot: usize,
    /// 历史分配过的最大槽位数（即 num_locals）
    pub max_locals: usize,
}

impl ScopeTree {
    /// 创建新的作用域树，根作用域为函数级作用域。
    #[must_use]
    pub fn new() -> Self {
        Self {
            scopes: vec![Scope {
                kind: ScopeKind::Function,
                bindings: HashMap::new(),
            }],
            next_slot: 0,
            max_locals: 0,
        }
    }

    /// 进入新的作用域（块级或函数级）。
    pub fn enter_scope(&mut self, kind: ScopeKind) {
        self.scopes.push(Scope {
            kind,
            bindings: HashMap::new(),
        });
    }

    /// 离开当前作用域，释放当前作用域占用的局部槽位（用于槽位复用）。
    pub fn leave_scope(&mut self) {
        if let Some(scope) = self.scopes.pop() {
            // 回收该作用域占用的槽位数
            self.next_slot = self.next_slot.saturating_sub(scope.bindings.len());
        }
    }

    /// 在当前作用域声明局部变量，分配新槽位。
    pub fn declare_local(&mut self, name: &str) -> usize {
        let slot = self.next_slot;
        self.next_slot += 1;
        if self.next_slot > self.max_locals {
            self.max_locals = self.next_slot;
        }
        if let Some(cur) = self.scopes.last_mut() {
            cur.bindings.insert(name.to_owned(), slot);
        }
        slot
    }

    /// 查找局部变量（从内层向外层逐级查找）。
    #[must_use]
    pub fn resolve_local(&self, name: &str) -> Option<usize> {
        for scope in self.scopes.iter().rev() {
            if let Some(&slot) = scope.bindings.get(name) {
                return Some(slot);
            }
        }
        None
    }
}

/// 符号解析结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedSymbol {
    /// 局部变量槽位
    Local(usize),
    /// 上值（闭包捕获）索引
    Upvalue(usize),
    /// 全局变量
    Global,
}

/// 循环作用域控制流跳转收集（用于 break / continue 目标地址回填）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoopScope {
    /// 待回填至循环结束位置的 break 跳转指令索引
    pub break_jumps: Vec<usize>,
    /// 待回填至循环更新/步进位置的 continue 跳转指令索引
    pub continue_jumps: Vec<usize>,
}

/// 父级词法作用域符号信息，支持多层嵌套闭包向外逐级捕获变量（直接局部变量或父级上值）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParentScopeInfo {
    /// 父级局部变量映射：变量名 -> 局部槽位
    pub locals: HashMap<String, usize>,
    /// 父级上值映射：变量名 -> Upvalue 索引
    pub upvalues: HashMap<String, usize>,
}

impl ParentScopeInfo {
    /// 创建新的父级作用域快照
    #[must_use]
    pub fn new(locals: HashMap<String, usize>, upvalues: HashMap<String, usize>) -> Self {
        Self { locals, upvalues }
    }
}

/// 一个函数（或顶层脚本）的编译产物单元。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompiledUnit {
    /// 指令序列
    pub code: Vec<Instr>,
    /// 常量池
    pub constants: Vec<Constant>,
    /// 局部槽位数
    pub locals: usize,
    /// 预估最大栈深
    pub max_stack: u32,
    /// 符号到局部变量槽位的映射表
    pub symbol_map: HashMap<String, usize>,
    /// 符号到 Upvalue 索引的映射表（闭包捕获）
    pub upvalue_map: HashMap<String, usize>,
    /// Try 异常表项
    pub try_table: Vec<TryEntry>,
    /// 上值捕获表项（闭包支持）
    pub upvalues: Vec<UpvalueCapture>,
    /// 形式参数数量
    pub num_params: u32,
    /// 循环作用域上下文栈（break/continue 回填）
    pub loop_stack: Vec<LoopScope>,
    /// 表达式闭包占位回填表项：(指令流索引, 函数定义, 创建时的父级作用域快照)
    pub closure_backpatches: Vec<(usize, FunctionDef, ParentScopeInfo)>,
    /// 隶属的类模板 ID（如果有）
    pub class_id: Option<usize>,
    /// 是否为变长参数函数
    pub is_var_args: bool,
    /// 块级作用域遮蔽日志：let/const 声明进入新槽时记录 (名, 外层槽)，
    /// 块结束按记录恢复符号表（后进先出）
    pub scope_shadow_log: Vec<(String, Option<usize>)>,
    /// 当前嵌套块深度（0 为函数/模块顶层）
    pub block_depth: usize,
    /// 函数体含直接 `eval(...)` 调用形态（动态求值作用域降级标记：
    /// 运行时据此物化局部名表，支持直接求值的词法穿透与写回）
    pub has_direct_eval: bool,
    /// 隐式全局模式（eval 全局作用域求值用）：main 层未声明赋值与
    /// var/function 声明直接落全局表，而非 main 局部槽
    pub implicit_globals: bool,
    /// LCOV 行覆盖表：(指令索引, 源码行号)。语句起始登记；空 = 无覆盖信息。
    pub line_table: Vec<(u32, u32)>,
    /// 行覆盖登记开关（ModuleCompiler.line_coverage 转发；默认关）
    pub line_coverage: bool,
}

impl CompiledUnit {
    /// 转换为可被 Verifier 校验和 VM 执行的函数模板。
    #[must_use]
    pub fn to_func_template(self, name: &str) -> FuncTemplate {
        let max_stack = if self.max_stack == 0 {
            crate::max_stack::compute_max_stack(&self.code, &self.try_table)
        } else {
            self.max_stack
        };

        FuncTemplate {
            name: name.to_owned(),
            num_params: self.num_params,
            num_locals: self.locals as u32,
            is_var_args: self.is_var_args,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code: self.code,
            max_stack,
            source_file: "compiled.js".to_owned(),
            constants: self.constants,
            upvalues: self.upvalues,
            try_table: self.try_table,
            line_table: self.line_table,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_tree_block_slot_reuse() {
        let mut tree = ScopeTree::new();
        // 函数级变量 a -> slot 0
        let slot_a = tree.declare_local("a");
        assert_eq!(slot_a, 0);
        assert_eq!(tree.max_locals, 1);

        // 进入块级作用域 1
        tree.enter_scope(ScopeKind::Block);
        let slot_b = tree.declare_local("b");
        let slot_c = tree.declare_local("c");
        assert_eq!(slot_b, 1);
        assert_eq!(slot_c, 2);
        assert_eq!(tree.max_locals, 3);
        assert_eq!(tree.resolve_local("b"), Some(1));
        assert_eq!(tree.resolve_local("a"), Some(0));

        // 离开块级作用域 1，槽位应释放回退到 1
        tree.leave_scope();
        assert_eq!(tree.resolve_local("b"), None);
        assert_eq!(tree.next_slot, 1);
        assert_eq!(tree.max_locals, 3); // 历史峰值保留为 3

        // 进入互斥的块级作用域 2，槽位复用从 1 开始
        tree.enter_scope(ScopeKind::Block);
        let slot_d = tree.declare_local("d");
        assert_eq!(slot_d, 1, "块级作用域 2 的变量 d 必须复用槽位 1");
        assert_eq!(tree.max_locals, 3);
        tree.leave_scope();
    }
}
