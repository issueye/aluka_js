//! 函数调用管理、帧上下文隔离与模块执行入口。

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Upvalue, Value};
use std::cell::RefCell;

thread_local! {
    /// 轻量 JS 调用链（诊断用）：(func_idx, 模板名)，invoke_function 进入时推入、退出时弹出
    static CALL_CHAIN: RefCell<Vec<(usize, String)>> = const { RefCell::new(Vec::new()) };
}

/// 调用链帧守卫：作用域结束时弹出栈顶（含错误传播路径）。
struct FrameGuard;

impl Drop for FrameGuard {
    fn drop(&mut self) {
        CALL_CHAIN.with(|c| {
            c.borrow_mut().pop();
        });
    }
}

/// 打印当前调用链（诊断开关：ALUKA_REQ_DEBUG）。
pub fn dump_call_chain(vm: &Vm, context: &str) {
    if std::env::var("ALUKA_REQ_DEBUG").is_err() {
        return;
    }
    let chain = CALL_CHAIN.with(|c| c.borrow().clone());
    let frames: Vec<String> = chain
        .iter()
        .map(|(f, n)| {
            let src = vm
                .module_functions
                .get(*f)
                .map(|t| t.source_file.clone())
                .unwrap_or_default();
            format!("{f}:{n}@{src}")
        })
        .collect();
    eprintln!("[req-dbg] call chain at {context}: {frames:?}");
}

/// 同步调用的实参暂存区。
///
/// 常见的小参数调用走栈上固定数组，避免每条 `CALL` 都分配临时 `Vec`；
/// 超过 8 个参数才退回堆上的动态数组。该值只在当前指令期间存活，递归
/// 或 NativeFn 重入会创建各自的栈帧，不共享 scratch。
pub(crate) enum CallArgs {
    /// 不超过固定容量的栈上参数数组
    Inline {
        /// 参数值（前 `len` 项有效）
        values: [Value; 8],
        /// 有效参数数
        len: usize,
    },
    /// 大参数调用的动态参数数组
    Heap(Vec<Value>),
}

impl CallArgs {
    /// 从 VM 操作数栈顶按调用顺序收集 `argc` 个参数。
    ///
    /// 调用约定要求栈顶是最后一个实参；小参数直接倒序写入数组后以切片
    /// 视图呈现为正序，避免 `Vec::reverse()`。在栈下溢时返回原有错误。
    pub(crate) fn collect_from_stack(
        &mut self,
        stack: &mut Vec<Value>,
        argc: usize,
    ) -> Result<(), VmError> {
        match self {
            Self::Inline { values, len } if argc <= 8 => {
                for slot in values[..argc].iter_mut().rev() {
                    *slot = stack.pop().ok_or(VmError::StackUnderflow)?;
                }
                *len = argc;
                Ok(())
            }
            Self::Heap(values) if argc > 8 => {
                values.clear();
                for _ in 0..argc {
                    values.push(stack.pop().ok_or(VmError::StackUnderflow)?);
                }
                values.reverse();
                Ok(())
            }
            _ => {
                let mut values = Vec::with_capacity(argc);
                for _ in 0..argc {
                    values.push(stack.pop().ok_or(VmError::StackUnderflow)?);
                }
                values.reverse();
                *self = Self::Heap(values);
                Ok(())
            }
        }
    }

    /// 构造适合 `argc` 的空实参暂存区。
    #[must_use]
    pub(crate) fn with_capacity(argc: usize) -> Self {
        if argc <= 8 {
            Self::Inline {
                values: [Value::Undefined; 8],
                len: 0,
            }
        } else {
            Self::Heap(Vec::with_capacity(argc))
        }
    }

    /// 返回按调用顺序排列的参数切片。
    #[must_use]
    pub(crate) fn as_slice(&self) -> &[Value] {
        match self {
            Self::Inline { values, len } => &values[..*len],
            Self::Heap(values) => values,
        }
    }
}

impl Vm {
    /// 解析可调用对象：闭包返回 (函数模板索引, 上值)；裸函数模板索引返回 (索引, 空上值)。
    pub(crate) fn resolve_callable(&self, callee: Value) -> (Option<usize>, Vec<Upvalue>) {
        if let Value::Object(r) = callee {
            if let Some(HeapObject::Closure {
                func_idx, upvalues, ..
            }) = self.heap.get(r.0 as usize)
            {
                return (Some(*func_idx), upvalues.clone());
            }
            if (r.0 as usize) < self.module_functions.len() {
                return (Some(r.0 as usize), Vec::new());
            }
        }
        (None, Vec::new())
    }

    /// 调用可调用对象；不可解析时返回 `undefined`（对齐现有 `CALL` 臂语义）。
    pub(crate) fn invoke_callable(
        &mut self,
        callee: Value,
        this_val: Value,
        args: &[Value],
    ) -> Result<Value, VmError> {
        if let Value::Object(r) = callee {
            // Proxy 对象：经 apply trap 派发（未安装时转发 target 调用）
            if self.proxy_parts(r).is_some() {
                return self.proxy_apply(r, this_val, args);
            }
            let resolver = match self.heap.get(r.0 as usize) {
                Some(HeapObject::PromiseResolver { promise, resolve }) => {
                    Some((*promise, *resolve))
                }
                _ => None,
            };
            if let Some((promise, resolve)) = resolver {
                let val = match args.first() {
                    Some(v) if !matches!(v, Value::Undefined) => *v,
                    _ => {
                        crate::builtins::timers::take_resolver_val(r.0).unwrap_or(Value::Undefined)
                    }
                };
                if resolve {
                    self.fulfill_promise(promise, val)?;
                } else {
                    self.reject_promise(promise, val)?;
                }
                return Ok(Value::Undefined);
            }
            // `RegExp(pat)` 无 new 直调等价 `new RegExp(pat)`（JS 语义）
            let ctor_name = match self.heap.get(r.0 as usize) {
                Some(HeapObject::NativeCtor { name, .. }) => Some(name.clone()),
                _ => None,
            };
            if ctor_name.as_deref() == Some("RegExp") {
                return self.construct_regexp(args);
            }
            // `Proxy(t, h)` 可调用形态等价 `new Proxy(t, h)`（规范 [[Call]] 拦截）
            if ctor_name.as_deref() == Some("Proxy") {
                return self.construct_proxy(args);
            }
            // Number(value)/Boolean(value)：转换为原始值（不加 new 语义）
            if ctor_name.as_deref() == Some("Number") {
                return Ok(Value::Number(self.to_number_value(
                    args.first().copied().unwrap_or(Value::Undefined),
                )));
            }
            if ctor_name.as_deref() == Some("Boolean") {
                return Ok(Value::Boolean(
                    args.first()
                        .copied()
                        .unwrap_or(Value::Undefined)
                        .is_truthy(),
                ));
            }
            // String(value)：无 new 直调 = 字符串化（真实包顶层大量
            // `String(x)` 形态，如 depd 的 containsNamespace）
            if ctor_name.as_deref() == Some("String") {
                let v = args.first().copied().unwrap_or(Value::Undefined);
                let s = self.alloc_string(self.format_value(v));
                return Ok(Value::Object(s));
            }
            // Symbol([description])：无 new 直调 = 创建符号
            if ctor_name.as_deref() == Some("Symbol") {
                return Ok(self.symbol_create(args));
            }
            // Date(value) 无 new 直调等价 new Date(value)
            if ctor_name.as_deref() == Some("Date") {
                return self.construct_date(args);
            }
            // Error 族无 new 直调等价 new（TypeError('msg') 常见形态）
            if matches!(
                ctor_name.as_deref(),
                Some(
                    "Error"
                        | "TypeError"
                        | "RangeError"
                        | "SyntaxError"
                        | "ReferenceError"
                        | "EvalError"
                        | "URIError"
                )
            ) {
                return self.do_construct(callee, args);
            }
            // eval / Function 动态求值拦截（直接/间接形态与动态函数模板；
            // Function 构造器为 NativeCtor 单例后同样命中——无 new 直调
            // `Function("return 8")` 语义等价 new）
            if let Some(HeapObject::NativeFn { name, .. }) = self.heap.get(r.0 as usize) {
                if name == "eval" || name == "eval.direct" {
                    return self.call_eval(name == "eval.direct", args);
                }
            }
            if ctor_name.as_deref() == Some("Function") {
                return self.construct_function(args);
            }
            // `revoke()`：捕获的撤销闭包面（自有属性 `_revokes` 存 proxy 句柄；
            // 处理器签名无法拿到自身 fn 对象，故在此特判）
            if let Some(HeapObject::NativeFn { name, .. }) = self.heap.get(r.0 as usize) {
                if name == "Proxy.revoke" {
                    if let Some(Value::Object(pr)) = self.get_native_fn_property(r, "_revokes") {
                        self.revoke_proxy(pr);
                    }
                    return Ok(Value::Undefined);
                }
            }
            let handler = match self.heap.get(r.0 as usize) {
                Some(HeapObject::NativeFn { name, .. }) => {
                    crate::builtins::set_pending_native_name(name);
                    self.builtin_registry.lookup(name)
                }
                _ => None,
            };
            if let Some(handler) = handler {
                crate::builtins::set_current_receiver(this_val);
                crate::builtins::set_pending_callee(callee);
                return handler(self, args);
            }
        }
        let (f_idx, uvs) = self.resolve_callable(callee);
        if let Some(fi) = f_idx {
            return self.invoke_function(fi, this_val, args, uvs);
        }
        // 调用不可调用值：JS 语义抛 TypeError（此前静默返回 undefined，
        // 掩盖真实缺陷）
        let desc = self.format_value(callee);
        let err = self.alloc_error_instance(&format!("{desc} is not a function"));
        let name = self.alloc_string("TypeError".to_owned());
        let _ = self.set_property(Value::Object(err), "name", Value::Object(name));
        Err(VmError::Thrown(Value::Object(err)))
    }

    /// 构造调用（`new X(args)`）：分配实例（挂 `callee.prototype`）并以 `this`=实例
    /// 调用构造器；构造器返回对象则采用之，否则采用实例。原生构造器由解释器拦截。
    pub(crate) fn do_construct(&mut self, callee: Value, args: &[Value]) -> Result<Value, VmError> {
        if let Value::Object(r) = callee {
            // Proxy 对象：经 construct trap 派发（未安装时转发 target 构造）
            if self.proxy_parts(r).is_some() {
                return self.proxy_construct(r, args);
            }
            let ctor_name = match self.heap.get(r.0 as usize) {
                Some(HeapObject::NativeCtor { name, .. }) => Some(name.clone()),
                Some(HeapObject::NativeFn { name, .. }) => Some(name.clone()),
                _ => None,
            };
            if let Some(ref name) = ctor_name {
                match name.as_str() {
                    "Error" | "TypeError" | "RangeError" | "SyntaxError" | "ReferenceError"
                    | "EvalError" | "URIError" => {
                        // message 未传或为 undefined 时按规范置空串；
                        // 子类实例 name 置子类名（对齐 Node：e.name === 'TypeError'）
                        let message = match args.first() {
                            Some(Value::Undefined) | None => String::new(),
                            Some(v) => self.format_value(*v),
                        };
                        let err = self.alloc_error_instance(&message);
                        if name != "Error" {
                            let n = self.alloc_string(name.clone());
                            let _ = self.set_property(Value::Object(err), "name", Value::Object(n));
                        }
                        return Ok(Value::Object(err));
                    }
                    "Promise" => {
                        // new Promise(executor)：创建 pending promise，以
                        // (resolve, reject) 解析器对调用执行器；执行器同步抛错
                        // 则该 promise 以异常拒绝（JS 语义）
                        let promise = self.alloc_pending_promise();
                        let resolve = self.alloc_promise_resolver(promise, true);
                        let reject = self.alloc_promise_resolver(promise, false);
                        let executor = args.first().copied().unwrap_or(Value::Undefined);
                        let (f_idx, uvs) = self.resolve_callable(executor);
                        let call_ret = match f_idx {
                            Some(fi) => self.invoke_function(
                                fi,
                                Value::Undefined,
                                &[Value::Object(resolve), Value::Object(reject)],
                                uvs,
                            ),
                            None => Ok(Value::Undefined),
                        };
                        if let Err(VmError::Thrown(exc)) = call_ret {
                            self.reject_promise(promise, exc)?;
                        }
                        return Ok(Value::Object(promise));
                    }
                    "Array" => return Ok(Value::Object(self.alloc_array(Vec::new()))),
                    "Object" => return Ok(Value::Object(self.alloc_ordinary())),
                    "RegExp" => return self.construct_regexp(args),
                    "Map" => return Ok(Value::Object(self.alloc_map(Vec::new()))),
                    "Set" => {
                        // new Set(iterable?)：元素按 to_property_key 去重，
                        // 值保留原元素（size/has 语义）
                        let mut entries: Vec<(String, Value)> = Vec::new();
                        if let Some(Value::Object(r)) = args.first().copied() {
                            if let Some(HeapObject::Array { elements, .. }) =
                                self.heap.get(r.0 as usize)
                            {
                                for elem in elements.clone() {
                                    let key = self.to_property_key(elem);
                                    if !entries.iter().any(|(k, _)| *k == key) {
                                        entries.push((key, elem));
                                    }
                                }
                            }
                        }
                        return Ok(Value::Object(self.alloc_map(entries)));
                    }
                    "URL" => return Ok(self.url_constructor(args)),
                    "Proxy" => return self.construct_proxy(args),
                    "Function" => return self.construct_function(args),
                    "Number" => {
                        return Ok(Value::Number(self.to_number_value(
                            args.first().copied().unwrap_or(Value::Undefined),
                        )));
                    }
                    "Boolean" => {
                        return Ok(Value::Boolean(
                            args.first()
                                .copied()
                                .unwrap_or(Value::Undefined)
                                .is_truthy(),
                        ));
                    }
                    "Date" => return self.construct_date(args),
                    "ArrayBuffer" => return self.construct_array_buffer(args, false),
                    "SharedArrayBuffer" => return self.construct_array_buffer(args, true),
                    "DataView" => return self.construct_data_view(args),
                    _ if crate::typed_array::TypedKind::by_ctor_name(name.as_str()).is_some() => {
                        let kind = crate::typed_array::TypedKind::by_ctor_name(name.as_str())
                            .expect("上方已确认命中");
                        return self.construct_typed_array(kind, args);
                    }
                    _ => {}
                }
                if let Some(handler) = self.builtin_registry.lookup(name) {
                    return handler(self, args);
                }
            }
        }
        let proto_ref = match self.get_property(callee, "prototype") {
            Ok(Value::Object(p)) => Some(p),
            _ => None,
        };
        let instance_ref = self.alloc_ordinary_with_proto(proto_ref);
        let instance_val = Value::Object(instance_ref);
        let (f_idx, uvs) = self.resolve_callable(callee);
        if let Some(fi) = f_idx {
            let res = self.invoke_function(fi, instance_val, args, uvs)?;
            if matches!(res, Value::Object(_)) {
                return Ok(res);
            }
        }
        Ok(instance_val)
    }

    /// `super(args)` 语义：在当前帧 `this` 槽（`locals[0]`，即派生实例）上调用父类构造器。
    pub(crate) fn do_construct_this(
        &mut self,
        callee: Value,
        args: &[Value],
    ) -> Result<Value, VmError> {
        let this_val = *self.locals.first().unwrap_or(&Value::Undefined);
        let (f_idx, uvs) = self.resolve_callable(callee);
        if let Some(fi) = f_idx {
            return self.invoke_function(fi, this_val, args, uvs);
        }
        Ok(this_val)
    }

    /// 将 spread 参数表转为参数列表（对齐 Go 版 `toArrayValues`）：
    /// 数组取元素列表，普通对象取自有属性值集，其余为空。
    pub(crate) fn to_array_values(&self, val: Value) -> Vec<Value> {
        if let Value::Object(r) = val {
            let idx = r.0 as usize;
            if idx < self.heap.len() {
                match &self.heap[idx] {
                    HeapObject::Array { elements, .. } => return elements.clone(),
                    HeapObject::Ordinary { .. } => {
                        return self.own_entries(idx).into_iter().map(|(_, v)| v).collect();
                    }
                    _ => {}
                }
            }
        }
        Vec::new()
    }

    /// 绑定调用实参到当前帧局部槽位（`self.locals` 须已初始化为全 undefined）。
    ///
    /// 对齐 Go 版：固定参数位只绑前 `num_params` 个；varargs 函数把多余实参
    /// 打包成 rest 数组写在 `locals[1 + num_params]`（不足为空数组）。
    pub(crate) fn bind_call_args(
        &mut self,
        this_val: Value,
        args: &[Value],
        num_params: usize,
        is_var_args: bool,
    ) {
        if !self.locals.is_empty() {
            self.locals[0] = this_val;
        }
        for (i, arg) in args.iter().take(num_params).enumerate() {
            let slot = i + 1; // locals[0] 是 this
            if slot < self.locals.len() {
                self.locals[slot] = *arg;
            }
        }
        if is_var_args {
            let rest: Vec<Value> = if args.len() > num_params {
                args[num_params..].to_vec()
            } else {
                Vec::new()
            };
            let rest_ref = self.alloc_array(rest);
            let rest_slot = 1 + num_params;
            if rest_slot < self.locals.len() {
                self.locals[rest_slot] = Value::Object(rest_ref);
            }
        }
    }

    /// 执行函数模板（自动隔离并保存/恢复局部槽位、上值环境与当前常量池）。
    ///
    /// 生成器函数**不执行函数体**：仅创建生成器对象并登记初始状态（JS 语义，
    /// 由 `next()` 驱动）；async 函数同步执行函数体并把结果包装为 fulfilled Promise。
    pub fn invoke_function(
        &mut self,
        func_idx: usize,
        this_val: Value,
        args: &[Value],
        upvalues: Vec<Upvalue>,
    ) -> Result<Value, VmError> {
        if func_idx >= self.module_functions.len() {
            return Ok(Value::Undefined);
        }
        let tmpl = self.module_functions[func_idx].clone();
        if tmpl.is_generator {
            return Ok(self.make_generator(&tmpl, func_idx, this_val, args, upvalues));
        }
        // Tier 1 热点：达阈值且资格符合 → 直接执行机器码（无 OSR，函数入口切换）
        if self.jit_enabled {
            if let Some(jit) = self.jit_lookup(func_idx) {
                return Ok(self.jit_run(func_idx, &jit, args, tmpl.num_params as usize, upvalues));
            }
        }
        // 调用链登记：进入解释帧推入，任何退出路径（含 ?）由 FrameGuard 弹出
        CALL_CHAIN.with(|c| c.borrow_mut().push((func_idx, tmpl.name.clone())));
        let _frame_guard = FrameGuard;
        let old_func_idx = std::mem::replace(&mut self.current_func_idx, func_idx as i64);
        let old_constants = std::mem::replace(
            &mut self.current_constants,
            self.module_constants[func_idx].clone(),
        );
        let old_try_table = std::mem::replace(&mut self.current_try_table, tmpl.try_table.clone());
        // 换出的外层帧状态登记进保存帧寄存器（GC 根集合成员）：嵌套执行
        // 触发回收时这些对象必须保持存活，否则恢复帧会读到悬垂复用引用
        let frame_slot = self.gc_saved_frames.len();
        self.gc_saved_frames.push(crate::gc::SavedFrameState {
            locals: std::mem::replace(
                &mut self.locals,
                vec![Value::Undefined; tmpl.num_locals as usize],
            ),
            upvalues: std::mem::replace(&mut self.current_upvalues, upvalues),
            open_upvalues: std::mem::take(&mut self.open_upvalues)
                .into_iter()
                .collect(),
            try_stack: std::mem::take(&mut self.try_stack),
        });
        // 本帧逻辑栈分界（AWAIT 挂起时收割本帧逻辑栈用）
        let frame_base = self.stack.len();

        self.bind_call_args(this_val, args, tmpl.num_params as usize, tmpl.is_var_args);
        // `arguments` 对象注入（对齐 Go：编译器给出槽位 + 未引用标记；
        // 仅对引用 arguments 的函数构建，性能零影响）
        if let Some(extras) = self.module_header_extras.get(func_idx) {
            if extras.arguments_slot >= 0 && !extras.no_arguments_object {
                let slot = extras.arguments_slot as usize;
                if slot < self.locals.len() {
                    let args_arr = self.alloc_array(args.to_vec());
                    self.locals[slot] = Value::Object(args_arr);
                }
            }
        }

        let ret =
            self.run_with_constants_rc(&tmpl.code, self.module_constants[func_idx].clone(), 0);

        // 恢复换出帧（弹保存帧寄存器；嵌套调用对称出入栈）
        let saved_frame = self.gc_saved_frames.pop().unwrap_or_default();
        debug_assert_eq!(frame_slot, self.gc_saved_frames.len());

        // async 函数遇未完成 Promise：**在恢复调用者之前**收割本帧
        //（否则收割到的是调用者上下文——挂起语义的帧归属错误）
        if let Err(VmError::Awaited(awaited_promise)) = &ret {
            if tmpl.is_async {
                let gen_stack = self.stack.split_off(frame_base);
                let frame = crate::generator::SuspendedFrame {
                    pc: self.yield_pc,
                    locals: std::mem::take(&mut self.locals),
                    stack: gen_stack,
                    upvalues: std::mem::take(&mut self.current_upvalues),
                    open_upvalues: std::mem::take(&mut self.open_upvalues),
                    try_stack: std::mem::take(&mut self.try_stack),
                };
                self.current_constants = old_constants.clone();
                self.current_try_table = old_try_table;
                self.current_func_idx = old_func_idx;
                self.locals = saved_frame.locals;
                self.current_upvalues = saved_frame.upvalues;
                self.open_upvalues = saved_frame.open_upvalues.into_iter().collect();
                self.try_stack = saved_frame.try_stack;
                let p_obj = self.alloc_pending_promise();
                self.promise_resumes.insert(
                    awaited_promise.index() as u32,
                    crate::builtins::PendingResume {
                        frame,
                        func_idx,
                        promise: p_obj,
                        awaited: *awaited_promise,
                    },
                );
                return Ok(Value::Object(p_obj));
            }
        }

        // 本帧逻辑栈收割：嵌套调用/异常路径可能在栈上残留本帧垃圾值，
        // 返回前按 frame_base 截断（共享栈模型——残留会污染调用者栈序，
        // M2.4 实测：GetIntrinsic 内层 stringToPath 的数组元素残留致
        // call-bound 的 callBindBasic 调用 callee 错位）
        self.stack.truncate(frame_base);

        // 正常路径：函数返回前，关闭当前帧所有未关闭的 open upvalues
        for (slot, uv) in &self.open_upvalues {
            if let Some(val) = self.locals.get(*slot) {
                *uv.0.borrow_mut() = *val;
            }
        }

        self.locals = saved_frame.locals;
        self.current_constants = old_constants.clone();
        self.current_upvalues = saved_frame.upvalues;
        self.open_upvalues = saved_frame.open_upvalues.into_iter().collect();
        for (slot, uv) in &self.open_upvalues {
            if let Some(loc) = self.locals.get_mut(*slot) {
                *loc = *uv.0.borrow();
            }
        }
        self.try_stack = saved_frame.try_stack;
        self.current_try_table = old_try_table;
        self.current_func_idx = old_func_idx;
        if std::env::var("ALUKA_ERR_TRACE").is_ok() {
            if let Err(VmError::Thrown(_)) = &ret {
                eprintln!(
                    "[err-trace] 穿过函数 func_idx={func_idx} name={} src={} pc={}",
                    tmpl.name, tmpl.source_file, self.last_pc
                );
            }
        }
        match ret {
            // async 函数（同步完成）：结果包装为 fulfilled Promise
            Ok(v) if tmpl.is_async => {
                let p = self.alloc_fulfilled_promise(v);
                Ok(Value::Object(p))
            }
            other => other,
        }
    }

    /// 预加载模块的函数扩展标量头（`arguments` 槽位等）。
    ///
    /// 需要**原始字节码**（`run_module` 只持反序列化对象），由 CLI/加载器
    /// 在 `run_module` 前调用；未调用时 [`Vm::module_header_extras`] 为空，
    /// `arguments` 注入自动跳过（向后兼容）。
    pub fn load_module(
        &mut self,
        data: &[u8],
        module: &aluka_bytecode::BytecodeModule,
    ) -> Result<(), aluka_bytecode::VerifyError> {
        self.module_header_extras =
            aluka_bytecode::read_all_func_header_extras(data, module.functions.len())?;
        Ok(())
    }

    /// 执行函数模板（自动根据常量池和局部变量槽位初始化执行环境）。
    ///
    /// 常量池与 Try 表同样按帧隔离——调用方（如 CJS 模块加载）嵌套运行
    /// 其他模块后，本帧的常量池解引用不受污染。
    pub fn run_func(&mut self, func: &aluka_bytecode::FuncTemplate) -> Result<Value, VmError> {
        let func = std::rc::Rc::new(func.clone());
        let constants = std::rc::Rc::new(func.constants.clone());
        let old_constants = std::mem::replace(&mut self.current_constants, constants.clone());
        let old_try_table = std::mem::replace(&mut self.current_try_table, func.try_table.clone());
        // 模块 main 帧与 invoke_function 帧协议一致：换出 locals **与打开上值
        // 表**。嵌套模块体执行期间若让外层表留在 open_upvalues，其 MAKE_CLOSURE
        // 以同 slot 捕获会命中外层 cell（entry(slot) 复用），STORE_LOCAL 快路径
        // 把外层 cell 污染成内层闭包——恢复时回写即覆写外层 locals（M2.4 实测
        // http-errors slot7 被 depd 的 eehaslisteners 覆写即此路径）。两者同时
        // 登记保存帧寄存器（GC 根集合成员）。
        self.gc_saved_frames.push(crate::gc::SavedFrameState {
            locals: std::mem::replace(
                &mut self.locals,
                vec![Value::Undefined; func.num_locals as usize],
            ),
            open_upvalues: std::mem::take(&mut self.open_upvalues)
                .into_iter()
                .collect(),
            ..Default::default()
        });
        let res = self.run_with_constants_rc(&func.code, constants, 0);
        let saved_frame = self.gc_saved_frames.pop().unwrap_or_default();
        self.locals = saved_frame.locals;
        self.open_upvalues = saved_frame.open_upvalues.into_iter().collect();
        // 打开上值内容同步回外层 locals（与 invoke_function 恢复语义一致）
        for (slot, uv) in &self.open_upvalues {
            if let Some(loc) = self.locals.get_mut(*slot) {
                *loc = *uv.0.borrow();
            }
        }
        self.current_constants = old_constants;
        self.current_try_table = old_try_table;
        if std::env::var("ALUKA_ERR_TRACE").is_ok() {
            if let Err(VmError::Thrown(_)) = &res {
                eprintln!(
                    "[err-trace] 穿过函数 func_idx={} name={} pc={}",
                    self.current_func_idx, func.name, self.last_pc
                );
            }
        }
        res
    }

    /// 仅装载模块函数表/常量池（不执行；测试与热点挂接验证用）。
    pub fn load_module_for_test(&mut self, module: &aluka_bytecode::BytecodeModule) {
        self.module_functions = module
            .functions
            .iter()
            .map(|f| std::rc::Rc::new(f.clone()))
            .collect();
        self.module_constants = self
            .module_functions
            .iter()
            .map(|f| std::rc::Rc::new(f.constants.clone()))
            .collect();
        self.module_classes = module.classes.clone();
        self.jit_reset();
    }

    /// 执行编译模块：按顺序执行顶层及入口闭包。
    pub fn run_module(
        &mut self,
        module: &aluka_bytecode::BytecodeModule,
    ) -> Result<Value, VmError> {
        if module.functions.is_empty() {
            return Ok(Value::Undefined);
        }
        self.module_functions = module
            .functions
            .iter()
            .map(|f| std::rc::Rc::new(f.clone()))
            .collect();
        self.module_constants = self
            .module_functions
            .iter()
            .map(|f| std::rc::Rc::new(f.constants.clone()))
            .collect();
        self.module_classes = module.classes.clone();
        // 模块替换：函数索引语义改变，JIT 缓存必须清空
        self.jit_reset();
        // 先执行 Func 0（主函数）
        let res = self.run_func(&self.module_functions[0].clone())?;

        let mut ret = res;
        if let Value::Object(r) = res {
            let (target_func, uvs) = if let Some(HeapObject::Closure {
                func_idx, upvalues, ..
            }) = self.heap.get(r.0 as usize)
            {
                (Some(*func_idx), upvalues.clone())
            } else if (r.0 as usize) < module.functions.len() {
                (Some(r.0 as usize), Vec::new())
            } else {
                (None, Vec::new())
            };
            if let Some(fi) = target_func {
                if fi < module.functions.len() {
                    // CJS 上下文（`setup_cjs` 后）：入口返回的闭包是 Go 版 CJS
                    // 包装函数，按 7 参签名 `(require, module, exports,
                    // __filename, __dirname, __import, __importMeta)` 调用；
                    // 非 CJS 场景维持无参调用（golden 语料零回归）。
                    ret = if self.require_fn.is_some() {
                        self.invoke_cjs_entry(fi, uvs)?
                    } else {
                        self.invoke_function(fi, Value::Undefined, &[], uvs)?
                    };
                }
            }
        }
        // 顶层收口：事件循环——微任务（nextTick/Promise）与宏任务（定时器）
        // 交替排空直到两者皆空（宏任务兑现 Promise 会追加微任务/恢复挂起帧；
        // 活跃内置库事件源在宏任务排空末尾泵询，有进展则继续循环）
        let loop_started = std::time::Instant::now();
        loop {
            self.drain_microtasks()?;
            if self.macro_tasks.is_empty() && !self.has_active_event_sources() {
                break;
            }
            let progressed = self.drain_macro_tasks()?;
            if self.macro_tasks.is_empty() && !progressed {
                // 事件源活跃但本轮无进展：稍候再泵（泵实现内部可阻塞等待）；
                // 超时保护防止事件永不达成时挂死
                if loop_started.elapsed() > std::time::Duration::from_secs(120) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        Ok(ret)
    }

    /// 以 CJS wrapper 签名调用入口闭包（`require/module/exports/…` 七参），
    /// 返回 `module.exports` 的最终值。
    fn invoke_cjs_entry(
        &mut self,
        func_idx: usize,
        upvalues: Vec<crate::value::Upvalue>,
    ) -> Result<Value, VmError> {
        let exports = Value::Object(self.alloc_ordinary());
        let module_obj = Value::Object(self.alloc_ordinary());
        self.set_property(module_obj, "exports", exports)?;
        let require_fn = self
            .require_fn
            .unwrap_or_else(|| self.alloc_native_fn("require"));
        let filename = Value::Object(self.alloc_string(self.entry_file.clone()));
        let dirname = Value::Object(
            self.alloc_string(
                self.base_dir
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
            ),
        );
        let ret = self.invoke_function(
            func_idx,
            Value::Undefined,
            &[
                Value::Object(require_fn),
                module_obj,
                exports,
                filename,
                dirname,
                Value::Undefined, // __import
                Value::Undefined, // __importMeta
            ],
            upvalues,
        )?;
        let _ = ret;
        // 模块可能重赋值 module.exports：以最终值为准
        self.get_property(module_obj, "exports")
    }
}
