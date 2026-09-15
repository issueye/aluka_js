//! `fs` 内置模块——同步文件系统族补齐（Phase 2）。
//!
//! 复用解释器预建的 `fs` 单例（readFileSync/writeFileSync/existsSync 同源），
//! 新增 `readdirSync` / `statSync` / `mkdirSync` / `rmSync`，
//! 以及真实项目实测（`demo/taskboard-demo`）暴露的
//! `renameSync` / `unlinkSync` / `copyFileSync` / `appendFileSync` /
//! `realpathSync`，语义实测对齐 Node.js 22 LTS 规范：
//! - `readdirSync(path)` 返回文件名数组（Go `os.ReadDir` 按文件名排序）；
//! - `statSync(path)` 返回 Stats 对象：`size`/`mtimeMs` 等数值属性 +
//!   `isFile()`/`isDirectory()` 可调用方法（本文件经「fs.stat」注册表模块
//!   子句柄分派，`CALL_METHOD` 形态二命中）；
//! - `mkdirSync(path[, {recursive}])` / `rmSync(path[, {recursive, force}])`
//!   （去掉 `node:` 前缀的 `require("fs")` 与 `require("node:fs")` 同单例）。
//!
//! ## 错误形状（Node 对齐）
//!
//! 此前 fs 家族的错误是**裸字符串**抛出 → `err.name`/`err.code`/`err.message`
//! 全为 `undefined`，真实项目里 `if (err.code === 'ENOENT')` 之类的分支全部失效。
//! 现统一抛 Node 风格 SystemError 对象（[`fs_error`]）：
//! `Error` 实例 + `code`/`errno`/`syscall`/`path`(/`dest`) 属性，
//! `message` 形如 `ENOENT: no such file or directory, open '<绝对路径>'`。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;
use std::path::{Component, Path, PathBuf};
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
    for method in [
        "readdirSync",
        "statSync",
        "mkdirSync",
        "rmSync",
        "renameSync",
        "unlinkSync",
        "copyFileSync",
        "appendFileSync",
        "realpathSync",
    ] {
        let fn_ref = vm.alloc_native_fn(&format!("fs.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    register_handler(registry, "fs", "readdirSync", readdir_sync);
    register_handler(registry, "fs", "statSync", stat_sync);
    register_handler(registry, "fs", "mkdirSync", mkdir_sync);
    register_handler(registry, "fs", "rmSync", rm_sync);
    register_handler(registry, "fs", "renameSync", rename_sync);
    register_handler(registry, "fs", "unlinkSync", unlink_sync);
    register_handler(registry, "fs", "copyFileSync", copy_file_sync);
    register_handler(registry, "fs", "appendFileSync", append_file_sync);
    register_handler(registry, "fs", "realpathSync", realpath_sync);
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
        .map_err(|e| fs_error(vm, &e, "scandir", &path, None))?
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
    let meta = std::fs::metadata(&path).map_err(|e| fs_error(vm, &e, "stat", &path, None))?;
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
    res.map_err(|e| fs_error(vm, &e, "mkdir", &path, None))?;
    Ok(Value::Undefined)
}

/// `rmSync(path[, {recursive, force}])`：删除文件/目录。
///
/// Node 语义：`recursive` 级联删除目录；`force` 忽略不存在（缺省 false 时
/// 不存在路径抛 ENOENT）。此前实现对 NotFound 一律吞掉（Go `os.RemoveAll`
/// 口径），与 Node 在未传 `force` 时不一致。
fn rm_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let path = arg_string(vm, args, 0);
    if path.is_empty() {
        return Ok(Value::Undefined);
    }
    let opts = args.get(1).copied().unwrap_or(Value::Undefined);
    let recursive = options_flag(vm, opts, "recursive");
    let force = options_flag(vm, opts, "force");
    let exists = std::fs::symlink_metadata(&path).is_ok();
    if !exists && force {
        return Ok(Value::Undefined);
    }
    let res = if recursive {
        std::fs::remove_dir_all(&path)
    } else if std::fs::metadata(&path)
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        std::fs::remove_dir(&path)
    } else {
        std::fs::remove_file(&path)
    };
    res.map_err(|e| fs_error(vm, &e, "rm", &path, None))?;
    Ok(Value::Undefined)
}

/// 解析第二参数：`{ recursive: true }` 或裸布尔（简化形态）。
fn options_recursive(vm: &mut Vm, opt: Value) -> bool {
    options_flag(vm, opt, "recursive")
}

/// 解析选项对象上的布尔旗标（`{ recursive }` / `{ force }`）。
fn options_flag(vm: &mut Vm, opt: Value, key: &str) -> bool {
    match opt.case() {
        ValueCase::Boolean(b) => b,
        ValueCase::Object(r) => {
            let v = vm
                .get_property(Value::Object(r), key)
                .unwrap_or(Value::Undefined);
            vm.truthy(v)
        }
        _ => false,
    }
}

/// 取第 `idx` 个位置参数并转字符串（缺参 → 空串，与既有 fs 族一致）。
fn arg_string(vm: &mut Vm, args: &[Value], idx: usize) -> String {
    args.get(idx)
        .map(|v| vm.format_value(*v))
        .unwrap_or_default()
}

/// `renameSync(oldPath, newPath)`：重命名 / 跨目录移动（同卷原子替换）。
fn rename_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let from = arg_string(vm, args, 0);
    let to = arg_string(vm, args, 1);
    std::fs::rename(&from, &to).map_err(|e| fs_error(vm, &e, "rename", &from, Some(&to)))?;
    Ok(Value::Undefined)
}

/// `unlinkSync(path)`：删除文件（对目录报 EISDIR/EPERM，与平台/Node 一致）。
fn unlink_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let path = arg_string(vm, args, 0);
    std::fs::remove_file(&path).map_err(|e| fs_error(vm, &e, "unlink", &path, None))?;
    Ok(Value::Undefined)
}

/// `copyFileSync(src, dest[, mode])`：复制文件（mode 位忽略，仅形态兼容）。
fn copy_file_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let src = arg_string(vm, args, 0);
    let dest = arg_string(vm, args, 1);
    std::fs::copy(&src, &dest).map_err(|e| {
        // Node 的 copyfile 错误：message 走 `copyfile 'src' -> 'dest'`，
        // 而 `err.path` 指向**目标**、`err.dest` 指向源。
        let err = fs_error_object(vm, &e, "copyfile", &src, Some(&dest));
        let dest_abs = abs_display(&dest);
        let dest_v = Value::Object(vm.alloc_string(dest_abs));
        let _ = vm.set_property(Value::Object(err), "path", dest_v);
        let src_abs = abs_display(&src);
        let src_v = Value::Object(vm.alloc_string(src_abs));
        let _ = vm.set_property(Value::Object(err), "dest", src_v);
        VmError::Thrown(Value::Object(err))
    })?;
    Ok(Value::Undefined)
}

/// `appendFileSync(file, data[, options])`：追加写入（不存在则新建，不建目录）。
fn append_file_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    use std::io::Write;
    let file = arg_string(vm, args, 0);
    let data = arg_string(vm, args, 1);
    let mut handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(|e| fs_error(vm, &e, "open", &file, None))?;
    handle
        .write_all(data.as_bytes())
        .map_err(|e| fs_error(vm, &e, "write", &file, None))?;
    Ok(Value::Undefined)
}

/// `realpathSync(path)`：规范化的绝对真实路径。
///
/// Windows 上 `std::fs::canonicalize` 返回 `\\?\C:\...` 扩展前缀形态，
/// Node（libuv）返回去前缀的普通绝对路径 → 此处剥离以对齐。
fn realpath_sync(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let path = arg_string(vm, args, 0);
    let canonical =
        std::fs::canonicalize(&path).map_err(|e| fs_error(vm, &e, "realpath", &path, None))?;
    let text = canonical.to_string_lossy().into_owned();
    let stripped = text
        .strip_prefix(r"\\?\")
        .map(str::to_owned)
        .unwrap_or(text);
    Ok(Value::Object(vm.alloc_string(stripped)))
}

/// Node 风格 SystemError 的 `errno` 占位值。
///
/// Node 的 `errno` 是 libuv 平台相关负值（Windows 与 Linux 各不相同），
/// 本实现不逐值对齐；判定语义请用 `err.code`（已逐字对齐）。
const FS_ERRNO_PLACEHOLDER: f64 = -4094.0;

/// `io::ErrorKind` → Node 的 `(code, 描述文案)`（libuv 错误表文案逐字对齐）。
fn errno_code_desc(kind: std::io::ErrorKind) -> (&'static str, &'static str) {
    use std::io::ErrorKind as K;
    match kind {
        K::NotFound => ("ENOENT", "no such file or directory"),
        K::PermissionDenied => ("EACCES", "permission denied"),
        K::AlreadyExists => ("EEXIST", "file already exists"),
        K::NotADirectory => ("ENOTDIR", "not a directory"),
        K::IsADirectory => ("EISDIR", "illegal operation on a directory"),
        K::DirectoryNotEmpty => ("ENOTEMPTY", "directory not empty"),
        K::InvalidInput | K::InvalidData => ("EINVAL", "invalid argument"),
        K::CrossesDevices => ("EXDEV", "cross-device link not permitted"),
        K::StorageFull => ("ENOSPC", "no space left on device"),
        K::ReadOnlyFilesystem => ("EROFS", "read-only file system"),
        K::ResourceBusy => ("EBUSY", "resource busy or locked"),
        K::TooManyLinks => ("EMLINK", "too many links"),
        K::FileTooLarge => ("EFBIG", "file too large"),
        K::ArgumentListTooLong => ("E2BIG", "argument list too long"),
        K::InvalidFilename => ("EINVAL", "invalid argument"),
        _ => ("UNKNOWN", "unknown error"),
    }
}

/// 相对路径 → 绝对路径并做词法归一（折叠 `.`/`..`），对齐 Node message 中
/// 的路径形态（Node 打印 `path.resolve()` 结果）。
pub(crate) fn abs_display(path: &str) -> String {
    let p = Path::new(path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p)
    };
    let mut out = PathBuf::new();
    for comp in abs.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().into_owned()
}

/// 构造 Node 风格 fs 错误对象（Error 实例 + code/errno/syscall/path[/dest]）。
///
/// `path`/`dest` 为 message 中的顺序（`<syscall> 'path' -> 'dest'`）。
pub(crate) fn fs_error_object(
    vm: &mut Vm,
    err: &std::io::Error,
    syscall: &str,
    path: &str,
    dest: Option<&str>,
) -> ObjectRef {
    let (code, desc) = errno_code_desc(err.kind());
    let path_abs = abs_display(path);
    let mut message = format!("{code}: {desc}, {syscall} '{path_abs}'");
    if let Some(d) = dest {
        message.push_str(&format!(" -> '{}'", abs_display(d)));
    }
    let instance = vm.alloc_error_instance(&message);
    vm.attach_error_proto(instance, "Error");
    let code_v = Value::Object(vm.alloc_string(code.to_owned()));
    let _ = vm.set_property(Value::Object(instance), "code", code_v);
    let errno_v = Value::Number(FS_ERRNO_PLACEHOLDER);
    let _ = vm.set_property(Value::Object(instance), "errno", errno_v);
    let syscall_v = Value::Object(vm.alloc_string(syscall.to_owned()));
    let _ = vm.set_property(Value::Object(instance), "syscall", syscall_v);
    let path_v = Value::Object(vm.alloc_string(path_abs));
    let _ = vm.set_property(Value::Object(instance), "path", path_v);
    if let Some(d) = dest {
        let dest_v = Value::Object(vm.alloc_string(abs_display(d)));
        let _ = vm.set_property(Value::Object(instance), "dest", dest_v);
    }
    instance
}

/// [`fs_error_object`] 的 `VmError` 包装（供 `map_err` 直接使用）。
pub(crate) fn fs_error(
    vm: &mut Vm,
    err: &std::io::Error,
    syscall: &str,
    path: &str,
    dest: Option<&str>,
) -> VmError {
    VmError::Thrown(Value::Object(fs_error_object(vm, err, syscall, path, dest)))
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
        let _: crate::builtins::BuiltinHandler = rename_sync;
        let _: crate::builtins::BuiltinHandler = unlink_sync;
        let _: crate::builtins::BuiltinHandler = copy_file_sync;
        let _: crate::builtins::BuiltinHandler = append_file_sync;
        let _: crate::builtins::BuiltinHandler = realpath_sync;
        let _: crate::builtins::BuiltinHandler = stat_is_file;
        let _: crate::builtins::BuiltinHandler = stat_is_directory;
    }

    /// 错误码映射锚定：Node（libuv）文案逐字对齐（真实项目按 `err.code` 分支）。
    #[test]
    fn errno_mapping_matches_node_libuv_table() {
        use std::io::ErrorKind as K;
        assert_eq!(
            errno_code_desc(K::NotFound),
            ("ENOENT", "no such file or directory")
        );
        assert_eq!(
            errno_code_desc(K::AlreadyExists),
            ("EEXIST", "file already exists")
        );
        assert_eq!(
            errno_code_desc(K::PermissionDenied),
            ("EACCES", "permission denied")
        );
        assert_eq!(
            errno_code_desc(K::NotADirectory),
            ("ENOTDIR", "not a directory")
        );
        assert_eq!(
            errno_code_desc(K::IsADirectory),
            ("EISDIR", "illegal operation on a directory")
        );
        assert_eq!(errno_code_desc(K::Other), ("UNKNOWN", "unknown error"));
    }

    /// 绝对化 + 词法归一：`a/../b` → `<cwd>/b`（对齐 Node message 路径形态）。
    #[test]
    fn abs_display_folds_lexically() {
        let cwd = std::env::current_dir().expect("cwd");
        let got = abs_display("a/../b");
        let want = cwd.join("b").to_string_lossy().into_owned();
        assert_eq!(got, want);
        let abs = cwd.to_string_lossy().into_owned();
        assert_eq!(abs_display(&abs), abs, "绝对路径应原样保留");
    }
}
