//! Hash / Hmac 实例：`createHash` / `createHmac` 工厂与 `update` / `digest`
//! 链式实例方法（命名空间 `crypto:hash` / `crypto:hmac`）。
//!
//! 语义对齐 Node.js 22 LTS 标准（`nodecrypto/crypto_hash.go`）：
//! - 实例暴露 `algorithm` 属性；`update` 无参时静默跳过并返回自身；
//! - `digest()` 默认返回 Buffer；`digest("hex"|"base64")` 返回字符串，
//!   **其他编码也返回 Buffer**（Go 的 switch default 分支）；
//! - digest 非破坏性：状态保留，可重复 digest 且后续 update 继续累积。

use super::async_cb::throw_error;
use super::digest::{Algo, Engine, to_hex};
use super::enc::base64_encode;
use super::hmac::HmacEngine;
use crate::builtins::buffer::create_buffer_instance;
use crate::builtins::{BuiltinRegistry, current_receiver, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use std::cell::RefCell;
use std::collections::HashMap;

/// 实例背后的摘要状态（Hash 或 Hmac）。
#[derive(Debug, Clone)]
enum DigestState {
    /// 纯摘要
    Hash(Engine),
    /// HMAC（带密钥）
    Hmac(HmacEngine),
}

impl DigestState {
    /// 吸收数据。
    fn update(&mut self, data: &[u8]) {
        match self {
            Self::Hash(e) => e.update(data),
            Self::Hmac(h) => h.update(data),
        }
    }

    /// 求摘要（非破坏性）。
    fn finalize(&self) -> Vec<u8> {
        match self {
            Self::Hash(e) => e.finalize(),
            Self::Hmac(h) => h.finalize(),
        }
    }
}

// 实例句柄 → 摘要状态（线程局部：堆句柄仅本线程 Vm 有效）。
thread_local! {
    static DIGEST_STATES: RefCell<Option<HashMap<u32, DigestState>>> = const { RefCell::new(None) };
}

/// 写入实例状态。
fn set_state(id: u32, state: DigestState) {
    DIGEST_STATES.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .insert(id, state);
    });
}

/// 读取实例状态（克隆返回，不长期持有借用）。
fn get_state(id: u32) -> Option<DigestState> {
    DIGEST_STATES.with(|g| g.borrow().as_ref()?.get(&id).cloned())
}

/// 更新实例状态。
fn mutate_state<F: FnOnce(&mut DigestState)>(id: u32, f: F) {
    DIGEST_STATES.with(|g| {
        if let Some(state) = g.borrow_mut().as_mut().and_then(|map| map.get_mut(&id)) {
            f(state);
        }
    });
}

/// `createHash(algorithm)`：创建 Hash 实例。
pub(crate) fn create_hash(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(arg) = args.first() else {
        return Err(throw_error(vm, "createHash: algorithm required"));
    };
    let name = vm.format_value(*arg);
    let Some(algo) = Algo::from_name(&name) else {
        return Err(throw_error(
            vm,
            &format!("createHash: unsupported algorithm \"{name}\""),
        ));
    };
    Ok(build_digest_instance(
        vm,
        DigestState::Hash(Engine::new(algo)),
        name,
    ))
}

/// `createHmac(algorithm, key)`：创建 Hmac 实例（key 支持 Buffer/字符串）。
pub(crate) fn create_hmac(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 2 {
        return Err(throw_error(vm, "createHmac: algorithm and key required"));
    }
    let name = vm.format_value(args[0]);
    let Some(algo) = Algo::from_name(&name) else {
        return Err(throw_error(
            vm,
            &format!("createHash: unsupported algorithm \"{name}\""),
        ));
    };
    let key = crypto_bytes(vm, args[1]);
    Ok(build_digest_instance(
        vm,
        DigestState::Hmac(HmacEngine::new(algo, &key)),
        name,
    ))
}

/// 构造带 `_builtinNs` 标记的摘要实例（update/digest 自动分派到本模块）。
fn build_digest_instance(vm: &mut Vm, state: DigestState, algorithm: String) -> Value {
    let obj = vm.alloc_ordinary();
    let ns = match state {
        DigestState::Hmac(_) => "crypto:hmac",
        DigestState::Hash(_) => "crypto:hash",
    };
    let ns_val = Value::Object(vm.alloc_string(ns.to_owned()));
    let _ = set_module_prop(vm, obj, "_builtinNs", ns_val);
    let algo_val = Value::Object(vm.alloc_string(algorithm));
    let _ = set_module_prop(vm, obj, "algorithm", algo_val);
    let update_fn = vm.alloc_native_fn(&format!("{ns}.update"));
    let digest_fn = vm.alloc_native_fn(&format!("{ns}.digest"));
    let _ = vm.set_property(Value::Object(obj), "update", Value::Object(update_fn));
    let _ = vm.set_property(Value::Object(obj), "digest", Value::Object(digest_fn));
    set_state(obj.0, state);
    Value::Object(obj)
}

/// 实例 `update(data)`：吸收数据并返回实例自身（链式）。
fn instance_update(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this = current_receiver();
    let id = match this.case() {
        ValueCase::Object(r) => r.0,
        _ => return Ok(Value::Undefined),
    };
    if let Some(arg) = args.first() {
        let data = crypto_bytes(vm, *arg);
        mutate_state(id, |state| state.update(&data));
    }
    Ok(this)
}

/// 实例 `digest([encoding])`：求摘要；默认 Buffer，`hex`/`base64` 返回字符串。
fn instance_digest(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this = current_receiver();
    let id = match this.case() {
        ValueCase::Object(r) => r.0,
        _ => return Ok(Value::Undefined),
    };
    let Some(state) = get_state(id) else {
        return Ok(Value::Undefined);
    };
    let sum = state.finalize();
    let encoding = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_else(|| "buffer".to_owned());
    match encoding.as_str() {
        "hex" => Ok(Value::Object(vm.alloc_string(to_hex(&sum)))),
        "base64" => Ok(Value::Object(vm.alloc_string(base64_encode(&sum)))),
        _ => Ok(Value::Object(create_buffer_instance(vm, sum))),
    }
}

/// 在模块 build 时登记 Hash/Hmac 工厂与实例分派。
pub(crate) fn register(registry: &mut BuiltinRegistry) {
    register_handler(registry, "crypto", "createHash", create_hash);
    register_handler(registry, "crypto", "createHmac", create_hmac);
    register_handler(registry, "crypto:hash", "update", instance_update);
    register_handler(registry, "crypto:hash", "digest", instance_digest);
    register_handler(registry, "crypto:hmac", "update", instance_update);
    register_handler(registry, "crypto:hmac", "digest", instance_digest);
}

/// Go `nodebase.CryptoBytes` 对应：Buffer 实例取底层字节；其余一律按
/// `String(v)` 取 UTF-8 字节（数组/数字等在 Go 中同样走字符串化路径）。
pub(crate) fn crypto_bytes(vm: &Vm, v: Value) -> Vec<u8> {
    if let Some(bytes) = super::strict_buffer_bytes(vm, v) {
        return bytes;
    }
    vm.format_value(v).into_bytes()
}

/// `hash(algorithm, data[, outputEncoding])`：一次性哈希（Go `crypto.hash` 语义）。
pub(crate) fn one_shot_hash(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 2 {
        return Err(throw_error(vm, "hash: algorithm and data required"));
    }
    let name = vm.format_value(args[0]);
    let Some(algo) = Algo::from_name(&name) else {
        return Err(throw_error(
            vm,
            &format!("createHash: unsupported algorithm \"{name}\""),
        ));
    };
    let data = crypto_bytes(vm, args[1]);
    let mut engine = Engine::new(algo);
    engine.update(&data);
    let sum = engine.finalize();
    if let Some(enc) = args.get(2) {
        if !matches!(*enc, Value::Undefined | Value::Null) {
            return match vm.format_value(*enc).as_str() {
                "buffer" => Ok(Value::Object(create_buffer_instance(vm, sum))),
                "hex" => Ok(Value::Object(vm.alloc_string(to_hex(&sum)))),
                "base64" => Ok(Value::Object(vm.alloc_string(base64_encode(&sum)))),
                other => Err(throw_error(
                    vm,
                    &format!("hash: unsupported output encoding \"{other}\""),
                )),
            };
        }
    }
    // Node 22 实测：不传 outputEncoding 时返回 hex 字符串
    Ok(Value::Object(vm.alloc_string(to_hex(&sum))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 命名空间键与摘要输出长度自洽（sha256=32 字节）。
    #[test]
    fn digest_state_lengths() {
        let mut engine = Engine::new(Algo::Sha256);
        engine.update(b"abc");
        let state = DigestState::Hash(engine);
        assert_eq!(state.finalize().len(), 32);
        let hmac = HmacEngine::new(Algo::Sha512, b"k");
        assert_eq!(DigestState::Hmac(hmac).finalize().len(), 64);
    }

    /// 编译期锚定：处理器签名与注册表一致。
    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = create_hash;
        let _: crate::builtins::BuiltinHandler = create_hmac;
        let _: crate::builtins::BuiltinHandler = instance_update;
        let _: crate::builtins::BuiltinHandler = instance_digest;
        let _: crate::builtins::BuiltinHandler = one_shot_hash;
    }
}
