//! 生成器（Generator）运行时：调用即建对象、`next()` 驱动、`YIELD` 挂起帧。
//!
//! 递归解释器模型下的帧挂起方案：
//! - 调用 `is_generator` 函数**不执行函数体**，仅创建 [`HeapObject::Generator`]
//!   身份对象并在 [`Vm::generators`] 注册表登记初始状态；
//! - `next()` 把生成器帧上下文（locals / 逻辑栈 / 上值 / try 栈）换入 VM，
//!   从挂起 pc 继续执行；`Op::Yield` 把恢复点写入 [`Vm::yield_pc`] 后以
//!   [`VmError::Yielded`] 沿调用链上抛，驱动层捕获并快照挂起；
//! - 生成器帧的操作数栈与调用者共享同一 `Vec`：进入驱动时记录调用者栈高为界，
//!   挂起时 `split_off` 出逻辑栈快照，恢复时拼回当前栈顶（与调用点栈高解耦）。

use crate::exception::TryHandler;
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Upvalue, Value};
use aluka_bytecode::{Constant, FuncTemplate, TryEntry};
use std::collections::HashMap;

/// 生成器挂起时的帧快照。
#[derive(Debug)]
pub(crate) struct SuspendedFrame {
    /// 恢复执行起点（`YIELD` 的下一条指令索引）
    pub(crate) pc: usize,
    /// 生成器帧局部槽位
    pub(crate) locals: Vec<Value>,
    /// 生成器帧逻辑操作数栈
    pub(crate) stack: Vec<Value>,
    /// 生成器帧上值（`Rc` 共享句柄，跨挂起持续有效）
    pub(crate) upvalues: Vec<Upvalue>,
    /// 生成器帧活跃的打开上值表
    pub(crate) open_upvalues: HashMap<usize, Upvalue>,
    /// 生成器帧 try handler 栈
    pub(crate) try_stack: Vec<TryHandler>,
}

/// 一个生成器对象的完整生命周期状态。
#[derive(Debug)]
pub(crate) struct GeneratorState {
    /// 生成器函数模板索引
    pub(crate) func_idx: usize,
    /// 绑定的 `this`
    pub(crate) this_val: Value,
    /// 调用生成器函数时传入的实参（首帧绑定后取走）
    pub(crate) args: Vec<Value>,
    /// MakeClosure 捕获的上值
    pub(crate) upvalues: Vec<Upvalue>,
    /// 函数模板常量池（执行期间换入 `current_constants`；Rc 共享零拷贝）
    pub(crate) constants: std::rc::Rc<Vec<Constant>>,
    /// 函数模板 Try 表
    pub(crate) try_table: Vec<TryEntry>,
    /// 函数局部槽位总数（首帧初始化用）
    pub(crate) num_locals: usize,
    /// 函数固定参数个数（首帧绑定用）
    pub(crate) num_params: usize,
    /// 是否 varargs（首帧 rest 打包用）
    pub(crate) is_var_args: bool,
    /// 挂起帧；`None` 表示尚未启动
    pub(crate) frame: Option<SuspendedFrame>,
    /// 生成器是否已结束
    pub(crate) done: bool,
}

impl Vm {
    /// 判断值是否为生成器对象。
    pub(crate) fn is_generator_obj(&self, val: Value) -> bool {
        matches!(
            val,
            Value::Object(r)
                if matches!(self.heap.get(r.0 as usize), Some(HeapObject::Generator))
        )
    }

    /// 创建生成器对象并登记初始状态（不执行函数体，对齐 JS 语义）。
    pub(crate) fn make_generator(
        &mut self,
        tmpl: &FuncTemplate,
        func_idx: usize,
        this_val: Value,
        args: &[Value],
        upvalues: Vec<Upvalue>,
    ) -> Value {
        let state = GeneratorState {
            func_idx,
            this_val,
            args: args.to_vec(),
            upvalues,
            constants: self.module_constants[func_idx].clone(),
            try_table: tmpl.try_table.clone(),
            num_locals: tmpl.num_locals as usize,
            num_params: tmpl.num_params as usize,
            is_var_args: tmpl.is_var_args,
            frame: None,
            done: false,
        };
        let idx = self.push_object(HeapObject::Generator);
        self.generators.insert(idx.0, state);
        Value::Object(idx)
    }

    /// 驱动生成器执行到下一个 `YIELD` 或结束，返回 `{ value, done }` 结果对象。
    ///
    /// `injected` 是 `next(v)` 的注入值（挂起恢复时压入栈顶，作为 `YIELD`
    /// 表达式的求值结果）；首次启动与无参 `next()` 为 `undefined`。
    pub(crate) fn drive_generator(
        &mut self,
        gen_ref: aluka_core::ObjectRef,
        injected: Option<Value>,
    ) -> Result<Value, VmError> {
        let key = gen_ref.0;
        if !self.generators.contains_key(&key) {
            let msg = self.alloc_string("TypeError: not a generator".to_owned());
            return Err(VmError::Thrown(Value::Object(msg)));
        }
        if self.generators[&key].done {
            return Ok(self.make_generator_result(None, true));
        }

        // 保存调用者帧上下文；base = 调用者栈高（生成器逻辑栈的分界）
        let caller = CallerFrame::save(self);
        let base = self.stack.len();

        // 取出模板数据（执行期间不能持有 &mut 借用）
        let (code, constants, try_table) = {
            let state = &self.generators[&key];
            let tmpl = &self.module_functions[state.func_idx];
            (
                tmpl.code.clone(),
                state.constants.clone(),
                state.try_table.clone(),
            )
        };

        // 换入生成器帧上下文（先把所需字段拷出，避免持有 self 借用执行变更）
        let frame = self
            .generators
            .get_mut(&key)
            .expect("key 已确认存在")
            .frame
            .take();
        let start_pc = match frame {
            None => {
                let (num_locals, num_params, is_var_args, this_val, args, upvalues) = {
                    let state = &self.generators[&key];
                    (
                        state.num_locals,
                        state.num_params,
                        state.is_var_args,
                        state.this_val,
                        state.args.clone(),
                        state.upvalues.clone(),
                    )
                };
                self.locals = vec![Value::Undefined; num_locals];
                self.bind_call_args(this_val, &args, num_params, is_var_args);
                self.current_upvalues = upvalues;
                0
            }
            Some(frame) => {
                let SuspendedFrame {
                    pc,
                    locals,
                    stack,
                    upvalues,
                    open_upvalues,
                    try_stack,
                } = frame;
                self.locals = locals;
                self.current_upvalues = upvalues;
                self.open_upvalues = open_upvalues;
                self.try_stack = try_stack;
                self.stack.extend(stack);
                // 注入值（next(v) 的 v）压栈成为 yield 表达式求值结果；
                // 无注入（如 fromAsync 内部驱动）按 next() 无参语义压 undefined
                self.stack.push(injected.unwrap_or(Value::Undefined));
                pc
            }
        };
        self.current_try_table = try_table;

        self.current_func_idx = self.generators[&key].func_idx as i64;
        let outcome = self.run_with_constants_at(&code, constants, start_pc);

        // 收割生成器帧：先截走逻辑栈，再收割执行上下文字段
        let gen_stack = self.stack.split_off(base);
        let gen_locals = std::mem::take(&mut self.locals);
        let gen_upvalues = std::mem::take(&mut self.current_upvalues);
        let gen_open_upvalues = std::mem::take(&mut self.open_upvalues);
        let gen_try_stack = std::mem::take(&mut self.try_stack);
        caller.restore(self);

        let state = self.generators.get_mut(&key).expect("key 已确认存在");
        match outcome {
            Ok(ret) => {
                state.done = true;
                Ok(self.make_generator_result(Some(ret), true))
            }
            Err(VmError::Yielded(yielded)) => {
                state.frame = Some(SuspendedFrame {
                    pc: self.yield_pc,
                    locals: gen_locals,
                    stack: gen_stack,
                    upvalues: gen_upvalues,
                    open_upvalues: gen_open_upvalues,
                    try_stack: gen_try_stack,
                });
                Ok(self.make_generator_result(Some(yielded), false))
            }
            Err(err) => {
                state.done = true;
                Err(err)
            }
        }
    }

    /// 构造 `{ value, done }` 结果对象。
    fn make_generator_result(&mut self, value: Option<Value>, done: bool) -> Value {
        let instance = self.alloc_ordinary();
        let val = value.unwrap_or(Value::Undefined);
        let _ = self.set_property(Value::Object(instance), "value", val);
        let _ = self.set_property(Value::Object(instance), "done", Value::Boolean(done));
        Value::Object(instance)
    }
}

/// 调用者帧上下文快照（生成器驱动期间换出、结束后换回）。
pub(crate) struct CallerFrame {
    locals: Vec<Value>,
    constants: std::rc::Rc<Vec<Constant>>,
    upvalues: Vec<Upvalue>,
    open_upvalues: HashMap<usize, Upvalue>,
    try_stack: Vec<TryHandler>,
    try_table: Vec<TryEntry>,
}

impl CallerFrame {
    pub(crate) fn save(vm: &mut Vm) -> Self {
        Self {
            locals: std::mem::take(&mut vm.locals),
            constants: std::mem::replace(&mut vm.current_constants, std::rc::Rc::new(Vec::new())),
            upvalues: std::mem::take(&mut vm.current_upvalues),
            open_upvalues: std::mem::take(&mut vm.open_upvalues),
            try_stack: std::mem::take(&mut vm.try_stack),
            try_table: std::mem::take(&mut vm.current_try_table),
        }
    }

    pub(crate) fn restore(self, vm: &mut Vm) {
        vm.locals = self.locals;
        vm.current_constants = self.constants;
        vm.current_upvalues = self.upvalues;
        vm.open_upvalues = self.open_upvalues;
        vm.try_stack = self.try_stack;
        vm.current_try_table = self.try_table;
    }
}
