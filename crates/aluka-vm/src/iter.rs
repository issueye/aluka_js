//! 数组 `for...of` 迭代器：`GetIterator` 遇数组时物化带标记的迭代器对象，
//! 编译器生成的 `iter.next()`（`CALL_METHOD "next"`）在此取步进结果
//! `{ value, done }`。迭代位置存于静态表（键为迭代器对象句柄），
//! 对齐 Go 版 OpGetIterator/OpCallMethod 组合的观测语义。

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// 迭代位置表：迭代器对象句柄 → 下一个待产出下标。
static ARRAY_ITER_POS: LazyLock<Mutex<HashMap<u32, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl Vm {
    /// 判断值是否为数组迭代器对象（`_isArrayIterator` 标记）。
    pub(crate) fn is_array_iterator(&self, val: Value) -> bool {
        matches!(
            val,
            Value::Object(r) if self.has_own_slot(r.0 as usize, "_isArrayIterator")
        )
    }

    /// 为数组物化迭代器对象（标记 `_isArrayIterator`，持有源数组引用）。
    pub(crate) fn alloc_array_iterator(&mut self, arr: ObjectRef) -> Value {
        let obj = self.alloc_ordinary();
        let _ = self.set_property(Value::Object(obj), "_isArrayIterator", Value::Boolean(true));
        let _ = self.set_property(Value::Object(obj), "_iterArray", Value::Object(arr));
        ARRAY_ITER_POS.lock().unwrap().insert(obj.0, 0);
        Value::Object(obj)
    }

    /// `iter.next()`：产出 `{ value, done }` 结果对象；耗尽后恒 `{ undefined, true }`。
    pub(crate) fn array_iterator_next(&mut self, iter: ObjectRef) -> Result<Value, VmError> {
        let arr = match self.heap.get(iter.0 as usize) {
            Some(HeapObject::Ordinary { .. }) => {
                match self.own_value(iter.0 as usize, "_iterArray") {
                    Some(Value::Object(a)) => a,
                    _ => return self.make_iterator_result(Value::Undefined, true),
                }
            }
            _ => return self.make_iterator_result(Value::Undefined, true),
        };
        let pos = ARRAY_ITER_POS
            .lock()
            .unwrap()
            .get(&iter.0)
            .copied()
            .unwrap_or(0);
        let (value, done) = match self.heap.get(arr.0 as usize) {
            Some(HeapObject::Array { elements, .. }) => {
                if pos < elements.len() {
                    (elements[pos], false)
                } else {
                    (Value::Undefined, true)
                }
            }
            _ => (Value::Undefined, true),
        };
        ARRAY_ITER_POS.lock().unwrap().insert(iter.0, pos + 1);
        self.make_iterator_result(value, done)
    }

    /// 物化迭代结果对象 `{ value, done }`。
    fn make_iterator_result(&mut self, value: Value, done: bool) -> Result<Value, VmError> {
        let result = self.alloc_ordinary();
        self.set_property(Value::Object(result), "value", value)?;
        self.set_property(Value::Object(result), "done", Value::Boolean(done))?;
        Ok(Value::Object(result))
    }
}
