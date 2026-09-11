//! KDF 模块面：`pbkdf2Sync` / `pbkdf2`、`scryptSync` / `scrypt`、
//! `hkdfSync` / `hkdf`（含异步回调投递）。
//!
//! 语义对齐 Node.js 22 LTS 标准（`nodecrypto/crypto.go` 与 `crypto_kdf.go`）：
//! - `pbkdf2Sync(password, salt, iterations, keylen[, digest])`，digest 缺省 `sha1`；
//! - `pbkdf2(...)`：第六参为回调，结果经宏任务回放 `(err)` / `(null, Buffer)`；
//! - `scryptSync(password, salt, keylen[, options])`，options 支持 `N`/`r`/`p`
//!   （缺省 16384/8/1）；
//! - `hkdfSync(digest, ikm, salt, info, keylen)` / `hkdf(..., callback)`。

use super::async_cb::{Delivery, is_callable, schedule_delivery, throw_error};
use super::digest::Algo;
use super::inst_digest::crypto_bytes;
use super::kdf::{hkdf_key, pbkdf2_key, scrypt_key};
use crate::builtins::buffer::create_buffer_instance;
use crate::builtins::register_handler;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;

/// Go `nodebase.IntArg` 对应：第 i 个参数取整（缺失/非数字返回默认值）。
pub(crate) fn int_arg(args: &[Value], i: usize, default: i64) -> i64 {
    match args.get(i) {
        Some(Value::Number(n)) => *n as i64,
        _ => default,
    }
}

/// 解析 digest 算法名（pbkdf2 仅支持 sha1/sha256/sha512，对齐 Go）。
fn parse_pbkdf2_digest(vm: &mut Vm, name: &str) -> Result<Algo, VmError> {
    match name {
        "sha1" => Ok(Algo::Sha1),
        "sha256" => Ok(Algo::Sha256),
        "sha512" => Ok(Algo::Sha512),
        _ => Err(throw_error(
            vm,
            &format!("pbkdf2: unsupported digest \"{name}\""),
        )),
    }
}

/// `pbkdf2Sync(password, salt, iterations, keylen[, digest])`。
fn pbkdf2_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 4 {
        return Err(throw_error(
            vm,
            "pbkdf2Sync: password, salt, iterations, keylen required",
        ));
    }
    let password = crypto_bytes(vm, args[0]);
    let salt = crypto_bytes(vm, args[1]);
    let iterations = int_arg(args, 2, 0);
    let keylen = int_arg(args, 3, 0);
    let digest = if args.len() > 4 {
        vm.format_value(args[4])
    } else {
        "sha1".to_owned()
    };
    let algo = parse_pbkdf2_digest(vm, &digest)?;
    match pbkdf2_key(algo, &password, &salt, iterations, keylen) {
        Ok(out) => Ok(Value::Object(create_buffer_instance(vm, out))),
        Err(msg) => Err(throw_error(vm, &msg)),
    }
}

/// `pbkdf2(password, salt, iterations, keylen, digest, callback)`。
fn pbkdf2_async(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 6 {
        return Err(throw_error(
            vm,
            "pbkdf2: password, salt, iterations, keylen, digest, callback required",
        ));
    }
    let password = crypto_bytes(vm, args[0]);
    let salt = crypto_bytes(vm, args[1]);
    let iterations = int_arg(args, 2, 0);
    let keylen = int_arg(args, 3, 0);
    let digest = vm.format_value(args[4]);
    let cb = args[5];
    if is_callable(vm, cb) {
        let algo = parse_pbkdf2_digest(vm, &digest)?;
        let result = pbkdf2_key(algo, &password, &salt, iterations, keylen);
        schedule_delivery(
            vm,
            cb,
            match result {
                Ok(out) => Delivery::Bytes(out),
                Err(msg) => Delivery::Err(msg),
            },
        );
    }
    Ok(Value::Undefined)
}

/// scrypt 解析结果：`(password, salt, keylen, N, r, p)`。
type ScryptArgs = (Vec<u8>, Vec<u8>, usize, i64, i64, i64);

/// 解析 scrypt 参数：`(password, salt, keylen[, options])`。
/// options 支持 `N`/`r`/`p`（缺省 16384/8/1；显式传 0/负数按原值传入以对齐
/// Go 的报错路径）。
fn parse_scrypt_args(vm: &mut Vm, args: &[Value]) -> Result<ScryptArgs, VmError> {
    if args.len() < 3 {
        return Err(throw_error(
            vm,
            "scrypt: password, salt and keylen required",
        ));
    }
    let password = crypto_bytes(vm, args[0]);
    let salt = crypto_bytes(vm, args[1]);
    let keylen = int_arg(args, 2, 0);
    if keylen <= 0 {
        return Err(throw_error(vm, "scrypt: keylen must be positive"));
    }
    let (mut n, mut r, mut p) = (16384i64, 8i64, 1i64);
    if let Some(opt_ref) = args.get(3).as_object {
        for (key, dest) in [("N", &mut n), ("r", &mut r), ("p", &mut p)] {
            if let Ok(Value::Number(num)) = vm.get_property(Value::Object(*opt_ref), key) {
                *dest = num as i64;
            }
        }
    }
    Ok((password, salt, keylen as usize, n, r, p))
}

/// `scryptSync(password, salt, keylen[, options])`。
fn scrypt_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let (password, salt, keylen, n, r, p) = parse_scrypt_args(vm, args)?;
    match scrypt_key(&password, &salt, n, r, p, keylen) {
        Ok(dk) => Ok(Value::Object(create_buffer_instance(vm, dk))),
        Err(msg) => Err(throw_error(vm, &format!("scrypt: {msg}"))),
    }
}

/// `scrypt(password, salt, keylen[, options], callback)`。
fn scrypt_async(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    // 回调取最后一个函数参数（对齐 Go）
    let mut cb = None;
    for arg in args.iter().rev() {
        if is_callable(vm, *arg) {
            cb = Some(*arg);
            break;
        }
    }
    let (password, salt, keylen, n, r, p) = parse_scrypt_args(vm, args)?;
    if let Some(cb) = cb {
        let result = scrypt_key(&password, &salt, n, r, p, keylen);
        schedule_delivery(
            vm,
            cb,
            match result {
                Ok(dk) => Delivery::Bytes(dk),
                Err(msg) => Delivery::Err(format!("scrypt: {msg}")),
            },
        );
    }
    Ok(Value::Undefined)
}

/// `hkdfSync(digest, ikm, salt, info, keylen)`。
fn hkdf_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 5 {
        return Err(throw_error(
            vm,
            "hkdfSync: digest, ikm, salt, info, keylen required",
        ));
    }
    let digest = vm.format_value(args[0]);
    let ikm = crypto_bytes(vm, args[1]);
    let salt = crypto_bytes(vm, args[2]);
    let info = crypto_bytes(vm, args[3]);
    let keylen = int_arg(args, 4, 0);
    match run_hkdf(&digest, &ikm, &salt, &info, keylen) {
        Ok(out) => Ok(Value::Object(create_buffer_instance(vm, out))),
        Err(msg) => Err(throw_error(vm, &msg)),
    }
}

/// `hkdf(digest, ikm, salt, info, keylen, callback)`。
fn hkdf_async(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.len() < 6 {
        return Err(throw_error(
            vm,
            "hkdf: digest, ikm, salt, info, keylen, callback required",
        ));
    }
    let digest = vm.format_value(args[0]);
    let ikm = crypto_bytes(vm, args[1]);
    let salt = crypto_bytes(vm, args[2]);
    let info = crypto_bytes(vm, args[3]);
    let keylen = int_arg(args, 4, 0);
    let cb = args[5];
    if is_callable(vm, cb) {
        let result = run_hkdf(&digest, &ikm, &salt, &info, keylen);
        schedule_delivery(
            vm,
            cb,
            match result {
                Ok(out) => Delivery::Bytes(out),
                Err(msg) => Delivery::Err(msg),
            },
        );
    }
    Ok(Value::Undefined)
}

/// HKDF 计算内核（digest 未经支持时复用统一算法表错误文案）。
fn run_hkdf(
    digest: &str,
    ikm: &[u8],
    salt: &[u8],
    info: &[u8],
    keylen: i64,
) -> Result<Vec<u8>, String> {
    let Some(algo) = Algo::from_name(digest) else {
        return Err(format!("createHash: unsupported algorithm \"{digest}\""));
    };
    if keylen <= 0 {
        return Err("hkdf: keylen must be positive".to_owned());
    }
    hkdf_key(algo, ikm, salt, info, keylen as usize)
}

/// 在模块 build 时登记 KDF 模块面。
pub(crate) fn register(registry: &mut crate::builtins::BuiltinRegistry) {
    register_handler(registry, "crypto", "pbkdf2Sync", pbkdf2_sync);
    register_handler(registry, "crypto", "pbkdf2", pbkdf2_async);
    register_handler(registry, "crypto", "scryptSync", scrypt_sync);
    register_handler(registry, "crypto", "scrypt", scrypt_async);
    register_handler(registry, "crypto", "hkdfSync", hkdf_sync);
    register_handler(registry, "crypto", "hkdf", hkdf_async);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// int_arg 缺省与取整语义。
    #[test]
    fn int_arg_defaults() {
        let args = [Value::Number(3.9), Value::Undefined];
        assert_eq!(int_arg(&args, 0, 7), 3);
        assert_eq!(int_arg(&args, 1, 7), 7);
        assert_eq!(int_arg(&args, 9, 7), 7);
    }

    /// 编译期锚定：处理器签名与注册表一致。
    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = pbkdf2_sync;
        let _: crate::builtins::BuiltinHandler = pbkdf2_async;
        let _: crate::builtins::BuiltinHandler = scrypt_sync;
        let _: crate::builtins::BuiltinHandler = scrypt_async;
        let _: crate::builtins::BuiltinHandler = hkdf_sync;
        let _: crate::builtins::BuiltinHandler = hkdf_async;
    }
}
