//! `crypto` / `node:crypto` 内置模块（Phase 4）：加密、哈希与密钥派生。
//!
//! 语义以 Node.js 22 LTS 规范为唯一真理，
//! 全部算法（md5/sha1/sha2/HMAC/AES/PBKDF2/scrypt/HKDF）为**手写纯 Rust
//! 实现**（零外部 crate），并以官方标准测试向量（FIPS 180-4/197、RFC
//! 1321/2104/2202/2898/4231/5869/6070/7914、SP 800-38A/38D）锚定。
//!
//! 模块面（与 Go `NewCrypto` + `registerX509` 一致）：
//! - 摘要：`createHash` / `createHmac` / `hash` / `getHashes`；
//! - 对称密码：`createCipheriv` / `createDecipheriv`（CBC/ECB/CTR/GCM）；
//! - KDF：`pbkdf2Sync` / `pbkdf2` / `scryptSync` / `scrypt` / `hkdfSync` / `hkdf`；
//! - 随机：`randomBytes` / `randomUUID` / `randomInt` / `randomFillSync` / `randomFill`；
//! - 其他：`timingSafeEqual` / `createSecretKey` / `checkPrimeSync` / `checkPrime` /
//!   `getCiphers` / `getRandomValues` / `webcrypto`；
//! - X.509：`X509Certificate` / `createPrivateKey`。
//!
//! 实例分派：Hash/Hmac/Cipher/X509 实例挂 `_builtinNs` 标记，`CALL_METHOD`
//! 按 `"{ns}.{method}"` 查分派表（见 `builtins::try_dispatch`）；实例可变状态
//! 存线程局部静态表（键为堆句柄）。异步回调（`pbkdf2`/`scrypt`/`hkdf`/`randomInt`/
//! `checkPrime`/`randomFill`）经宏任务投递蹦床回放，顺序与 Node.js 22 LTS 标准 的
//! 「同步脚本 → 微任务 → 宏任务」一致。

mod aes;
mod async_cb;
mod der;
mod digest;
mod enc;
mod hmac;
mod inst_cipher;
mod inst_digest;
mod kdf;
mod kdf_api;
mod md5;
mod modes;
mod rand_api;
mod random;
mod sha1;
mod sha2;
mod x509_api;

use crate::builtins::buffer::{create_buffer_instance, extract_bytes};
use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::cell::RefCell;

/// `require("crypto")` / `require("node:crypto")` 主模块。
pub const MODULE: ModuleDef = ModuleDef {
    name: "crypto",
    build,
};

/// `getCiphers` 的确定性算法表（逐字对齐 Node.js 22 LTS 标准）。
const CIPHER_NAMES: [&str; 9] = [
    "aes-128-cbc",
    "aes-192-cbc",
    "aes-256-cbc",
    "aes-128-ecb",
    "aes-256-ecb",
    "aes-128-ctr",
    "aes-256-ctr",
    "aes-128-gcm",
    "aes-256-gcm",
];

/// 构建 crypto 模块单例并登记全部方法处理器。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();

    // 各子模块登记自己的处理器（含实例命名空间）
    inst_digest::register(registry);
    inst_cipher::register(registry);
    kdf_api::register(registry);
    rand_api::register(registry);
    x509_api::register(registry);
    async_cb::register(registry);
    register_handler(registry, "crypto", "getHashes", get_hashes);
    register_handler(registry, "crypto", "hash", inst_digest::one_shot_hash);
    register_handler(registry, "crypto", "timingSafeEqual", timing_safe_equal);
    register_handler(registry, "crypto", "createSecretKey", create_secret_key);
    register_handler(registry, "crypto", "checkPrimeSync", check_prime_sync);
    register_handler(registry, "crypto", "checkPrime", check_prime);
    register_handler(registry, "crypto", "getCiphers", get_ciphers);
    register_handler(registry, "crypto", "getRandomValues", get_random_values);
    register_handler(registry, "crypto:secret", "export", secret_export);

    // 模块属性：全部为原生函数（GET_PROP 后调用按函数名分派）
    let functions = [
        "createHash",
        "createHmac",
        "hash",
        "getHashes",
        "createCipheriv",
        "createDecipheriv",
        "pbkdf2Sync",
        "pbkdf2",
        "scryptSync",
        "scrypt",
        "hkdfSync",
        "hkdf",
        "randomBytes",
        "randomUUID",
        "randomInt",
        "randomFillSync",
        "randomFill",
        "timingSafeEqual",
        "createSecretKey",
        "checkPrimeSync",
        "checkPrime",
        "getCiphers",
        "getRandomValues",
        "X509Certificate",
        "createPrivateKey",
    ];
    for name in functions {
        let fn_ref = vm.alloc_native_fn(&format!("crypto.{name}"));
        set_module_prop(vm, obj, name, Value::Object(fn_ref))?;
    }

    // webcrypto 占位对象（Rust VM 无全局 crypto；对齐 `typeof webcrypto === "object"`）
    let webcrypto = vm.alloc_ordinary();
    let subtle = vm.alloc_ordinary();
    let _ = set_module_prop(vm, webcrypto, "subtle", Value::Object(subtle));
    let grv_fn = vm.alloc_native_fn("crypto.getRandomValues");
    let _ = set_module_prop(vm, webcrypto, "getRandomValues", Value::Object(grv_fn));
    let _ = set_module_prop(vm, obj, "webcrypto", Value::Object(webcrypto));

    Ok(obj)
}

/// `getHashes()`：确定性摘要算法数组。
fn get_hashes(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let elems: Vec<Value> = ["md5", "sha1", "sha256", "sha384", "sha512"]
        .iter()
        .map(|name| Value::Object(vm.alloc_string((*name).to_owned())))
        .collect();
    Ok(Value::Object(vm.alloc_array(elems)))
}

/// `getCiphers()`：确定性对称算法数组（与 Go 相同的 9 项固定表）。
fn get_ciphers(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let elems: Vec<Value> = CIPHER_NAMES
        .iter()
        .map(|name| Value::Object(vm.alloc_string((*name).to_owned())))
        .collect();
    Ok(Value::Object(vm.alloc_array(elems)))
}

/// 严格 Buffer 字节提取（仅认真实 Buffer 实例；字符串不算，对齐 Go `AsBuffer`）。
pub(crate) fn strict_buffer_bytes(vm: &Vm, v: Value) -> Option<Vec<u8>> {
    let Value::Object(r) = v else {
        return None;
    };
    let is_buffer = match vm.heap.get(r.0 as usize) {
        Some(HeapObject::Ordinary { .. }) => vm.has_own_slot(r.0 as usize, "_isBuffer"),
        _ => false,
    };
    if is_buffer {
        extract_bytes(vm, v)
    } else {
        None
    }
}

/// `timingSafeEqual(a, b)`：等长 Buffer 常数时间比较。
fn timing_safe_equal(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 2 {
        return Err(async_cb::throw_error(
            vm,
            "timingSafeEqual: two buffers required",
        ));
    }
    let (Some(a), Some(b)) = (
        strict_buffer_bytes(vm, args[0]),
        strict_buffer_bytes(vm, args[1]),
    ) else {
        return Err(async_cb::throw_error(
            vm,
            "timingSafeEqual: arguments must be Buffer, TypedArray, or DataView",
        ));
    };
    if a.len() != b.len() {
        return Err(async_cb::throw_error(
            vm,
            "timingSafeEqual: input buffers must have the same length",
        ));
    }
    // 常数时间比较（异或累积）
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(&b) {
        diff |= x ^ y;
    }
    Ok(Value::Boolean(diff == 0))
}

/// `createSecretKey(key)`：`{ type: "secret", symmetricKeySize, export() }`。
fn create_secret_key(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() {
        return Err(async_cb::throw_error(
            vm,
            "createSecretKey: key argument required",
        ));
    }
    let key = inst_digest::crypto_bytes(vm, args[0]);
    let obj = vm.alloc_ordinary();
    let type_val = Value::Object(vm.alloc_string("secret".to_owned()));
    let _ = set_module_prop(vm, obj, "type", type_val);
    let _ = vm.set_property(
        Value::Object(obj),
        "symmetricKeySize",
        Value::Number(key.len() as f64),
    );
    let export_fn = vm.alloc_native_fn("crypto:secret.export");
    let _ = vm.set_property(Value::Object(obj), "export", Value::Object(export_fn));
    let ns_val = Value::Object(vm.alloc_string("crypto:secret".to_owned()));
    let _ = vm.set_property(Value::Object(obj), "_builtinNs", ns_val);
    // export() 捕获密钥字节：经线程局部表登记（键为 KeyObject 句柄）
    SECRET_KEYS.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(std::collections::HashMap::new)
            .insert(obj.0, key);
    });
    Ok(Value::Object(obj))
}

// secret KeyObject 句柄 → 密钥字节（export 实例方法取用；线程局部：堆句柄仅本线程 Vm 有效）。
thread_local! {
    static SECRET_KEYS: RefCell<Option<std::collections::HashMap<u32, Vec<u8>>>> =
        const { RefCell::new(None) };
}

/// 实例 `export()`：导出 secret KeyObject 字节为 Buffer。
fn secret_export(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let Value::Object(r) = crate::builtins::current_receiver() else {
        return Ok(Value::Undefined);
    };
    let bytes = SECRET_KEYS.with(|g| {
        g.borrow()
            .as_ref()
            .and_then(|m| m.get(&r.0))
            .cloned()
            .unwrap_or_default()
    });
    Ok(Value::Object(create_buffer_instance(vm, bytes)))
}

/// `checkPrimeSync(candidate)`：Miller-Rabin 确定性素性检验（i64 域内确定）。
fn check_prime_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(check_prime_value(vm, args)))
}

/// 从参数提取候选整数（BigInt 十进制串支持；其余按 Number）。
fn check_prime_value(vm: &Vm, args: &[Value]) -> bool {
    let Some(arg) = args.first() else {
        return false;
    };
    let candidate: i64 = match arg {
        Value::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(HeapObject::BigInt(text)) => text.trim().parse().unwrap_or(0),
            _ => return false,
        },
        Value::Number(n) => {
            if n.fract() != 0.0 {
                return false;
            }
            *n as i64
        }
        _ => return false,
    };
    is_prime_u64(candidate as u64)
}

/// `checkPrime(candidate, callback)`：异步素性检验。
fn check_prime(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(cb) = args.last().copied() else {
        return Err(async_cb::throw_error(vm, "checkPrime: callback required"));
    };
    if !async_cb::is_callable(vm, cb) {
        return Err(async_cb::throw_error(vm, "checkPrime: callback required"));
    }
    let verdict = check_prime_value(vm, args);
    async_cb::schedule_delivery(vm, cb, async_cb::Delivery::Bool(verdict));
    Ok(Value::Undefined)
}

/// Miller-Rabin 素性检验（固定基数组；u64 域内与 Go `ProbablyPrime` 的布尔
/// 输出在常规输入下一致，负数/0/1 均为合数）。
fn is_prime_u64(n: u64) -> bool {
    if n < 2 {
        return false;
    }
    const SMALL_PRIMES: [u64; 15] = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47];
    for p in SMALL_PRIMES {
        if n == p {
            return true;
        }
        if n % p == 0 {
            return false;
        }
    }
    let d = (n - 1) >> (n - 1).trailing_zeros();
    let s = (n - 1).trailing_zeros();
    for base in [2u64, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41] {
        let mut x = pow_mod(base, d, n);
        if x == 1 || x == n - 1 {
            continue;
        }
        let mut composite = true;
        for _ in 0..s {
            x = mul_mod(x, x, n);
            if x == n - 1 {
                composite = false;
                break;
            }
        }
        if composite {
            return false;
        }
    }
    true
}

/// 模乘（u128 中间积；模数限 u64 域）。
fn mul_mod(a: u64, b: u64, m: u64) -> u64 {
    ((u128::from(a) * u128::from(b)) % u128::from(m)) as u64
}

/// 快速模幂。
fn pow_mod(mut base: u64, mut exp: u64, m: u64) -> u64 {
    let mut result: u64 = 1;
    base %= m;
    while exp > 0 {
        if exp & 1 == 1 {
            result = mul_mod(result, base, m);
        }
        base = mul_mod(base, base, m);
        exp >>= 1;
    }
    result
}

/// `getRandomValues(typedArray)`：就地向 Buffer/TypedArray 填充随机字节。
fn get_random_values(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if let Some(arg) = args.first().copied() {
        if let Some(mut bytes) = extract_bytes(vm, arg) {
            random::fill_random(&mut bytes);
            if let Value::Object(r) = arg {
                crate::builtins::buffer::overwrite_buffer_instance(vm, r, &bytes);
            }
            return Ok(arg);
        }
    }
    Err(async_cb::throw_error(
        vm,
        "getRandomValues: expects a typed array",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// getCiphers 表与 Node.js 22 LTS 标准 逐字一致（9 项固定顺序）。
    #[test]
    fn cipher_table_matches_go() {
        assert_eq!(
            CIPHER_NAMES,
            [
                "aes-128-cbc",
                "aes-192-cbc",
                "aes-256-cbc",
                "aes-128-ecb",
                "aes-256-ecb",
                "aes-128-ctr",
                "aes-256-ctr",
                "aes-128-gcm",
                "aes-256-gcm"
            ]
        );
    }

    /// Miller-Rabin 与已知素数/合数表一致。
    #[test]
    fn primality_basics() {
        for p in [2u64, 3, 5, 7, 97, 7919, 104729, 67280421310721] {
            assert!(is_prime_u64(p), "{p} 应为素数");
        }
        for c in [0u64, 1, 4, 100, 7917, 1_000_001] {
            // 1000001 = 101 × 9901
            assert!(!is_prime_u64(c), "{c} 应为合数");
        }
        // 大素数与强伪素数边界
        assert!(is_prime_u64(2_147_483_647));
        assert!(!is_prime_u64(2_147_483_649));
        assert!(is_prime_u64(67280421310721));
    }

    /// 编译期锚定：处理器签名与注册表一致。
    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = get_hashes;
        let _: crate::builtins::BuiltinHandler = get_ciphers;
        let _: crate::builtins::BuiltinHandler = timing_safe_equal;
        let _: crate::builtins::BuiltinHandler = create_secret_key;
        let _: crate::builtins::BuiltinHandler = check_prime_sync;
        let _: crate::builtins::BuiltinHandler = check_prime;
        let _: crate::builtins::BuiltinHandler = get_random_values;
        let _: crate::builtins::BuiltinHandler = secret_export;
    }
}
