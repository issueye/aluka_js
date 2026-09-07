//! 动态代码求值子系统（`eval` 与 `new Function`，语义对齐 Node.js 22 LTS）。
//!
//! 架构契约：后端 [`Vm`] 只接收字节码，不依赖前端编译器。动态求值经
//! 「运行时编译器 Hook」（[`Vm::set_eval_provider`]）由宿主装配编译入口，
//! 产出的 [`BytecodeModule`] **强制**通过 `verify()` 静态安全校验后再执行。
//!
//! 执行机制（append-only，单表 VM 多模块的关键不变量）：
//! 动态模块的函数/类模板**追加**进全局表并重写索引（`MakeClosure` /
//! `MakeClass` 操作数 += 基址），已加载模块的索引不受影响——与 `require`
//! 的嵌套加载完全同型，调用方帧在求值后继续有效。
//!
//! 直接求值词法穿透：编译器给含裸 `eval(...)` 调用的函数模板注入局部名表
//! 常量（`__aluka_locals__`，见 `aluka-compiler::attach_direct_eval_marker`）。
//! 运行时据此把当前帧局部**快照注入全局表**后以脚本形态执行求值体，
//! 结束后把全局表终值**写回局部槽**（var 语义），并恢复同名全局原值。
//! 间接求值与 `new Function` 强制在全局作用域执行，不注入局部。

use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_bytecode::BytecodeModule;

/// 动态编译器 Hook：源码 → 字节码模块。
pub(crate) type EvalProvider =
    std::rc::Rc<std::cell::RefCell<dyn FnMut(&str) -> Result<BytecodeModule, String>>>;

/// 直接求值调用的专管全局名（编译器把 `eval(...)` 改写到该名，
/// 运行时据此区分直接/间接求值）。
pub const DIRECT_EVAL_GLOBAL: &str = "%aluka_direct_eval%";

/// 局部名表常量前缀（见 `aluka-compiler` 的 `attach_direct_eval_marker`）。
pub const LOCALS_MARKER_PREFIX: &str = "__aluka_locals__\u{1}";

/// 上值名表常量前缀（见 `aluka-compiler` 的
/// `attach_direct_eval_upvalue_marker`）。
pub const UPVALS_MARKER_PREFIX: &str = "__aluka_upvals__\u{1}";

/// 空求值模块（main 仅 `ReturnUndef`；`eval("")` 等空源码场景免编译）。
#[must_use]
pub fn empty_eval_module() -> BytecodeModule {
    BytecodeModule {
        version: 30,
        functions: vec![aluka_bytecode::FuncTemplate {
            name: "main".to_owned(),
            num_params: 0,
            num_locals: 1,
            is_var_args: false,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code: vec![aluka_bytecode::Instr::new(
                aluka_bytecode::Op::ReturnUndef,
                0,
            )],
            max_stack: 1,
            source_file: String::new(),
            constants: Vec::new(),
            upvalues: Vec::new(),
            try_table: Vec::new(),
        }],
        classes: Vec::new(),
        header_extras: Vec::new(),
    }
}

impl Vm {
    /// 装配运行时编译器 Hook（宿主在执行前调用一次）。
    ///
    /// 未装配时 `eval` / `new Function` 抛 EvalError（Node 的
    /// `--disallow-code-generation-from-strings` 等价形态）。
    pub fn set_eval_provider(
        &mut self,
        provider: impl FnMut(&str) -> Result<BytecodeModule, String> + 'static,
    ) {
        self.eval_provider = Some(std::rc::Rc::new(std::cell::RefCell::new(provider)));
    }

    /// 经 Hook 编译动态源码并**强制** Verifier 静态安全校验。
    fn compile_dynamic(&mut self, src: &str) -> Result<BytecodeModule, VmError> {
        let provider = self.eval_provider.clone().ok_or_else(|| {
            let err = self.alloc_error_instance(
                "EvalError: Code generation from strings disallowed for this context",
            );
            let name = self.alloc_string("EvalError".to_owned());
            let _ = self.set_property(Value::Object(err), "name", Value::Object(name));
            VmError::Thrown(Value::Object(err))
        })?;
        let module = (provider.borrow_mut())(src).map_err(|e| {
            let err = self.alloc_error_instance(&e);
            let name = self.alloc_string("SyntaxError".to_owned());
            let _ = self.set_property(Value::Object(err), "name", Value::Object(name));
            VmError::Thrown(Value::Object(err))
        })?;
        // 动态字节码安全门禁：非法跳转/栈深越界的模块一律拒绝执行
        module.verify().map_err(|e| {
            let err = self.alloc_error_instance(&format!("dynamic bytecode verify: {e}"));
            let name = self.alloc_string("SyntaxError".to_owned());
            let _ = self.set_property(Value::Object(err), "name", Value::Object(name));
            VmError::Thrown(Value::Object(err))
        })?;
        Ok(module)
    }

    /// 追加动态模块进全局函数/类表（重写索引基址），返回其 main 函数索引。
    ///
    /// 与 `require` 的嵌套加载同型：append-only 保证已加载模块的索引不变。
    fn append_module(&mut self, module: &BytecodeModule) -> usize {
        let fn_base = self.module_functions.len() as u32;
        let class_base = self.module_classes.len() as u32;
        let mut funcs: Vec<aluka_bytecode::FuncTemplate> = module.functions.to_vec();
        for f in funcs.iter_mut() {
            for instr in f.code.iter_mut() {
                match instr.op {
                    aluka_bytecode::Op::MakeClosure => instr.operand += fn_base,
                    aluka_bytecode::Op::MakeClass => instr.operand += class_base,
                    _ => {}
                }
            }
        }
        let mut classes = module.classes.clone();
        for c in classes.iter_mut() {
            c.constructor_index += fn_base;
            for m in c.methods.iter_mut() {
                m.func_index += fn_base;
            }
        }
        while self.module_header_extras.len() < fn_base as usize {
            self.module_header_extras.push(Default::default());
        }
        self.module_header_extras
            .extend(module.header_extras.iter().cloned());
        while self.module_header_extras.len() < fn_base as usize + funcs.len() {
            self.module_header_extras.push(Default::default());
        }
        self.module_functions
            .extend(funcs.iter().cloned().map(std::rc::Rc::new));
        self.module_constants.extend(
            module
                .functions
                .iter()
                .map(|f| std::rc::Rc::new(f.constants.clone())),
        );
        self.module_classes.extend(classes);
        fn_base as usize
    }

    /// 读取当前帧函数的局部名表（无标记 → `None`）。
    fn current_locals_names(&self) -> Option<Vec<String>> {
        const SEP: char = '\u{1}';
        for c in self.current_constants.iter() {
            if let aluka_bytecode::Constant::String(s) = c {
                if let Some(rest) = s.strip_prefix(LOCALS_MARKER_PREFIX) {
                    let names: Vec<String> = rest.split(SEP).map(str::to_owned).collect();
                    return Some(names);
                }
            }
        }
        None
    }

    /// 读取当前帧函数的上值名表（无标记 → 空集）。
    ///
    /// 嵌套函数内的直接求值经此外溢捕获外层词法绑定：名表第 i 项对应
    /// `current_upvalues[i]` 的上值句柄（读写共享同一 RefCell）。
    fn current_upvalue_names(&self) -> Vec<String> {
        const SEP: char = '\u{1}';
        for c in self.current_constants.iter() {
            if let aluka_bytecode::Constant::String(s) = c {
                if let Some(rest) = s.strip_prefix(UPVALS_MARKER_PREFIX) {
                    return rest.split(SEP).map(str::to_owned).collect();
                }
            }
        }
        Vec::new()
    }

    /// `eval(code)` 求值入口。
    ///
    /// `direct=true`（编译器改写的直接调用形态）：把当前帧局部与上值快照
    /// 注入全局表后以脚本执行，结束后写回并恢复；`direct=false`（间接调用）
    /// 纯全局作用域执行。
    pub(crate) fn call_eval(&mut self, direct: bool, args: &[Value]) -> Result<Value, VmError> {
        let src = match args.first().copied() {
            None | Some(Value::Undefined) => return Ok(Value::Undefined),
            Some(v) => self.format_value(v),
        };
        if src.trim().is_empty() {
            return Ok(Value::Undefined);
        }
        // 作用域快照（局部面）：名表第 i 项对应局部槽位 i + 1
        let scope: Vec<(usize, String, Option<Value>, Value)> = if direct {
            self.current_locals_names()
                .map(|names| {
                    names
                        .iter()
                        .enumerate()
                        .filter(|(_, n)| !n.is_empty())
                        .filter_map(|(i, n)| {
                            let slot = i + 1;
                            let local = self.locals.get(slot).copied()?;
                            Some((slot, n.clone(), self.globals.get(n).copied(), local))
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        // 作用域快照（上值面）：名表第 i 项对应 current_upvalues[i] 句柄
        // （嵌套函数内的直接求值经此外溢捕获外层词法绑定，读写共享）
        let mut uv_scope: Vec<(usize, String, Option<Value>)> = Vec::new();
        if direct {
            for (i, n) in self.current_upvalue_names().into_iter().enumerate() {
                if n.is_empty() {
                    continue;
                }
                if let Some(uv) = self.current_upvalues.get(i) {
                    let val = uv.0.borrow().to_owned();
                    let old = self.globals.get(n.as_str()).copied();
                    uv_scope.push((i, n.clone(), old));
                    self.globals.insert(n, val);
                }
            }
        }
        // 局部快照注入全局表（同名全局先让位）
        for (_, name, _, local) in &scope {
            self.globals.insert(name.clone(), *local);
        }
        let module = self.compile_dynamic(&src)?;
        let main_idx = self.append_module(&module);
        let run_res = self.run_func(&self.module_functions[main_idx].clone());
        // 写回 + 恢复（无论求值成败都必须执行，避免全局表被快照污染）
        let writeback = |vm: &mut Vm| {
            for (slot, name, old_global, _) in &scope {
                // 求值期间的赋值已落全局表：终值写回调用帧局部槽
                if let Some(updated) = vm.globals.get(name).copied() {
                    if *slot < vm.locals.len() {
                        vm.locals[*slot] = updated;
                    }
                }
                match old_global {
                    Some(v) => {
                        vm.globals.insert(name.clone(), *v);
                    }
                    None => {
                        vm.globals.remove(name);
                    }
                }
            }
            for (uv_idx, name, old_global) in &uv_scope {
                if let Some(updated) = vm.globals.get(name).copied() {
                    if let Some(uv) = vm.current_upvalues.get(*uv_idx) {
                        *uv.0.borrow_mut() = updated;
                    }
                }
                match old_global {
                    Some(v) => {
                        vm.globals.insert(name.clone(), *v);
                    }
                    None => {
                        vm.globals.remove(name);
                    }
                }
            }
        };
        writeback(self);
        run_res
    }

    /// `new Function(p1, ..., body)`：形参与函数体字符串拼接解析，
    /// 以全局作用域函数模板动态生成闭包。
    pub(crate) fn construct_function(&mut self, args: &[Value]) -> Result<Value, VmError> {
        let mut params = Vec::new();
        let mut body = String::new();
        if let Some((last, rest)) = args.split_last() {
            body = self.format_value(*last);
            for p in rest {
                params.push(self.format_value(*p));
            }
        }
        let src = format!(
            "(function anonymous({}) {{\n{}\n}})",
            params.join(","),
            body
        );
        let module = self.compile_dynamic(&src)?;
        let main_idx = self.append_module(&module);
        // main 的完成值即 `(function(){...})` 表达式的闭包值
        self.run_func(&self.module_functions[main_idx].clone())
    }
}
