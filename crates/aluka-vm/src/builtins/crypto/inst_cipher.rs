//! Cipher / Decipher 实例：`createCipheriv` / `createDecipheriv` 工厂与
//! `update` / `final` / `getAuthTag` / `setAuthTag` 实例方法
//! （命名空间 `crypto:cipher`）。
//!
//! 语义对齐 Node.js 22 LTS 标准（`nodecrypto/crypto_cipher.go`）：
//! - 支持 `aes-128/192/256-cbc`、`-ecb`、`-ctr`、`-gcm`；
//! - key 长度必须与算法匹配（`createCipheriv: key must be N bytes`）；
//! - cbc/ctr 要求 16 字节 IV，gcm 要求 12 字节 IV，ecb 允许 null IV；
//! - `update` 把数据累积到状态并**恒返回空 Buffer**（密文在 `final` 产出）；
//! - `final` 非破坏性：状态保留，可重复调用（与 Go 一致）。

use super::aes::AesBlock;
use super::async_cb::throw_error;
use super::inst_digest::crypto_bytes;
use super::modes::{
    cbc_decrypt, cbc_encrypt, ctr, ecb_decrypt, ecb_encrypt, gcm_decrypt, gcm_encrypt, pkcs7_pad,
    pkcs7_unpad,
};
use crate::builtins::buffer::create_buffer_instance;
use crate::builtins::{BuiltinRegistry, current_receiver, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use std::cell::RefCell;
use std::collections::HashMap;

/// 工作模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// CBC（PKCS#7 填充）
    Cbc,
    /// ECB（PKCS#7 填充）
    Ecb,
    /// CTR（流式，无填充）
    Ctr,
    /// GCM（AEAD）
    Gcm,
}

/// 密码实例内部状态。
#[derive(Debug, Clone)]
struct CipherState {
    /// AES 块密码（已扩展轮密钥）
    aes: AesBlock,
    /// IV / nonce
    iv: Vec<u8>,
    /// 解密方向
    decrypt: bool,
    /// 工作模式
    mode: Mode,
    /// 累积数据（update 追加，final 消费但不清空，对齐 Go）
    buf: Vec<u8>,
    /// GCM 认证标签（加密后产出 / 解密前 setAuthTag 设置）
    auth_tag: Vec<u8>,
}

// 实例句柄 → 密码状态（线程局部：堆句柄仅本线程 Vm 有效）。
thread_local! {
    static CIPHER_STATES: RefCell<Option<HashMap<u32, CipherState>>> = const { RefCell::new(None) };
}

/// 写入实例状态。
fn set_state(id: u32, state: CipherState) {
    CIPHER_STATES.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .insert(id, state);
    });
}

/// 读取实例状态快照。
fn get_state(id: u32) -> Option<CipherState> {
    CIPHER_STATES.with(|g| g.borrow().as_ref()?.get(&id).cloned())
}

/// 就地更新实例状态。
fn mutate_state<F: FnOnce(&mut CipherState)>(id: u32, f: F) {
    CIPHER_STATES.with(|g| {
        if let Some(state) = g.borrow_mut().as_mut().and_then(|map| map.get_mut(&id)) {
            f(state);
        }
    });
}

/// `createCipheriv` / `createDecipheriv(algorithm, key, iv)`。
fn create_cipher(vm: &mut Vm, args: &[Value], decrypt: bool) -> Result<Value, VmError> {
    if args.len() < 3 {
        return Err(throw_error(
            vm,
            "createCipheriv: algorithm, key, iv required",
        ));
    }
    let algorithm = vm.format_value(args[0]);
    let alg = algorithm.to_lowercase();
    let key_len: i64 = match alg.as_str() {
        "aes-128-cbc" | "aes-128-ecb" | "aes-128-ctr" | "aes-128-gcm" => 16,
        "aes-192-cbc" => 24,
        "aes-256-cbc" | "aes-256-ecb" | "aes-256-ctr" | "aes-256-gcm" => 32,
        _ => {
            return Err(throw_error(
                vm,
                &format!("createCipheriv: unsupported algorithm \"{algorithm}\""),
            ));
        }
    };
    let key = crypto_bytes(vm, args[1]);
    if key.len() as i64 != key_len {
        return Err(throw_error(
            vm,
            &format!("createCipheriv: key must be {key_len} bytes"),
        ));
    }
    let Some(aes) = AesBlock::new(&key) else {
        return Err(throw_error(vm, "createCipheriv: invalid key length"));
    };
    let iv = if matches!(args[2], Value::Null | Value::Undefined) {
        Vec::new()
    } else {
        crypto_bytes(vm, args[2])
    };
    let mode = if alg.ends_with("-cbc") {
        if iv.len() != 16 {
            return Err(throw_error(vm, "createCipheriv: iv must be 16 bytes"));
        }
        Mode::Cbc
    } else if alg.ends_with("-ecb") {
        Mode::Ecb
    } else if alg.ends_with("-ctr") {
        if iv.len() != 16 {
            return Err(throw_error(vm, "createCipheriv: iv must be 16 bytes"));
        }
        Mode::Ctr
    } else if alg.ends_with("-gcm") {
        if iv.len() != 12 {
            return Err(throw_error(vm, "createCipheriv: iv must be 12 bytes"));
        }
        Mode::Gcm
    } else {
        return Err(throw_error(
            vm,
            &format!("createCipheriv: unsupported algorithm \"{algorithm}\""),
        ));
    };
    let obj = vm.alloc_ordinary();
    let ns_val = Value::Object(vm.alloc_string("crypto:cipher".to_owned()));
    let _ = set_module_prop(vm, obj, "_builtinNs", ns_val);
    let update_fn = vm.alloc_native_fn("crypto:cipher.update");
    let final_fn = vm.alloc_native_fn("crypto:cipher.final");
    let _ = vm.set_property(Value::Object(obj), "update", Value::Object(update_fn));
    let _ = vm.set_property(Value::Object(obj), "final", Value::Object(final_fn));
    if mode == Mode::Gcm {
        let get_tag_fn = vm.alloc_native_fn("crypto:cipher.getAuthTag");
        let set_tag_fn = vm.alloc_native_fn("crypto:cipher.setAuthTag");
        let _ = vm.set_property(Value::Object(obj), "getAuthTag", Value::Object(get_tag_fn));
        let _ = vm.set_property(Value::Object(obj), "setAuthTag", Value::Object(set_tag_fn));
    }
    set_state(
        obj.0,
        CipherState {
            aes,
            iv,
            decrypt,
            mode,
            buf: Vec::new(),
            auth_tag: Vec::new(),
        },
    );
    Ok(Value::Object(obj))
}

/// `createCipheriv(algorithm, key, iv)`。
pub(crate) fn create_cipheriv(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    create_cipher(vm, args, false)
}

/// `createDecipheriv(algorithm, key, iv)`。
pub(crate) fn create_decipheriv(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    create_cipher(vm, args, true)
}

/// 处理累积数据（按模式加解密；对齐 Go `cipherState.finalize`）。
fn finalize_data(state: &CipherState) -> Result<Vec<u8>, String> {
    let bs = 16usize;
    match (state.mode, state.decrypt) {
        (Mode::Cbc, true) => {
            if state.buf.is_empty() || state.buf.len() % bs != 0 {
                return Err("cipher: data length must be multiple of block size".to_owned());
            }
            let out = cbc_decrypt(&state.aes, &state.iv, &state.buf);
            Ok(pkcs7_unpad(&out, bs))
        }
        (Mode::Cbc, false) => {
            let padded = pkcs7_pad(&state.buf, bs);
            Ok(cbc_encrypt(&state.aes, &state.iv, &padded))
        }
        (Mode::Ecb, true) => {
            if state.buf.is_empty() || state.buf.len() % bs != 0 {
                return Err("cipher: data length must be multiple of block size".to_owned());
            }
            let out = ecb_decrypt(&state.aes, &state.buf);
            Ok(pkcs7_unpad(&out, bs))
        }
        (Mode::Ecb, false) => {
            let padded = pkcs7_pad(&state.buf, bs);
            Ok(ecb_encrypt(&state.aes, &padded))
        }
        (Mode::Ctr, _) => Ok(ctr(&state.aes, &state.iv, &state.buf)),
        (Mode::Gcm, true) => {
            if state.auth_tag.is_empty() {
                return Err("cipher: setAuthTag must be called before final for AES-GCM".to_owned());
            }
            gcm_decrypt(&state.aes, &state.iv, &[], &state.buf, &state.auth_tag)
                .ok_or_else(|| "cipher: GCM authentication failed".to_owned())
        }
        (Mode::Gcm, false) => {
            let (_, ct) = gcm_encrypt(&state.aes, &state.iv, &[], &state.buf);
            Ok(ct)
        }
    }
}

/// GCM 加密时顺带产出认证标签（分离返回避免借用冲突）。
fn finalize_gcm_tag(state: &CipherState) -> Vec<u8> {
    let (tag, _) = gcm_encrypt(&state.aes, &state.iv, &[], &state.buf);
    tag.to_vec()
}

/// 实例 `update(data)`：累积数据，恒返回空 Buffer（对齐 Go）。
fn instance_update(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this = current_receiver();
    let id = match this {
        Value::Object(r) => r.0,
        _ => return Ok(Value::Undefined),
    };
    if let Some(arg) = args.first() {
        let data = crypto_bytes(vm, *arg);
        mutate_state(id, |state| state.buf.extend_from_slice(&data));
    }
    Ok(Value::Object(create_buffer_instance(vm, Vec::new())))
}

/// 实例 `final()`：处理累积数据并产出密文/明文 Buffer。
fn instance_final(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = current_receiver();
    let id = match this {
        Value::Object(r) => r.0,
        _ => return Ok(Value::Undefined),
    };
    let Some(state) = get_state(id) else {
        return Ok(Value::Undefined);
    };
    if state.mode == Mode::Gcm && !state.decrypt {
        let tag = finalize_gcm_tag(&state);
        mutate_state(id, |s| s.auth_tag = tag);
    }
    match finalize_data(&state) {
        Ok(out) => Ok(Value::Object(create_buffer_instance(vm, out))),
        Err(msg) => Err(throw_error(vm, &msg)),
    }
}

/// 实例 `getAuthTag()`：GCM 加密后取认证标签。
fn instance_get_auth_tag(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = current_receiver();
    let id = match this {
        Value::Object(r) => r.0,
        _ => return Ok(Value::Undefined),
    };
    let Some(state) = get_state(id) else {
        return Ok(Value::Undefined);
    };
    if state.auth_tag.is_empty() {
        return Err(throw_error(
            vm,
            "getAuthTag: no auth tag available (call final first)",
        ));
    }
    Ok(Value::Object(create_buffer_instance(vm, state.auth_tag)))
}

/// 实例 `setAuthTag(tag)`：GCM 解密前设置认证标签（返回实例，链式）。
fn instance_set_auth_tag(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this = current_receiver();
    let id = match this {
        Value::Object(r) => r.0,
        _ => return Ok(Value::Undefined),
    };
    let Some(arg) = args.first() else {
        return Err(throw_error(vm, "setAuthTag: tag argument required"));
    };
    let tag = crypto_bytes(vm, *arg);
    mutate_state(id, |state| state.auth_tag = tag);
    Ok(this)
}

/// 在模块 build 时登记密码工厂与实例分派。
pub(crate) fn register(registry: &mut BuiltinRegistry) {
    register_handler(registry, "crypto", "createCipheriv", create_cipheriv);
    register_handler(registry, "crypto", "createDecipheriv", create_decipheriv);
    register_handler(registry, "crypto:cipher", "update", instance_update);
    register_handler(registry, "crypto:cipher", "final", instance_final);
    register_handler(
        registry,
        "crypto:cipher",
        "getAuthTag",
        instance_get_auth_tag,
    );
    register_handler(
        registry,
        "crypto:cipher",
        "setAuthTag",
        instance_set_auth_tag,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CBC 往返 + PKCS#7 填充（以 AES-128 已知密钥/IV 自洽验证）。
    #[test]
    fn cbc_state_roundtrip() {
        let key = vec![0x2bu8; 16];
        let iv = vec![0xf0u8; 16];
        let aes = AesBlock::new(&key).unwrap();
        let state = CipherState {
            aes: aes.clone(),
            iv: iv.clone(),
            decrypt: false,
            mode: Mode::Cbc,
            buf: b"Hello, World!".to_vec(),
            auth_tag: Vec::new(),
        };
        let ct = finalize_data(&state).unwrap();
        assert_eq!(ct.len(), 16);
        let dec = CipherState {
            aes,
            iv,
            decrypt: true,
            mode: Mode::Cbc,
            buf: ct,
            auth_tag: Vec::new(),
        };
        assert_eq!(finalize_data(&dec).unwrap(), b"Hello, World!");
    }

    /// GCM 加密产出标签，setAuthTag 后可解密。
    #[test]
    fn gcm_state_roundtrip() {
        let key = vec![1u8; 32];
        let iv = vec![2u8; 12];
        let aes = AesBlock::new(&key).unwrap();
        let enc_state = CipherState {
            aes: aes.clone(),
            iv: iv.clone(),
            decrypt: false,
            mode: Mode::Gcm,
            buf: b"hello gcm".to_vec(),
            auth_tag: Vec::new(),
        };
        let tag = finalize_gcm_tag(&enc_state);
        let ct = finalize_data(&enc_state).unwrap();
        assert_eq!(tag.len(), 16);
        let dec_state = CipherState {
            aes,
            iv,
            decrypt: true,
            mode: Mode::Gcm,
            buf: ct,
            auth_tag: tag,
        };
        assert_eq!(finalize_data(&dec_state).unwrap(), b"hello gcm");
    }

    /// GCM 解密缺标签必须报错（对齐 Go 错误文案）。
    #[test]
    fn gcm_decrypt_requires_tag() {
        let aes = AesBlock::new(&[3u8; 16]).unwrap();
        let state = CipherState {
            aes,
            iv: vec![0u8; 12],
            decrypt: true,
            mode: Mode::Gcm,
            buf: vec![0u8; 16],
            auth_tag: Vec::new(),
        };
        assert_eq!(
            finalize_data(&state).unwrap_err(),
            "cipher: setAuthTag must be called before final for AES-GCM"
        );
    }

    /// 编译期锚定：处理器签名与注册表一致。
    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = create_cipheriv;
        let _: crate::builtins::BuiltinHandler = create_decipheriv;
        let _: crate::builtins::BuiltinHandler = instance_update;
        let _: crate::builtins::BuiltinHandler = instance_final;
        let _: crate::builtins::BuiltinHandler = instance_get_auth_tag;
        let _: crate::builtins::BuiltinHandler = instance_set_auth_tag;
    }
}
