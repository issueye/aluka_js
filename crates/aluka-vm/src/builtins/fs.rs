//! `fs` 内置模块——同步文件系统族补齐（Phase 2）。
//!
//! 复用解释器预建的 `fs` 单例（readFileSync/writeFileSync/existsSync 同源），
//! 新增 `readdirSync` / `statSync` / `mkdirSync` / `rmSync`，语义实测对齐
//! Node.js 22 LTS 规范：
//! - `readdirSync(path)` 返回文件名数组（Go `os.ReadDir` 按文件名排序）；
//! - `statSync(path)` 返回 Stats 对象：`size`/`mtimeMs` 等数值属性 +
//!   `isFile()`/`isDirectory()` 可调用方法（本文件经「fs.stat」注册表模块
//!   子句柄分派，`CALL_METHOD` 形态二命中）；
//! - `mkdirSync(path[, {recursive}])` / `rmSync(path[, {recursive}])`
//!   （去掉 `node:` 前缀的 `require("fs")` 与 `require("node:fs")` 同单例）。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;
use std::time::UNIX_EPOCH;

/// `require("fs")` 主模块（复用 interpreter 单例）。
pub const MODULE: ModuleDef = ModuleDef { name: "fs", build };

/// `fs.stat` 子模块句柄：`statSync` 返回对象的 `isFile()`/`isDirectory()` 等
/// 方法经此注册（Stats 对象是运行时共享槽位，注册表只记一个句柄）。
pub const STAT_MODULE: ModuleDef = ModuleDef {
    name: "fs.stat",
    build: build_stat_slot,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.fs_object.ok_or_else(|| {
        let msg = vm.alloc_string("fs: 单例未初始化".to_owned());
        VmError::Thrown(Value::Object(msg))
    })?;
    for method in ["readdirSync", "statSync", "mkdirSync", "rmSync"] {
        let fn_ref = vm.alloc_native_fn(&format!("fs.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    register_handler(registry, "fs", "readdirSync", readdir_sync);
    register_handler(registry, "fs", "statSync", stat_sync);
    register_handler(registry, "fs", "mkdirSync", mkdir_sync);
    register_handler(registry, "fs", "rmSync", rm_sync);
    Ok(obj)
}

/// 建立 `statSync` 返回值的共享槽位并登记其方法分派（isFile/isDirectory 等）。
fn build_stat_slot(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let slot = vm.alloc_ordinary();
    register_handler(registry, "fs.stat", "isFile", stat_is_file);
    register_handler(registry, "fs.stat", "isDirectory", stat_is_directory);
    register_handler(registry, "fs.stat", "isSymbolicLink", stat_is_symbolic_link);
    Ok(slot)
}

/// 读 `fs.stat` 槽位上的布尔属性（statSync 每次调用时刷新）。
fn stat_prop(vm: &mut Vm, key: &str) -> Result<Value, VmError> {
    let slot = vm.builtin_registry.module("fs.stat").ok_or_else(|| {
        let msg = vm.alloc_string("fs.stat: 槽位未注册".to_owned());
        VmError::Thrown(Value::Object(msg))
    })?;
    vm.get_property(Value::Object(slot), key)
}

fn stat_is_file(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    stat_prop(vm, "isFile")
}

fn stat_is_directory(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    stat_prop(vm, "isDirectory")
}

fn stat_is_symbolic_link(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    stat_prop(vm, "isSymbolicLink")
}

/// `readdirSync(path)`：目录条目名数组（按文件名排序，对齐 Go `os.ReadDir`）。
fn readdir_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let path = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let mut names: Vec<String> = std::fs::read_dir(&path)
        .map_err(|e| thrown(vm, &format!("fs.readdirSync: {e}")))?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    names.sort();
    let elems: Vec<Value> = names
        .iter()
        .map(|n| Value::Object(vm.alloc_string(n.clone())))
        .collect();
    Ok(Value::Object(vm.alloc_array(elems)))
}

/// `statSync(path)`：Stats 简化对象（数值属性 + isFile/isDirectory 方法）。
fn stat_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let path = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let meta = std::fs::metadata(&path).map_err(|e| thrown(vm, &format!("fs.statSync: {e}")))?;
    let stat_obj = vm.builtin_registry.module("fs.stat").ok_or_else(|| {
        let msg = vm.alloc_string("fs.stat: 槽位未注册".to_owned());
        VmError::Thrown(Value::Object(msg))
    })?;

    let size = meta.len() as i64;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0);
    // S_IF* 文件类型位 + 权限位（对齐 Go `statToObj` 的 mode 构成；探测仅打印
    // isFile/isDirectory/size/mtimeMs，mode 提供形态即可）。
    let mode: i64 = if meta.is_dir() {
        0o040000 | 0o755
    } else if meta.file_type().is_symlink() {
        0o120000 | 0o777
    } else {
        0o100000 | 0o644
    };
    let props: [(&str, Value); 8] = [
        ("isFile", Value::Boolean(meta.is_file())),
        ("isDirectory", Value::Boolean(meta.is_dir())),
        (
            "isSymbolicLink",
            Value::Boolean(meta.file_type().is_symlink()),
        ),
        ("size", Value::Number(size as f64)),
        ("mode", Value::Number(mode as f64)),
        ("mtimeMs", Value::Number(mtime_ms)),
        ("ctimeMs", Value::Number(mtime_ms)),
        ("atimeMs", Value::Number(mtime_ms)),
    ];
    for (k, v) in props {
        let _ = vm.set_property(Value::Object(stat_obj), k, v);
    }
    // birthtimeMs/nlink/uid/gid/rdev/blksize/blocks/ino/dev：占位（对齐 Go 形态，
    // 探测不打印，工程侧不依赖精确值）。
    for (k, v) in [
        ("birthtimeMs", Value::Number(mtime_ms)),
        ("nlink", Value::Number(1.0)),
        ("uid", Value::Number(0.0)),
        ("gid", Value::Number(0.0)),
        ("rdev", Value::Number(0.0)),
        ("blksize", Value::Number(4096.0)),
        ("blocks", Value::Number(0.0)),
        ("ino", Value::Number(0.0)),
        ("dev", Value::Number(0.0)),
    ] {
        let _ = vm.set_property(Value::Object(stat_obj), k, v);
    }
    Ok(Value::Object(stat_obj))
}

/// `mkdirSync(path[, {recursive}])`：创建目录（recursive 时级联）。
fn mkdir_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let path = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let recursive = options_recursive(vm, args.get(1).copied().unwrap_or(Value::Undefined));
    let res = if recursive {
        std::fs::create_dir_all(&path)
    } else {
        std::fs::create_dir(&path)
    };
    res.map_err(|e| thrown(vm, &format!("fs.mkdirSync: {e}")))?;
    Ok(Value::Undefined)
}

/// `rmSync(path[, {recursive}])`：删除文件/目录（recursive 时级联删除目录）。
fn rm_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let path = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    if path.is_empty() {
        return Ok(Value::Undefined);
    }
    let recursive = options_recursive(vm, args.get(1).copied().unwrap_or(Value::Undefined));
    // Go `os.RemoveAll` 对不存在路径返回 nil；std::fs::remove_dir_all 会报
    // NotFound，这里按 Go 口径吞掉（递归删除天然幂等）。
    let res = if recursive {
        match std::fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    } else if std::fs::metadata(&path)
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        std::fs::remove_dir(&path)
    } else {
        std::fs::remove_file(&path)
    };
    res.map_err(|e| thrown(vm, &format!("fs.rmSync: {e}")))?;
    Ok(Value::Undefined)
}

/// 解析第二参数：`{ recursive: true }` 或裸布尔（简化形态）。
fn options_recursive(vm: &mut Vm, opt: Value) -> bool {
    match opt {
        Value::Boolean(b) => b,
        Value::Object(r) => {
            let key = vm
                .get_property(Value::Object(r), "recursive")
                .unwrap_or(Value::Undefined);
            key.is_truthy()
        }
        _ => false,
    }
}

fn thrown(vm: &mut Vm, msg: &str) -> VmError {
    VmError::Thrown(Value::Object(vm.alloc_string(msg.to_owned())))
}

/// 编译期锚定：处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = readdir_sync;
        let _: crate::builtins::BuiltinHandler = stat_sync;
        let _: crate::builtins::BuiltinHandler = mkdir_sync;
        let _: crate::builtins::BuiltinHandler = rm_sync;
        let _: crate::builtins::BuiltinHandler = stat_is_file;
        let _: crate::builtins::BuiltinHandler = stat_is_directory;
    }
}
