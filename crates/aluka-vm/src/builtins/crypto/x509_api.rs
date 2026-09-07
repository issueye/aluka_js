//! X.509 证书与私钥表面：`X509Certificate` 构造器与 `createPrivateKey`
//! （对齐 Node.js 22 LTS 标准 `nodecrypto/crypto_x509.go` 的简化实现）。
//!
//! 实例属性：`subject` / `issuer`（`TAG=value\n...`）、`validFrom` / `validTo`
//! （GMT 串）、`serialNumber`（大写 hex）、`fingerprint{,256,512}`（冒号分隔
//! 大写 hex）、`ca`、`raw`、`subjectAltName`、`keyUsage`、`publicKey`；
//! 实例方法：`toString` / `checkHost` / `checkIssued` / `checkPrivateKey` /
//! `verify` / `toLegacyObject`。

use super::async_cb::throw_error;
use super::der::{parse_certificate, parse_private_key_pem, parse_public_key_pem};
use super::digest::{Algo, Engine, to_hex};
use super::enc::pem_encode;
use super::inst_digest::crypto_bytes;
use crate::builtins::buffer::{create_buffer_instance, extract_bytes};
use crate::builtins::register_handler;
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 实例句柄 → 解析后的证书。
static X509_CERTS: Mutex<Option<HashMap<u32, Arc<super::der::ParsedCert>>>> = Mutex::new(None);

/// 密钥对象句柄 → RSA 公开参数（公私钥对象通用）。
static KEY_STORE: Mutex<Option<HashMap<u32, KeyEntry>>> = Mutex::new(None);

/// 密钥对象登记项（RSA 公开参数；公私钥对象通用）。
#[derive(Debug, Clone)]
struct KeyEntry {
    /// RSA 模数（去符号位大端）
    modulus: Vec<u8>,
    /// 指数
    exponent: u64,
}

/// 写入证书登记。
fn set_cert(id: u32, cert: Arc<super::der::ParsedCert>) {
    X509_CERTS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(id, cert);
}

/// 读取证书登记。
fn get_cert(r: ObjectRef) -> Option<Arc<super::der::ParsedCert>> {
    X509_CERTS.lock().unwrap().as_ref()?.get(&r.0).cloned()
}

/// 登记密钥对象。
fn set_key(id: u32, entry: KeyEntry) {
    KEY_STORE
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(id, entry);
}

/// 读取密钥对象登记。
fn get_key(r: ObjectRef) -> Option<KeyEntry> {
    KEY_STORE.lock().unwrap().as_ref()?.get(&r.0).cloned()
}

/// 读取对象字符串属性（缺失返回 `None`）。
fn get_string_prop(vm: &mut Vm, r: ObjectRef, key: &str) -> Option<String> {
    match vm.get_property(Value::Object(r), key) {
        Ok(v) => match v {
            Value::Object(s) => match vm.heap.get(s.index()) {
                Some(HeapObject::String(text)) => Some(text.clone()),
                _ => None,
            },
            _ => None,
        },
        Err(_) => None,
    }
}

/// 模数位数（去符号位大端 → bit 长度）。
fn modulus_bits(modulus: &[u8]) -> usize {
    match modulus.first() {
        None => 0,
        Some(&lead) => (modulus.len() - 1) * 8 + (8 - lead.leading_zeros() as usize),
    }
}

/// 冒号分隔大写 hex 指纹（对齐 Go `x509Fingerprint`）。
fn fingerprint(der: &[u8], algo: Algo) -> String {
    let mut h = Engine::new(algo);
    h.update(der);
    h.finalize()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// `X509Certificate(cert)`（`new` 与直调等价）：cert 为 PEM 串或 DER Buffer。
fn x509_certificate(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(arg) = args.first() else {
        return Err(throw_error(
            vm,
            "X509Certificate: certificate argument required",
        ));
    };
    // 字符串 → PEM 解码；Buffer 实例 → DER（对齐 Go：先试 PEM，失败按
    // Buffer 处理仅在非字符串时发生）
    let is_string = matches!(arg, Value::Object(r) if matches!(
        vm.heap.get(r.0 as usize),
        Some(HeapObject::String(_))
    ));
    let from_pem = if is_string {
        true
    } else {
        extract_bytes(vm, *arg).is_none()
    };
    let der = if is_string || from_pem {
        let text = vm.format_value(*arg);
        match super::enc::pem_decode(&text) {
            Some(der) => der,
            None => return Err(throw_error(vm, "X509Certificate: invalid PEM certificate")),
        }
    } else {
        extract_bytes(vm, *arg).unwrap_or_default()
    };
    let Some(parsed) = parse_certificate(&der) else {
        if from_pem {
            return Err(throw_error(vm, "X509Certificate: invalid PEM certificate"));
        }
        return Err(throw_error(vm, "X509Certificate: invalid certificate DER"));
    };
    let parsed = Arc::new(parsed);
    let obj = vm.alloc_ordinary();
    let der = parsed.der.clone();
    let spki_der = parsed.spki_der.clone();
    let is_ca = parsed.is_ca;
    let san = parsed.san.clone();
    let eku = parsed.ext_key_usage.clone();
    let serial = parsed.serial.clone();

    let string_prop = |vm: &mut Vm, key: &str, value: String| {
        let s_val = Value::Object(vm.alloc_string(value));
        let _ = vm.set_property(Value::Object(obj), key, s_val);
    };
    string_prop(vm, "subject", parsed.subject.clone());
    string_prop(vm, "issuer", parsed.issuer.clone());
    string_prop(vm, "validFrom", parsed.valid_from.clone());
    string_prop(vm, "validTo", parsed.valid_to.clone());
    string_prop(vm, "serialNumber", to_hex(&serial).to_uppercase());
    string_prop(vm, "fingerprint", fingerprint(&der, Algo::Sha1));
    string_prop(vm, "fingerprint256", fingerprint(&der, Algo::Sha256));
    string_prop(vm, "fingerprint512", fingerprint(&der, Algo::Sha512));
    let _ = vm.set_property(Value::Object(obj), "ca", Value::Boolean(is_ca));
    let raw_val = Value::Object(create_buffer_instance(vm, der.clone()));
    let _ = vm.set_property(Value::Object(obj), "raw", raw_val);
    if !san.is_empty() {
        string_prop(vm, "subjectAltName", san);
    }
    if !eku.is_empty() {
        let elems: Vec<Value> = eku
            .iter()
            .map(|oid| Value::Object(vm.alloc_string(oid.clone())))
            .collect();
        let key_usage = Value::Object(vm.alloc_array(elems));
        let _ = vm.set_property(Value::Object(obj), "keyUsage", key_usage);
    }
    // publicKey（简化 KeyObject：raw SPKI + 公开参数登记）
    let pk = vm.alloc_ordinary();
    let pk_type = Value::Object(vm.alloc_string("public".to_owned()));
    let _ = vm.set_property(Value::Object(pk), "type", pk_type);
    let pk_akt = Value::Object(vm.alloc_string("rsa".to_owned()));
    let _ = vm.set_property(Value::Object(pk), "asymmetricKeyType", pk_akt);
    let pk_raw = Value::Object(create_buffer_instance(vm, spki_der));
    let _ = vm.set_property(Value::Object(pk), "raw", pk_raw);
    // 对齐 Go：cert.publicKey 不登记为可验证密钥参数（verify(publicKey) 为 false）
    let _ = vm.set_property(Value::Object(obj), "publicKey", Value::Object(pk));

    // 实例方法（实例挂 _builtinNs，CALL_METHOD 以实例为 receiver 分派）
    let _ = set_module_prop_x509(vm, obj);
    for (name, f) in [
        ("toString", "crypto:x509.toString"),
        ("checkHost", "crypto:x509.checkHost"),
        ("checkIssued", "crypto:x509.checkIssued"),
        ("checkPrivateKey", "crypto:x509.checkPrivateKey"),
        ("verify", "crypto:x509.verify"),
        ("toLegacyObject", "crypto:x509.toLegacyObject"),
    ] {
        let fn_ref = vm.alloc_native_fn(f);
        let _ = vm.set_property(Value::Object(obj), name, Value::Object(fn_ref));
    }
    set_cert(obj.0, parsed);
    Ok(Value::Object(obj))
}

/// 给 X509 实例挂命名空间标记（实例方法按 `crypto:x509.{method}` 分派）。
fn set_module_prop_x509(vm: &mut Vm, obj: ObjectRef) -> Result<(), VmError> {
    let ns = Value::Object(vm.alloc_string("crypto:x509".to_owned()));
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", ns);
    Ok(())
}

/// `createPrivateKey(key)`：PEM RSA 私钥（PKCS#1/PKCS#8）→ 简化 KeyObject。
fn create_private_key(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() {
        return Err(throw_error(vm, "createPrivateKey: key argument required"));
    }
    let data = crypto_bytes(vm, args[0]);
    let text = String::from_utf8_lossy(&data).into_owned();
    let Some(info) = parse_private_key_pem(&text) else {
        return Err(throw_error(vm, "createPrivateKey: invalid PEM"));
    };
    let obj = vm.alloc_ordinary();
    let ko_type = Value::Object(vm.alloc_string("private".to_owned()));
    let _ = vm.set_property(Value::Object(obj), "type", ko_type);
    let ko_akt = Value::Object(vm.alloc_string("rsa".to_owned()));
    let _ = vm.set_property(Value::Object(obj), "asymmetricKeyType", ko_akt);
    let ko_pem = Value::Object(vm.alloc_string(text));
    let _ = vm.set_property(Value::Object(obj), "__alukaKeyPEM", ko_pem);
    set_key(
        obj.0,
        KeyEntry {
            modulus: info.modulus,
            exponent: info.exponent,
        },
    );
    Ok(Value::Object(obj))
}

/// RSA 公开参数相等（模数 + 指数逐字节比对）。
fn keys_equal(a: (&[u8], u64), b: (&[u8], u64)) -> bool {
    a.0 == b.0 && a.1 == b.1
}

/// 从 JS 值解析密钥公开参数（KeyObject 登记 / `__alukaKeyPEM` / PEM 文本）。
fn key_info_from_arg(vm: &mut Vm, v: Value) -> Option<KeyEntry> {
    if let Value::Object(r) = v {
        if let Some(entry) = get_key(r) {
            return Some(entry);
        }
        if let Some(pem) = get_string_prop(vm, r, "__alukaKeyPEM") {
            if let Some(info) = parse_private_key_pem(&pem) {
                return Some(KeyEntry {
                    modulus: info.modulus,
                    exponent: info.exponent,
                });
            }
            if let Some(info) = parse_public_key_pem(&pem) {
                return Some(KeyEntry {
                    modulus: info.modulus,
                    exponent: info.exponent,
                });
            }
        }
    }
    // 裸 PEM 文本
    let text = vm.format_value(v);
    if let Some(info) = parse_private_key_pem(&text) {
        return Some(KeyEntry {
            modulus: info.modulus,
            exponent: info.exponent,
        });
    }
    parse_public_key_pem(&text).map(|info| KeyEntry {
        modulus: info.modulus,
        exponent: info.exponent,
    })
}

/// 实例 `toString()`：原始 PEM。
fn instance_to_string(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Value::Object(r) = crate::builtins::current_receiver() else {
        return Ok(Value::Undefined);
    };
    let Some(cert) = get_cert(r) else {
        return Ok(Value::Undefined);
    };
    Ok(Value::Object(
        vm.alloc_string(pem_encode("CERTIFICATE", &cert.der)),
    ))
}

/// 实例 `checkHost(host)`：精确 DNS → 通配符 DNS → IP；不匹配返回 undefined。
fn instance_check_host(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Value::Object(r) = crate::builtins::current_receiver() else {
        return Ok(Value::Undefined);
    };
    let Some(cert) = get_cert(r) else {
        return Ok(Value::Undefined);
    };
    let Some(arg) = args.first() else {
        return Ok(Value::Undefined);
    };
    let host = vm.format_value(*arg);
    let h = host.trim_end_matches('.').to_lowercase();
    for d in &cert.dns_names {
        if d.to_lowercase() == h {
            return Ok(Value::Object(vm.alloc_string(d.clone())));
        }
    }
    for d in &cert.dns_names {
        let Some(stripped) = d.strip_prefix("*.") else {
            continue;
        };
        let suffix = format!(".{}", stripped.to_lowercase());
        if h.ends_with(&suffix) {
            let left = &h[..h.len() - suffix.len()];
            if !left.contains('.') {
                return Ok(Value::Object(vm.alloc_string(d.clone())));
            }
        }
    }
    for ip in &cert.ip_addresses {
        if *ip == h {
            return Ok(Value::Object(vm.alloc_string(host)));
        }
    }
    Ok(Value::Undefined)
}

/// 实例 `checkIssued(other)`：issuer DN 与 other.subject DN 串一致（对齐 Go 简化）。
fn instance_check_issued(_vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Value::Object(r) = crate::builtins::current_receiver() else {
        return Ok(Value::Undefined);
    };
    let Some(cert) = get_cert(r) else {
        return Ok(Value::Undefined);
    };
    let Some(Value::Object(other)) = args.first().copied() else {
        return Ok(Value::Boolean(false));
    };
    let Some(other_cert) = get_cert(other) else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(cert.issuer == other_cert.subject))
}

/// 实例 `checkPrivateKey(key)`：私钥公开参数与证书公钥匹配。
fn instance_check_private_key(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Value::Object(r) = crate::builtins::current_receiver() else {
        return Ok(Value::Undefined);
    };
    let Some(cert) = get_cert(r) else {
        return Ok(Value::Undefined);
    };
    let Some(arg) = args.first().copied() else {
        return Ok(Value::Boolean(false));
    };
    let Some(entry) = key_info_from_arg(vm, arg) else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(keys_equal(
        (&cert.modulus, cert.exponent),
        (&entry.modulus, entry.exponent),
    )))
}

/// 实例 `verify(publicKey)`：给定公钥（或私钥）参数与证书公钥匹配。
fn instance_verify(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Value::Object(r) = crate::builtins::current_receiver() else {
        return Ok(Value::Undefined);
    };
    let Some(cert) = get_cert(r) else {
        return Ok(Value::Undefined);
    };
    let Some(arg) = args.first().copied() else {
        return Ok(Value::Boolean(false));
    };
    let Some(entry) = key_info_from_arg(vm, arg) else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(keys_equal(
        (&cert.modulus, cert.exponent),
        (&entry.modulus, entry.exponent),
    )))
}

/// 实例 `toLegacyObject()`：对象形式（Node 结构；时间串为 UTC 后缀）。
fn instance_to_legacy_object(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Value::Object(r) = crate::builtins::current_receiver() else {
        return Ok(Value::Undefined);
    };
    let Some(cert) = get_cert(r) else {
        return Ok(Value::Undefined);
    };
    let lo = vm.alloc_ordinary();
    let obj_string = |vm: &mut Vm, target: ObjectRef, key: &str, value: String| {
        let s_val = Value::Object(vm.alloc_string(value));
        let _ = vm.set_property(Value::Object(target), key, s_val);
    };
    // subject / issuer：{TAG: value} 对象
    for (field, pairs) in [
        ("subject", &cert.subject_pairs),
        ("issuer", &cert.issuer_pairs),
    ] {
        let o = vm.alloc_ordinary();
        for (tag, value) in pairs {
            obj_string(vm, o, tag, value.clone());
        }
        let _ = vm.set_property(Value::Object(lo), field, Value::Object(o));
    }
    if !cert.san.is_empty() {
        obj_string(vm, lo, "subjectaltname", cert.san.clone());
    }
    let _ = vm.set_property(Value::Object(lo), "ca", Value::Boolean(cert.is_ca));
    if !cert.modulus.is_empty() {
        obj_string(vm, lo, "modulus", to_hex(&cert.modulus).to_uppercase());
        obj_string(vm, lo, "bits", format!("{}", modulus_bits(&cert.modulus)));
        obj_string(vm, lo, "exponent", format!("{:#x}", cert.exponent));
    }
    let pubkey_val = Value::Object(create_buffer_instance(vm, cert.spki_der.clone()));
    let _ = vm.set_property(Value::Object(lo), "pubkey", pubkey_val);
    // Go：toLegacyObject 的时间用 cert.NotBefore.Format(...)，时区为 UTC
    obj_string(
        vm,
        lo,
        "valid_from",
        cert.valid_from.replace(" GMT", " UTC"),
    );
    obj_string(vm, lo, "valid_to", cert.valid_to.replace(" GMT", " UTC"));
    obj_string(vm, lo, "fingerprint", fingerprint(&cert.der, Algo::Sha1));
    obj_string(
        vm,
        lo,
        "fingerprint256",
        fingerprint(&cert.der, Algo::Sha256),
    );
    obj_string(
        vm,
        lo,
        "fingerprint512",
        fingerprint(&cert.der, Algo::Sha512),
    );
    if !cert.ext_key_usage.is_empty() {
        let elems: Vec<Value> = cert
            .ext_key_usage
            .iter()
            .map(|oid| Value::Object(vm.alloc_string(oid.clone())))
            .collect();
        let eku_val = Value::Object(vm.alloc_array(elems));
        let _ = vm.set_property(Value::Object(lo), "ext_key_usage", eku_val);
    }
    obj_string(vm, lo, "serialNumber", to_hex(&cert.serial).to_uppercase());
    let raw_out = Value::Object(create_buffer_instance(vm, cert.der.clone()));
    let _ = vm.set_property(Value::Object(lo), "raw", raw_out);
    Ok(Value::Object(lo))
}

/// 在模块 build 时登记 X509 表面。
pub(crate) fn register(registry: &mut crate::builtins::BuiltinRegistry) {
    register_handler(registry, "crypto", "X509Certificate", x509_certificate);
    register_handler(registry, "crypto", "createPrivateKey", create_private_key);
    register_handler(registry, "crypto:x509", "toString", instance_to_string);
    register_handler(registry, "crypto:x509", "checkHost", instance_check_host);
    register_handler(
        registry,
        "crypto:x509",
        "checkIssued",
        instance_check_issued,
    );
    register_handler(
        registry,
        "crypto:x509",
        "checkPrivateKey",
        instance_check_private_key,
    );
    register_handler(registry, "crypto:x509", "verify", instance_verify);
    register_handler(
        registry,
        "crypto:x509",
        "toLegacyObject",
        instance_to_legacy_object,
    );
}

#[cfg(test)]
mod tests {
    use super::super::der::RsaPrivateKeyInfo;
    use super::*;

    /// 模数 bit 长度：1024 位模数（首字节 0xc2 → 最高位在第 2 位）。
    #[test]
    fn modulus_bit_length() {
        let info = RsaPrivateKeyInfo {
            modulus: vec![0xc2, 0x74, 0x53],
            exponent: 0x10001,
        };
        assert_eq!(modulus_bits(&info.modulus), 24);
        assert_eq!(modulus_bits(&[0x00, 0xff]), 8);
        assert_eq!(modulus_bits(&[]), 0);
    }

    /// 密钥相等判定：模数逐字节、指数相等。
    #[test]
    fn key_equality() {
        assert!(keys_equal((&[0x01, 0x02], 65537), (&[0x01, 0x02], 65537)));
        assert!(!keys_equal((&[0x01], 65537), (&[0x01], 3)));
    }

    /// 编译期锚定：处理器签名与注册表一致。
    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = x509_certificate;
        let _: crate::builtins::BuiltinHandler = create_private_key;
        let _: crate::builtins::BuiltinHandler = instance_to_string;
        let _: crate::builtins::BuiltinHandler = instance_check_host;
        let _: crate::builtins::BuiltinHandler = instance_check_issued;
        let _: crate::builtins::BuiltinHandler = instance_check_private_key;
        let _: crate::builtins::BuiltinHandler = instance_verify;
        let _: crate::builtins::BuiltinHandler = instance_to_legacy_object;
    }
}
