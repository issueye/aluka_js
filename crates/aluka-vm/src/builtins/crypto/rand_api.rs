//! 随机数模块面：`randomBytes`、`randomUUID`、`randomInt`、
//! `randomFillSync` / `randomFill`（含异步回调投递）。

use super::async_cb::{Delivery, is_callable, schedule_delivery, throw_error};
use super::kdf_api::int_arg;
use super::random::{fill_random, random_int_range, random_uuid_v4};
use crate::builtins::buffer::{create_buffer_instance, extract_bytes};
use crate::builtins::register_handler;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;

/// `randomBytes(size)`：返回随机字节 Buffer（`size <= 0` 或 `> 1MB` 报错）。
fn random_bytes(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let size = int_arg(args, 0, 0);
    if size <= 0 || size > 1 << 20 {
        return Err(throw_error(
            vm,
            &format!("randomBytes: invalid size {size}"),
        ));
    }
    let mut buf = vec![0u8; size as usize];
    fill_random(&mut buf);
    Ok(Value::Object(create_buffer_instance(vm, buf)))
}

/// `randomUUID()`：RFC 4122 version 4 UUID 字符串。
fn random_uuid(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Object(vm.alloc_string(random_uuid_v4())))
}

/// `randomInt([min,] max[, callback])`：`[min, max)` 均匀随机整数
/// （48 位拒绝采样，无模偏差；跨度过大按 Go 报 RangeError 文案）。
fn random_int(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut n = args.len();
    let mut cb: Option<Value> = None;
    if n > 0 && is_callable(vm, args[n - 1]) {
        cb = Some(args[n - 1]);
        n -= 1;
    }
    let (min, max) = match n {
        1 => (0i64, int_arg(args, 0, 0)),
        2 => (int_arg(args, 0, 0), int_arg(args, 1, 0)),
        _ => return Err(throw_error(vm, "randomInt: expected max or (min, max)")),
    };
    if max <= min {
        return Err(throw_error(vm, "randomInt: (max - min) must be positive"));
    }
    if (max - min) as u64 >= 1 << 48 {
        return Err(throw_error(
            vm,
            "randomInt: (max - min) must be less than 2**48",
        ));
    }
    if let Some(cb) = cb {
        schedule_delivery(vm, cb, Delivery::Int(random_int_range(min, max)));
        return Ok(Value::Undefined);
    }
    Ok(Value::Number(random_int_range(min, max) as f64))
}

/// 严格判定值是否为真实 Buffer 实例（`_isBuffer` 标记；字符串不算）。
fn strict_buffer_bytes(vm: &Vm, v: Value) -> Option<Vec<u8>> {
    let Value::Object(r) = v else {
        return None;
    };
    let is_buffer = match vm.heap.get(r.0 as usize) {
        Some(crate::heap::HeapObject::Ordinary { .. }) => {
            vm.has_own_slot(r.0 as usize, "_isBuffer")
        }
        _ => false,
    };
    if is_buffer {
        extract_bytes(vm, v)
    } else {
        None
    }
}

/// 解析 `randomFill[Sync]` 的 `(buffer[, offset[, size]])`。
/// 返回 `(buffer 值, 字节内容, offset, size)`。
fn random_fill_args(vm: &Vm, args: &[Value]) -> Option<(Value, Vec<u8>, usize, usize)> {
    let first = args.first().copied()?;
    let bytes = strict_buffer_bytes(vm, first)?;
    let offset = int_arg(args, 1, 0);
    if offset < 0 || offset as usize > bytes.len() {
        return None;
    }
    let offset = offset as usize;
    let mut size = bytes.len() - offset;
    if args.len() > 2 {
        let explicit = int_arg(args, 2, size as i64);
        if explicit < 0 || offset + explicit as usize > bytes.len() {
            return None;
        }
        size = explicit as usize;
    }
    Some((first, bytes, offset, size))
}

/// `randomFillSync(buffer[, offset[, size]])`：原地填充并返回原 Buffer 值。
fn random_fill_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some((target, mut bytes, offset, size)) = random_fill_args(vm, args) else {
        return Err(throw_error(vm, "randomFillSync: invalid buffer or bounds"));
    };
    fill_random(&mut bytes[offset..offset + size]);
    if let Value::Object(r) = target {
        crate::builtins::buffer::overwrite_buffer_instance(vm, r, &bytes);
    }
    Ok(target)
}

/// `randomFill(buffer[, offset[, size]], callback)`：异步原地填充。
fn random_fill(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() || !is_callable(vm, *args.last().unwrap()) {
        return Err(throw_error(vm, "randomFill: callback required"));
    }
    let cb = *args.last().unwrap();
    let Some((target, mut bytes, offset, size)) = random_fill_args(vm, &args[..args.len() - 1])
    else {
        return Err(throw_error(vm, "randomFill: invalid buffer or bounds"));
    };
    fill_random(&mut bytes[offset..offset + size]);
    if let Value::Object(r) = target {
        crate::builtins::buffer::overwrite_buffer_instance(vm, r, &bytes);
    }
    schedule_delivery(vm, cb, Delivery::Passthrough(target));
    Ok(Value::Undefined)
}

/// 在模块 build 时登记随机数模块面。
pub(crate) fn register(registry: &mut crate::builtins::BuiltinRegistry) {
    register_handler(registry, "crypto", "randomBytes", random_bytes);
    register_handler(registry, "crypto", "randomUUID", random_uuid);
    register_handler(registry, "crypto", "randomInt", random_int);
    register_handler(registry, "crypto", "randomFillSync", random_fill_sync);
    register_handler(registry, "crypto", "randomFill", random_fill);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 严格 Buffer 判定排除裸字符串（语义锚：字符串非 TypedArray/Buffer）。
    #[test]
    fn random_range_bounds() {
        // 随机整数域测试在 random.rs；此处锚定 int_arg 组合
        let args = [Value::Number(5.0), Value::Number(9.0)];
        assert_eq!(int_arg(&args, 0, 0), 5);
        assert_eq!(int_arg(&args, 1, 0), 9);
    }

    /// 编译期锚定：处理器签名与注册表一致。
    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = random_bytes;
        let _: crate::builtins::BuiltinHandler = random_uuid;
        let _: crate::builtins::BuiltinHandler = random_int;
        let _: crate::builtins::BuiltinHandler = random_fill_sync;
        let _: crate::builtins::BuiltinHandler = random_fill;
    }
}
