//! 函数调用管理、帧上下文隔离与模块执行入口。

use crate::heap::{HeapObject, OrdinaryProps};
use crate::interpreter::{Vm, VmError};
use crate::value::{Upvalue, Value, ValueCase};
use std::cell::RefCell;

thread_local! {
    /// 轻量 JS 调用链（诊断 + 错误 `stack` 生成）：(func_idx, 模板名)，
    /// `invoke_function` 进入时推入、退出时弹出。
    ///
    /// 模板名用 `Rc<str>`：调用链在**每次函数调用**上维护，若用 `String` 则
    /// 每次调用都要克隆函数名（堆分配），热路径代价不可忽略。
    static CALL_CHAIN: RefCell<Vec<(usize, std::rc::Rc<str>)>> = const { RefCell::new(Vec::new()) };
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

/// 当前 JS 调用链快照（由内向外：首元素为**最内层**帧）——错误 `stack` 生成用。
///
/// 元素为 `(func_idx, 模板名)`；无活跃帧（模块顶层）时返回空表。
#[must_use]
pub fn call_chain_snapshot() -> Vec<(usize, std::rc::Rc<str>)> {
    CALL_CHAIN.with(|c| c.borrow().clone())
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
    /// 解析可调用对象：仅**闭包**携带函数模板索引与上值。
    ///
    /// 此前这里还有一条「裸函数模板索引」回退——只要对象句柄的堆索引小于当前
    /// 函数表长度就当作模板索引。该约定在 Rust VM 中**没有任何创建者**（模板
    /// 一律经 `alloc_closure*` 包装），却会劫持索引较小的真实对象：函数表随
    /// `require` 追加而增长，一旦 `table_len > 某内建对象的堆索引`，该对象就被
    /// 误解析成函数模板并执行**另一个函数**（实测：`AppError` 构造器里的
    /// `super(m)` 以 `Error`（NativeCtor，堆索引 55 < 表长 64）为 callee，被
    /// 解析成 `module_functions[55]` = `TaskService_remove` 并执行其函数体）。
    /// 这也是缺陷「顺序/布局敏感」的成因：表长随已加载模块数变化。
    pub(crate) fn resolve_callable(&self, callee: Value) -> (Option<usize>, Vec<Upvalue>) {
        if let Some(r) = callee.as_object()
            && let Some(HeapObject::Closure {
                func_idx, upvalues, ..
            }) = self.heap.get(r.0 as usize)
        {
            return (Some(*func_idx), upvalues.clone());
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
        if let Some(r) = callee.as_object() {
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
                    Some(v) if !v.is_undefined() => *v,
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
            if std::env::var("ALUKA_CALL_DEBUG").is_ok() {
                let nm = match self.heap.get(r.0 as usize) {
                    Some(HeapObject::NativeFn { name, .. })
                    | Some(HeapObject::NativeCtor { name, .. }) => name.clone(),
                    _ => "<non-native>".to_owned(),
                };
                eprintln!("[call-dbg] invoke_callable receiver-fn name={nm}");
            }
            if ctor_name.as_deref() == Some("RegExp") {
                return self.construct_regexp(args);
            }
            // `Proxy(t, h)` 可调用形态等价 `new Proxy(t, h)`（规范 [[Call]] 拦截）
            if ctor_name.as_deref() == Some("Proxy") {
                return self.construct_proxy(args);
            }
            // `Array(...)` 无 new 直调等价 `new Array(...)`（ES §23.1.1）
            if ctor_name.as_deref() == Some("Array") {
                return self.do_construct(callee, args);
            }
            // `new JSON()`：JSON 是普通命名空间对象而非构造器 → TypeError
            if matches!(ctor_name.as_deref(), Some("JSON")) {
                return Err(self.type_error("JSON is not a constructor"));
            }
            // Number(value)：无 new 直调 = ToNumeric 全语义（对象经 ToPrimitive
            // hint number、wrapper 槽直解；符号/无可原始化 → TypeError；
            // 无参 → +0）。此前用 to_number_value（&self）不做 ToPrimitive，
            // `Number({valueOf:()=>1})` 得 NaN（S8.12.8 / S9.1 族）
            if ctor_name.as_deref() == Some("Number") {
                let v = match args.first() {
                    None => 0.0,
                    // 规范 Number(value)：ToNumeric 中 **BigInt 走 ToNumber
                    // 特例**（`Number(1n) === 1`）——与一元 `+1n` 抛 TypeError
                    // 不同，本次为显式转换故允许
                    Some(v) if self.is_bigint_value(*v) => self.to_number_value(*v),
                    Some(v) => self.numeric_operand(*v)?,
                };
                return Ok(Value::Number(v));
            }
            if ctor_name.as_deref() == Some("Boolean") {
                return Ok(Value::Boolean(
                    self.truthy(args.first().copied().unwrap_or(Value::Undefined)),
                ));
            }
            // String(value)：无 new 直调 = 字符串化（真实包顶层大量
            // `String(x)` 形态，如 depd 的 containsNamespace）；无参按规范 ""
            if ctor_name.as_deref() == Some("String") {
                // ToString 全语义（用户自定义 toString 生效；符号 → TypeError）
                let s = match args.first() {
                    None => String::new(),
                    Some(v) => {
                        // 符号特例（SymbolDescriptiveString）；其余严格 ToString
                        if self.is_symbol(*v) {
                            self.format_value(*v)
                        } else {
                            self.js_string_strict(*v)?
                        }
                    }
                };
                let s = self.alloc_string(s);
                return Ok(Value::Object(s));
            }
            // Symbol([description])：无 new 直调 = 创建符号
            if ctor_name.as_deref() == Some("Symbol") {
                return self.symbol_create(args);
            }
            // BigInt(value)：无 new 直调 = 转 BigInt（数字须为整数，
            // 否则 RangeError；字符串按十进制/进制前缀解析；S20.2.1 族）
            if ctor_name.as_deref() == Some("BigInt") {
                let v = args.first().copied().unwrap_or(Value::Undefined);
                return self.bigint_from_value(v);
            }
            // Date(value)：无 new 直调 → **当前时间的可读字符串**（Annex B：
            // 函数调用形态忽略参数，`typeof Date() === "string"`；
            // `new Date(v)` 才是对象。此前带参误等价 new Date(v) 得对象）
            if ctor_name.as_deref() == Some("Date") {
                let now = self.construct_date(&[]);
                return match now {
                    Ok(d) => {
                        let s = self.get_property(d, "toString")?;
                        let t = self.invoke_callable(s, d, &[])?;
                        let text = match self
                            .heap
                            .get(t.as_object().map(|r| r.index()).unwrap_or(usize::MAX))
                        {
                            Some(HeapObject::String(str_val)) => str_val.clone(),
                            _ => self.format_value(t),
                        };
                        Ok(Value::Object(self.alloc_string(text)))
                    }
                    Err(e) => Err(e),
                };
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
            // Object()/Array() 无 new 直调语义等价 new（M7.2 修复：此前
            // `Object()` 报 [function Function] is not a function——S15.2.1.1 族）
            if matches!(ctor_name.as_deref(), Some("Object") | Some("Array")) {
                return self.do_construct(callee, args);
            }
            // `Buffer(arg[, enc[, len]])` 无 new 直调（Node 的 Buffer 是普通
            // 函数，safe-buffer 的 SafeBuffer 就靠裸调用转发；此前报
            // [function Function] is not a function，致 express 的
            // `Buffer.from` 回退路径瘫痪）
            if ctor_name.as_deref() == Some("Buffer") {
                return crate::builtins::buffer::buffer_construct(self, args, callee);
            }
            // `revoke()`：捕获的撤销闭包面（自有属性 `_revokes` 存 proxy 句柄；
            // 处理器签名无法拿到自身 fn 对象，故在此特判）
            if let Some(HeapObject::NativeFn { name, .. }) = self.heap.get(r.0 as usize) {
                if name == "Proxy.revoke" {
                    if let Some(pr) = self
                        .get_native_fn_property(r, "_revokes")
                        .and_then(|v| v.as_object())
                    {
                        self.revoke_proxy(pr);
                    }
                    return Ok(Value::Undefined);
                }
            }
            let handler = match self.heap.get(r.0 as usize) {
                Some(HeapObject::NativeFn { name, .. }) => {
                    // `Math.<m>`：Math 方法未注册到分派表（走 CALL_METHOD
                    // 硬编码单源求值），故经 invoke_callable 的间接调用
                    //（`Reflect.apply(Math.max, null, [1,2])` /
                    // `Math.max.call(...)`）此前报 "is not a function"。
                    // 此处按前缀名直接求值，与 CALL_METHOD 同源。
                    if let Some(m) = name.strip_prefix("Math.") {
                        crate::builtins::set_current_receiver(this_val);
                        crate::builtins::set_pending_callee(callee);
                        return Ok(crate::interpreter::math_method(m, args));
                    }
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
        self.attach_error_proto(err, "TypeError");
        Err(VmError::Thrown(Value::Object(err)))
    }

    /// 构造调用（`new X(args)`）：分配实例（挂 `callee.prototype`）并以 `this`=实例
    /// 调用构造器；构造器返回对象则采用之，否则采用实例。原生构造器由解释器拦截。
    pub(crate) fn do_construct(&mut self, callee: Value, args: &[Value]) -> Result<Value, VmError> {
        if let Some(r) = callee.as_object() {
            // Proxy 对象：经 construct trap 派发（未安装时转发 target 构造）
            if self.proxy_parts(r).is_some() {
                return self.proxy_construct(r, args);
            }
            // JSON 命名空间对象（普通 Ordinary + `_isJSON` 标记）：不可 new
            if self.has_own_slot(r.0 as usize, "_isJSON") {
                return Err(self.type_error("JSON is not a constructor"));
            }
            // 符号包装实例（`Object(Symbol())` 产物）：不可作为构造器
            //（S19.4.3 族——`new Object(Symbol())` 的实例再 new 抛 TypeError）
            if self.has_own_slot(r.0 as usize, "[[SymbolData]]") {
                return Err(self.type_error("Symbol is not a constructor"));
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
                        let (message, has_arg) = match args.first() {
                            None => (String::new(), false),
                            Some(v) if v.is_undefined() => (String::new(), false),
                            Some(v) => (self.format_value(*v), true),
                        };
                        // 规范：未传 message 时**不落自有 message 槽**
                        //（`new Error()` 的 Object.getOwnPropertyNames 只需 stack；
                        //  `new Error('')` 才带自有 message）——Node 实测口径
                        self.last_error_message = has_arg.then(|| message.clone());
                        let err = if has_arg {
                            self.alloc_error_instance(&message)
                        } else {
                            self.alloc_error_instance_no_message()
                        };
                        if name != "Error" {
                            // 实例挂**独立**子类原型（instanceof TypeError 判
                            // 型；error_subclass_ctor 的 prototype.constructor
                            // 判定面配套——共享 Error.prototype 时代已终结）
                            // attach_error_proto 内部同步 stack 首行 + 收口可枚举性
                            self.attach_error_proto(err, name);
                        } else {
                            // 构造收尾：name/message/stack 一律不可枚举（Node）
                            self.refresh_error_enumerability(err);
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
                    "Array" => {
                        // new Array(...)（ES §23.1.1 Array 构造语义）：
                        // - 单数值参数 n → 稀疏数组（length=n，槽位 undefined）
                        // - 其余形态（无参/多参/单非数值）→ 参数即元素
                        // raw-body `new Array(arguments.length)` 依赖 length
                        // 语义——曾无条件空数组导致 done 回调参数全丢
                        if args.len() == 1
                            && let Some(n) = args[0].as_number()
                        {
                            if n.fract() == 0.0 && (0.0..4294967296.0).contains(&n) {
                                let len = n as usize;
                                // VM 数组为密集 Vec 表示（无稀疏字段），巨大
                                // length 会撑爆内存——与 Node 稀疏语义的差异点，
                                // 上限保护（真实包 length 均很小）
                                if len > 4_000_000 {
                                    let err = self.alloc_error_instance("Invalid array length");
                                    let name = self.alloc_string("RangeError".to_owned());
                                    let _ = self.set_property(
                                        Value::Object(err),
                                        "name",
                                        Value::Object(name),
                                    );
                                    return Err(VmError::Thrown(Value::Object(err)));
                                }
                                return Ok(Value::Object(
                                    self.alloc_array(vec![Value::Undefined; len]),
                                ));
                            }
                            // 非整数/负数/越界：RangeError（Node 语义）
                            let err = self.alloc_error_instance("Invalid array length");
                            let name = self.alloc_string("RangeError".to_owned());
                            let _ =
                                self.set_property(Value::Object(err), "name", Value::Object(name));
                            return Err(VmError::Thrown(Value::Object(err)));
                        }
                        return Ok(Value::Object(self.alloc_array(args.to_vec())));
                    }
                    // Symbol/BigInt 不可 new（规范 TypeError）
                    "Symbol" => {
                        return Err(self.type_error("Symbol is not a constructor"));
                    }
                    "BigInt" => {
                        return Err(self.type_error("BigInt is not a constructor"));
                    }
                    "Object" => {
                        // 规范：Object(v) 与 new Object(v) 同型——原始值造
                        // 包装实例（数据槽承载）；undefined/null → 空普通
                        // 对象；对象原样返回
                        let arg = args.first().copied().unwrap_or(Value::Undefined);
                        return match arg.case() {
                            ValueCase::Undefined | ValueCase::Null => {
                                Ok(Value::Object(self.alloc_ordinary()))
                            }
                            ValueCase::Object(r) => match self.heap.get(r.0 as usize) {
                                Some(HeapObject::String(_))
                                | Some(HeapObject::BigInt(_))
                                | Some(HeapObject::Symbol { .. }) => {
                                    Ok(Value::Object(self.alloc_primitive_wrapper(arg)))
                                }
                                _ => Ok(arg),
                            },
                            _ => Ok(Value::Object(self.alloc_primitive_wrapper(arg))),
                        };
                    }
                    "RegExp" => return self.construct_regexp(args),
                    "Map" => {
                        // new Map(iterable?)：逐项按 `[key, value]` 取键值
                        // （有序插入；Node 语义）；无参为空 Map。
                        // 接受**任意可迭代**（数组 / Map / Set / 四类内建迭代器 /
                        // 字符串 / 自定义 Symbol.iterator）——此前只识别数组，
                        // `new Map("ab")` 之类会静默得到空 Map。
                        // 键保留**原始 Value** + SameValueZero 去重：重复键不新增
                        // 条目、保持首次出现的位置、值取后者（Node 语义）。
                        let arg = args.first().copied().unwrap_or(Value::Undefined);
                        let items = if matches!(arg, Value::Undefined | Value::Null) {
                            Vec::new()
                        } else {
                            self.collect_iter_values(arg)?
                        };
                        let mut entries: Vec<(Value, Value)> = Vec::new();
                        for item in items {
                            let pair = self.to_array_values(item);
                            if pair.len() >= 2 {
                                let key = pair[0];
                                if let Some(slot) = entries
                                    .iter_mut()
                                    .find(|(k, _)| self.values_same_zero(*k, key))
                                {
                                    slot.1 = pair[1];
                                } else {
                                    entries.push((key, pair[1]));
                                }
                            }
                        }
                        return Ok(Value::Object(self.alloc_map(entries)));
                    }
                    "Set" => {
                        // new Set(iterable?)：接受**任意可迭代**，元素按
                        // SameValueZero 去重，键与值同存元素原值（size/has/迭代语义）
                        let arg = args.first().copied().unwrap_or(Value::Undefined);
                        let items = if matches!(arg, Value::Undefined | Value::Null) {
                            Vec::new()
                        } else {
                            self.collect_iter_values(arg)?
                        };
                        let mut entries: Vec<(Value, Value)> = Vec::new();
                        for elem in items {
                            if !entries.iter().any(|(k, _)| self.values_same_zero(*k, elem)) {
                                entries.push((elem, elem));
                            }
                        }
                        let set_ref = self.alloc_map(entries);
                        // 登记 Set 实例句柄（与 Map 共用 HeapObject::Map 变体，
                        // 迭代/分派靠登记区分——见 iter.rs）
                        self.register_set_instance(set_ref);
                        return Ok(Value::Object(set_ref));
                    }
                    "URL" => return crate::builtins::global::url_obj::url_ctor(self, args),
                    "Proxy" => return self.construct_proxy(args),
                    "Function" => return self.construct_function(args),
                    // 包装对象（`new Boolean(v)` / `new Number(v)`）：Ordinary
                    // 实例挂对应原型 + `_primData` 私有槽（M7.2 修复：此前
                    // 与无 new 直调共用原始值分支，包装对象从未存在）。
                    // 无 new 直调的原始值语义在 `invoke_callable` 的
                    // NativeCtor 分支另行处理，两路径于此分离。
                    "Number" => {
                        let proto = match self.get_property(callee, "prototype").map(|v| v.case()) {
                            Ok(ValueCase::Object(p)) => Some(p),
                            _ => self.num_proto,
                        };
                        let inst = self.alloc_ordinary_with_proto(proto);
                        // 无参 → +0（规范 `new Number()` 的 [[NumberData]] 为 +0；
                        // 此前经 to_number_value(undefined) 得 NaN，S15.7.2.1 族）
                        let v = match args.first() {
                            None => 0.0,
                            Some(a) if a.is_undefined() => 0.0,
                            Some(a) => self.numeric_operand(*a)?,
                        };
                        // 数据槽直接以 Dict 模式承载（eq/方法分派的纯堆读取面）
                        if let Some(HeapObject::Ordinary { props, .. }) =
                            self.heap.get_mut(inst.0 as usize)
                        {
                            *props = OrdinaryProps::Dict {
                                properties: vec![("[[NumberValue]]".to_owned(), Value::Number(v))],
                                index: std::collections::HashMap::from([(
                                    "[[NumberValue]]".to_owned(),
                                    0usize,
                                )]),
                            };
                        }
                        return Ok(Value::Object(inst));
                    }
                    "Boolean" => {
                        let proto = match self.get_property(callee, "prototype").map(|v| v.case()) {
                            Ok(ValueCase::Object(p)) => Some(p),
                            _ => self.bool_proto,
                        };
                        let inst = self.alloc_ordinary_with_proto(proto);
                        let v = self.truthy(args.first().copied().unwrap_or(Value::Undefined));
                        if let Some(HeapObject::Ordinary { props, .. }) =
                            self.heap.get_mut(inst.0 as usize)
                        {
                            *props = OrdinaryProps::Dict {
                                properties: vec![(
                                    "[[BooleanValue]]".to_owned(),
                                    Value::Boolean(v),
                                )],
                                index: std::collections::HashMap::from([(
                                    "[[BooleanValue]]".to_owned(),
                                    0usize,
                                )]),
                            };
                        }
                        return Ok(Value::Object(inst));
                    }
                    // `new String(v)`：包装实例 + `[[StringValue]]` 数据槽
                    // + length/索引自有属性（真实 String 包装的读取面）
                    "String" => {
                        let proto = match self.get_property(callee, "prototype").map(|v| v.case()) {
                            Ok(ValueCase::Object(p)) => Some(p),
                            _ => None,
                        };
                        let inst = self.alloc_ordinary_with_proto(proto);
                        let text = match args.first() {
                            None => String::new(),
                            Some(v) => self.js_string(*v)?,
                        };
                        let s_val = Value::Object(self.alloc_string(text.clone()));
                        // 数据槽 Dict 模式直载（eq 纯堆读取面）
                        if let Some(HeapObject::Ordinary { props, .. }) =
                            self.heap.get_mut(inst.0 as usize)
                        {
                            *props = OrdinaryProps::Dict {
                                properties: vec![("[[StringValue]]".to_owned(), s_val)],
                                index: std::collections::HashMap::from([(
                                    "[[StringValue]]".to_owned(),
                                    0usize,
                                )]),
                            };
                        }
                        let _ = self.set_property(
                            Value::Object(inst),
                            "length",
                            Value::Number(text.chars().count() as f64),
                        );
                        for (i, ch) in text.chars().enumerate() {
                            let ch_val = Value::Object(self.alloc_string(ch.to_string()));
                            let _ = self.set_property(Value::Object(inst), &i.to_string(), ch_val);
                        }
                        return Ok(Value::Object(inst));
                    }
                    "Date" => return self.construct_date(args),
                    // `new Buffer(arg[, enc[, len]])`：与裸调用同一实现
                    //（Node 两种形态等价）
                    "Buffer" => {
                        return crate::builtins::buffer::buffer_construct(
                            self,
                            args,
                            Value::Object(r),
                        );
                    }
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
                if name == "stream.Readable" {
                    let r = crate::builtins::stream::create_readable_instance(self, args)?;
                    return Ok(Value::Object(r));
                }
                if name == "stream.Writable" {
                    let r = crate::builtins::stream::create_writable_instance(self, args)?;
                    return Ok(Value::Object(r));
                }
                if name == "stream.Transform" {
                    let r = crate::builtins::stream::create_transform_instance(self, args)?;
                    return Ok(Value::Object(r));
                }
                if let Some(handler) = self.builtin_registry.lookup(name) {
                    return handler(self, args);
                }
            }
        }
        // 非对象 callee（符号/数字/字符串等原始值）不可构造：
        // `new Symbol()(符号原始值)` / `new Object(Symbol())()` 等 → TypeError
        // （规范 IsConstructor 对非对象恒 false，S19.4.3 族）
        if self.is_symbol(callee) {
            return Err(self.type_error("Symbol is not a constructor"));
        }
        let proto_ref = match self.get_property(callee, "prototype").map(|v| v.case()) {
            Ok(ValueCase::Object(p)) => Some(p),
            _ => None,
        };
        let instance_ref = self.alloc_ordinary_with_proto(proto_ref);
        let instance_val = Value::Object(instance_ref);
        let (f_idx, uvs) = self.resolve_callable(callee);
        if let Some(fi) = f_idx {
            let res = self.invoke_function(fi, instance_val, args, uvs)?;
            if matches!(res.case(), ValueCase::Object(_)) {
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
        // 内建构造器的 `super(...)`（`class E extends Error { constructor(m)
        // { super(m); } }`）：NativeCtor 无可执行字节码，此前直接返回未初始化
        // 的 this → 父类构造语义（message/name）完全丢失。
        // 复用 `new Error(m)` 的构造路径取得父类初始化结果，再把**子类原型**
        // 与 this 的既有自有属性（在 super() 之前由派生构造器写入的）合并到
        // 返回实例上——规范 [[Construct]] 返回父类实例、派生构造器继续以
        // 该实例为 this（子类原型由 NewTarget 决定，实例化时已注入）。
        if let Some(r) = callee.as_object() {
            if matches!(
                self.heap.get(r.0 as usize),
                Some(HeapObject::NativeCtor { .. })
            ) {
                let built = self.do_construct(callee, args)?;
                if let Some(br) = built.as_object() {
                    // 保留派生构造器已建立的 this 原型（子类原型），并把
                    // 父类构造写入的自有属性（message 等）复制过去
                    if let Some(tr) = this_val.as_object() {
                        let proto = match self.heap.get(tr.0 as usize) {
                            Some(HeapObject::Ordinary { proto, .. }) => *proto,
                            _ => None,
                        };
                        self.set_prototype_of(built, proto);
                        // 复制父类构造写入的自有属性（message/stack 等）：
                        // 必须用**含不可枚举键**的 `own_entries_all`——
                        // `Error` 面上 message/stack 均为不可枚举（Node 语义），
                        // 用 `own_entries` 会全部过滤掉，致
                        // `class E extends Error { constructor(m) { super(m); } }`
                        // 的实例 message 丢失（实测：`new E('x').message === ""`）。
                        let entries = self.own_entries_all(br.0 as usize);
                        for (k, v) in entries {
                            let _ = self.set_property(Value::Object(tr), &k, v);
                        }
                        // 复制后需重算 stack 首行（子类名 + message）
                        self.refresh_error_stack_name(tr);
                        self.refresh_error_enumerability(tr);
                        return Ok(Value::Object(tr));
                    }
                }
                return Ok(built);
            }
        }
        Ok(this_val)
    }

    /// 将 spread 参数表转为参数列表（对齐 Go 版 `toArrayValues`）：
    /// 数组取元素列表，普通对象取自有属性值集，其余为空。
    pub(crate) fn to_array_values(&self, val: Value) -> Vec<Value> {
        if let Some(r) = val.as_object() {
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
            let gen_val = self.make_generator(&tmpl, func_idx, this_val, args, upvalues);
            // async 生成器：参数默认值在**调用时**同步求值（规范
            // EvaluateAsyncGeneratorBody 的参数初始化先于生成器对象返回；
            // `f(_ = thrower())` 的抛错在 f() 调用点同步传播）——创建后
            // 立即驱动一次，执行停在解析器注入的边界 yield 标记（函数体
            // 尚未开始），默认值抛错沿本次驱动同步上抛
            if tmpl.is_async
                && let Some(gen_ref) = gen_val.as_object()
            {
                self.drive_generator(gen_ref, None)?;
            }
            return Ok(gen_val);
        }
        // Tier 1 热点：达阈值且资格符合 → 直接执行机器码（无 OSR，函数入口切换）
        if self.jit_enabled {
            if let Some(jit) = self.jit_lookup(func_idx) {
                return Ok(self.jit_run(func_idx, &jit, args, tmpl.num_params as usize, upvalues));
            }
        }
        // 调用链登记：进入解释帧推入，任何退出路径（含 ?）由 FrameGuard 弹出。
        // 函数名以 `Rc<str>` 共享（模板名 → 链帧零拷贝），错误 `stack` 生成时复用。
        CALL_CHAIN.with(|c| c.borrow_mut().push((func_idx, tmpl.name.clone().into())));
        let _frame_guard = FrameGuard;
        let old_func_idx = std::mem::replace(&mut self.current_func_idx, func_idx as i64);
        let old_coverage_func = self.coverage.as_mut().map(|c| {
            (
                std::mem::replace(&mut c.cur_func, func_idx as i64),
                c.last_hit.take(),
            )
        });
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
                    // `arguments` 载体是数组（实现选择），但按规范它是普通
                    // 类数组对象——`Array.isArray(arguments) === false`。
                    // 打内部标记供 isArray 排除（其余数组语义不变）
                    let _ = self.set_property(
                        Value::Object(args_arr),
                        "_isArguments",
                        Value::Boolean(true),
                    );
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
                if let (Some(cov), Some((of, oh))) = (self.coverage.as_mut(), old_coverage_func) {
                    cov.cur_func = of;
                    cov.last_hit = oh;
                }
                if let (Some(cov), Some((of, oh))) = (self.coverage.as_mut(), old_coverage_func) {
                    cov.cur_func = of;
                    cov.last_hit = oh;
                }
                self.locals = saved_frame.locals;
                self.current_upvalues = saved_frame.upvalues;
                self.open_upvalues = saved_frame.open_upvalues.into_iter().collect();
                // 上值 cell → 宿主槽回写（与正常返回路径对称）：async 函数
                // **挂起前**已执行的语句对外层绑定的写入（`async function f()
                // { v = true; await p; }` 中的 v 经 STORE_UPVALUE）须对调用者
                // 立即可见——async 体在首个 await 前是**同步执行**的
                for (slot, uv) in &self.open_upvalues {
                    if let Some(loc) = self.locals.get_mut(*slot) {
                        *loc = *uv.0.borrow();
                    }
                }
                self.try_stack = saved_frame.try_stack;
                let p_obj = self.alloc_pending_promise();
                // 目标已兑现（await 一个 settled promise）：不会再收到 fulfill
                // 事件，须**立即**排队恢复任务（否则帧永不续跑）
                let already_settled = matches!(
                    self.heap.get(awaited_promise.index()),
                    Some(HeapObject::Promise { pending: false, .. })
                );
                let resume = crate::builtins::PendingResume {
                    frame,
                    func_idx,
                    promise: p_obj,
                    awaited: *awaited_promise,
                };
                if already_settled {
                    self.microtask_queue
                        .push_back(crate::builtins::Job::ResumeFrame(resume));
                } else {
                    self.promise_resumes
                        .insert(awaited_promise.index() as u32, resume);
                }
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
        if let (Some(cov), Some((of, oh))) = (self.coverage.as_mut(), old_coverage_func) {
            cov.cur_func = of;
            cov.last_hit = oh;
        }
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
            // async 函数**同步抛错**（首次 await 之前）：规范
            // AsyncFunctionStart/AsyncBlockStart 要求把异常转为**返回 Promise 的
            // 拒绝**，而不是向调用方同步抛出。此前只包装 `Ok`，同步 throw 直接以
            // `Err` 逃逸到调用方——真实项目实测：`async route() { … throw … }`
            // 的异常穿透 `handle().catch(...)` 变成模块级未捕获错误（探针
            // `demo/taskboard-demo/tools/probe-async-throw.js` /
            // demo 的 404 路径）。
            Err(VmError::Thrown(reason)) if tmpl.is_async => {
                let p = self.alloc_rejected_promise(reason);
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
        // 脚本/CJS main 帧：顶层 `this` = 全局对象
        //（`typeof this === "object"`，非函数内 this 的 undefined 语义）
        if func.name == "main"
            && let Some(gt) = self.globals.get("globalThis").copied()
            && !self.locals.is_empty()
        {
            self.locals[0] = gt;
        }
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
        // 模块替换：函数索引语义改变，动态求值缓存同样失效
        self.eval_module_cache.clear();
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
        // 模块替换：函数索引语义改变，JIT 与动态求值缓存必须清空
        self.eval_module_cache.clear();
        self.jit_reset();
        // 先执行 Func 0（主函数）
        let res = self.run_func(&self.module_functions[0].clone())?;

        let mut ret = res;
        if let Some(r) = res.as_object() {
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
        // CJS wrapper 的 this = **exports 对象**（`typeof this === "object"`，
        // 与 modules.rs 的 require 加载路径一致；此前传 undefined）
        let ret = self.invoke_function(
            func_idx,
            exports,
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
