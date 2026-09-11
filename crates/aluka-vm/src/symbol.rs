//! Symbol 原语：堆对象承载（`HeapObject::Symbol`），经 `Value::Object` 句柄
//! 引用。唯一性由句柄身份保证（`===` 走引用比较）；`Symbol.for` 注册表保证
//! 同键复现；属性键经私有区前缀 mangling 存入 `Ordinary.properties`
//! （`Object.keys` / `JSON.stringify` 过滤之，`getOwnPropertySymbols` 反解）。
//!
//! 语义对齐 Node.js 22 LTS 标准 实测：
//! - `typeof sym === "symbol"`；`console.log(sym)` → `Symbol(d)`；`String(sym)` 同形；
//! - 同描述的两个 Symbol 不相等；`Symbol.for(k)` 幂等；`keyFor` 仅注册符号有值；
//! - `.description` 在 Go 侧未实现（恒 undefined）——**不实现**，保持对拍一致；
//! - 知名符号（`Symbol.iterator` 等 13 个）各自唯一且缓存。

use crate::heap::HeapObject;
use crate::interpreter::Vm;
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// 属性键 mangling 前缀（Unicode 私用区，普通字符串键不可达）。
pub(crate) const SYMBOL_KEY_PREFIX: char = '\u{E000}';

/// 自增符号 id（唯一性）。
static NEXT_SYM_ID: AtomicU64 = AtomicU64::new(1);

/// 取下一个全局符号 id（内置模块直接构造 Symbol 对象时复用同一计数器）。
pub(crate) fn next_symbol_id() -> u64 {
    NEXT_SYM_ID.fetch_add(1, Ordering::Relaxed)
}

// `Symbol.for` 注册表：键 → 符号对象句柄（线程局部：堆句柄仅本线程 Vm 有效）。
thread_local! {
    static FOR_REGISTRY: RefCell<HashMap<String, ObjectRef>> = RefCell::new(HashMap::new());
}

// 反查表：符号句柄 → 注册键（`keyFor` 用；非注册符号不在表中）。
thread_local! {
    static FOR_BY_HANDLE: RefCell<HashMap<u32, String>> = RefCell::new(HashMap::new());
}

// 知名符号缓存：名称 → 符号对象句柄。
thread_local! {
    static WELL_KNOWN: RefCell<HashMap<String, ObjectRef>> = RefCell::new(HashMap::new());
}

/// 知名符号名单（Node 22 全集）。
pub(crate) const WELL_KNOWN_NAMES: &[&str] = &[
    "asyncIterator",
    "hasInstance",
    "isConcatSpreadable",
    "iterator",
    "match",
    "matchAll",
    "replace",
    "search",
    "species",
    "split",
    "toPrimitive",
    "toStringTag",
    "unscopables",
];

/// 符号显示形态：`Symbol(d)` / `Symbol()`。
pub(crate) fn symbol_display(description: &str) -> String {
    format!("Symbol({description})")
}

/// 符号句柄 → 属性键 mangling。
pub(crate) fn mangled_key(handle: ObjectRef) -> String {
    format!("{SYMBOL_KEY_PREFIX}sym:{}", handle.0)
}

/// 属性键反解：是符号键则还原句柄。
pub(crate) fn parse_symbol_key(key: &str) -> Option<ObjectRef> {
    let rest = key.strip_prefix(SYMBOL_KEY_PREFIX)?.strip_prefix("sym:")?;
    rest.parse::<u32>().ok().map(ObjectRef)
}

/// 判断属性键是否为符号键。
pub(crate) fn is_symbol_key(key: &str) -> bool {
    key.starts_with(SYMBOL_KEY_PREFIX)
}

/// GC root provider：符号注册表持有的句柄（for 注册表 / 反查键 / 知名符号）。
pub(crate) fn registry_roots(out: &mut crate::gc::GcRoots) {
    let handles: Vec<ObjectRef> = FOR_REGISTRY.with(|c| c.borrow().values().copied().collect());
    for r in handles {
        out.push(Value::Object(r));
    }
    let by_handle: Vec<u32> = FOR_BY_HANDLE.with(|c| c.borrow().keys().copied().collect());
    for h in by_handle {
        out.push(Value::Object(ObjectRef(h)));
    }
    let well_known: Vec<ObjectRef> = WELL_KNOWN.with(|c| c.borrow().values().copied().collect());
    for r in well_known {
        out.push(Value::Object(r));
    }
}

impl Vm {
    /// 判断值是否为符号。
    pub(crate) fn is_symbol(&self, val: Value) -> bool {
        matches!(val.case(), ValueCase::Object(r)
                if matches!(self.heap.get(r.0 as usize), Some(HeapObject::Symbol { .. }))
        )
    }

    /// `Symbol([description])`：分配唯一符号。
    pub(crate) fn symbol_create(&mut self, args: &[Value]) -> Value {
        let description = match args.first() {
            Some(Value::Undefined) | None => String::new(),
            Some(v) => self.format_value(*v),
        };
        let id = NEXT_SYM_ID.fetch_add(1, Ordering::Relaxed);
        Value::Object(self.alloc_symbol(id, description))
    }

    /// `Symbol.for(key)`：注册表幂等分配。
    pub(crate) fn symbol_for(
        &mut self,
        args: &[Value],
    ) -> Result<Value, crate::interpreter::VmError> {
        let key = args
            .first()
            .map(|v| self.format_value(*v))
            .unwrap_or_default();
        if let Some(handle) = FOR_REGISTRY.with(|c| c.borrow().get(&key).copied()) {
            return Ok(Value::Object(handle));
        }
        let id = NEXT_SYM_ID.fetch_add(1, Ordering::Relaxed);
        let handle = self.alloc_symbol(id, key.clone());
        FOR_REGISTRY.with(|c| c.borrow_mut().insert(key.clone(), handle));
        FOR_BY_HANDLE.with(|c| c.borrow_mut().insert(handle.0, key));
        Ok(Value::Object(handle))
    }

    /// `Symbol.keyFor(sym)`：注册符号返回键，否则 undefined。
    pub(crate) fn symbol_key_for(
        &mut self,
        args: &[Value],
    ) -> Result<Value, crate::interpreter::VmError> {
        let arg = args.first().copied().unwrap_or(Value::Undefined);
        if !self.is_symbol(arg) {
            return Ok(Value::Undefined);
        }
        let registered = match arg.case() {
            ValueCase::Object(r) => FOR_BY_HANDLE.with(|c| c.borrow().get(&r.0).cloned()),
            _ => None,
        };
        Ok(match registered {
            Some(key) => Value::Object(self.alloc_string(key)),
            None => Value::Undefined,
        })
    }

    /// 知名符号（`Symbol.iterator` 等）：缓存幂等。
    pub(crate) fn well_known_symbol(&mut self, name: &str) -> Value {
        if let Some(handle) = WELL_KNOWN.with(|c| c.borrow().get(name).copied()) {
            return Value::Object(handle);
        }
        let id = NEXT_SYM_ID.fetch_add(1, Ordering::Relaxed);
        let handle = self.alloc_symbol(id, format!("Symbol.{name}"));
        WELL_KNOWN.with(|c| c.borrow_mut().insert(name.to_owned(), handle));
        Value::Object(handle)
    }

    /// 符号接收者的原型方法：`toString` / `valueOf`。
    pub(crate) fn call_symbol_method(
        &mut self,
        method: &str,
        sym: ObjectRef,
    ) -> Option<Result<Value, crate::interpreter::VmError>> {
        let description = match self.heap.get(sym.0 as usize) {
            Some(HeapObject::Symbol { description, .. }) => description.clone(),
            _ => return None,
        };
        match method {
            "toString" => Some(Ok(Value::Object(
                self.alloc_string(symbol_display(&description)),
            ))),
            "valueOf" => Some(Ok(Value::Object(sym))),
            _ => None,
        }
    }
}
