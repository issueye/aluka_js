//! `path/win32` 内置模块：Win32（`\`）分隔符语义的路径操作。
//!
//! 语义实测对齐 Node.js 22 LTS 标准（`nodeos.NewPathWin32`）→ Go 标准库 `path/filepath`
//! （Windows 平台版，go1.25 `filepathlite`）：
//! - `/` 与 `\` 同为分隔符，输出恒为 `\`；
//! - `join`/`clean` 保留卷名（`C:` 驱动相对不加分隔符；`C:\x` 绝对化）；
//! - `resolve` 对齐 `filepath.Abs`（Windows 走 `GetFullPathName` 语义：
//!   相对路径基于当前工作目录，驱动相对路径基于该驱动上的 cwd）；
//! - `basename`/`dirname`/`extname` 移植 `filepathlite`（含 `postClean`：
//!   `a/../c:` → `.\c:` 防相对路径被卷解析劫持）。

use crate::builtins::{
    BuiltinHandler, BuiltinRegistry, ModuleDef, register_handler, set_module_prop,
};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// Windows 分隔符 path 模块（join/basename/dirname/extname/resolve）。
pub const MODULE: ModuleDef = ModuleDef {
    name: "path/win32",
    build,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    for (name, handler) in METHODS {
        let f = vm.alloc_native_fn(&format!("path/win32.{name}"));
        set_module_prop(vm, obj, name, Value::Object(f))?;
        register_handler(registry, "path/win32", name, *handler);
    }
    // `path/win32` 模块自身的 `sep`/`delimiter`（`require('path/win32').sep`）
    let sep_v = Value::Object(vm.alloc_string(SEP.to_owned()));
    let delim_v = Value::Object(vm.alloc_string(DELIMITER.to_owned()));
    set_module_prop(vm, obj, "sep", sep_v)?;
    set_module_prop(vm, obj, "delimiter", delim_v)?;
    Ok(obj)
}

/// `path.parse(p)` → `{ root, dir, base, ext, name }`。
///
/// 由既有 dirname/basename/extname 助手组装（同平台语义单一来源）：
/// `dir` 为 `.` 时按 Node 语义归空串；`name` = `base` 去除 `ext` 尾。
fn parse(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let p = match args.first() {
        Some(v) => vm.format_value(*v),
        None => String::new(),
    };
    let root = {
        let b = p.as_bytes();
        // 卷根前缀：盘符（`C:` / `C:\`）、UNC/设备根、单分隔符
        if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
            let n = if b.len() >= 3 && is_sep(b[2]) { 3 } else { 2 };
            p[..n].to_owned()
        } else if b.len() >= 2 && is_sep(b[0]) && is_sep(b[1]) {
            // UNC/设备根：\server\share\ 或 \\.\x\
            let mut n = 2;
            let mut seen = 0;
            while n < b.len() && seen < 2 {
                if is_sep(b[n]) {
                    seen += 1;
                    if seen == 2 {
                        n += 1;
                        break;
                    }
                }
                n += 1;
            }
            p[..n.min(p.len())].to_owned()
        } else if !b.is_empty() && is_sep(b[0]) {
            p[..1].to_owned()
        } else {
            String::new()
        }
    }
    .to_owned();
    let dir = {
        let src_val = Value::Object(vm.alloc_string(p.clone()));
        let d = dirname(vm, &[src_val])?;
        let text = vm.format_value(d);
        if text == "." { String::new() } else { text }
    };
    let base_src = Value::Object(vm.alloc_string(p.clone()));
    let base_val = basename(vm, &[base_src])?;
    let base = vm.format_value(base_val);
    let ext = if base.is_empty() {
        String::new()
    } else {
        let ext_src = Value::Object(vm.alloc_string(base.clone()));
        let e = extname(vm, &[ext_src])?;
        vm.format_value(e)
    };
    let name = base.strip_suffix(ext.as_str()).unwrap_or(&base).to_owned();

    let out = vm.alloc_ordinary();
    let set = |vm: &mut Vm, k: &str, v: String| -> Result<(), VmError> {
        let s = vm.alloc_string(v);
        vm.set_property(Value::Object(out), k, Value::Object(s))?;
        Ok(())
    };
    set(vm, "root", root)?;
    set(vm, "dir", dir)?;
    set(vm, "base", base)?;
    set(vm, "ext", ext)?;
    set(vm, "name", name)?;
    Ok(Value::Object(out))
}

/// `path.format(obj)`：`dir`/`root` + `base`（或 `name` + `ext`）重组。
///
/// Node 算法：`dir` 优先（未以分隔符收尾则补一个），否则用 `root`；
/// `base` 优先于 `name` + `ext`。
fn format(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(obj) = args.first().copied() else {
        return Ok(Value::Object(vm.alloc_string(String::new())));
    };
    let text_of = |vm: &mut Vm, key: &str| -> Result<String, VmError> {
        let v = vm.get_property(obj, key)?;
        Ok(vm.format_value(v))
    };
    let dir = text_of(vm, "dir")?;
    let root = text_of(vm, "root")?;
    let base = text_of(vm, "base")?;
    let name = text_of(vm, "name")?;
    let ext = text_of(vm, "ext")?;

    let mut out = String::new();
    if !dir.is_empty() {
        out.push_str(&dir);
        let dir_ends_sep = dir.as_bytes().last().is_some_and(|&b| is_sep(b));
        if !dir_ends_sep {
            out.push('\\');
        }
    } else if !root.is_empty() {
        out.push_str(&root);
    }
    if !base.is_empty() {
        out.push_str(&base);
    } else {
        if !name.is_empty() {
            out.push_str(&name);
        }
        if !ext.is_empty() && (ext.starts_with('.') || !name.is_empty()) {
            out.push_str(&ext);
        }
    }
    Ok(Value::Object(vm.alloc_string(out)))
}

/// Windows 分隔符语义的方法表（`path/win32` 与平台 `path` 共用同一实现，
/// 仅 NativeFn 名前缀不同——由 [`crate::builtins::register_all`] 逐项转挂）。
pub(crate) const METHODS: &[(&str, BuiltinHandler)] = &[
    ("join", join),
    ("normalize", normalize),
    ("relative", relative),
    ("basename", basename),
    ("dirname", dirname),
    ("extname", extname),
    ("resolve", resolve),
    ("isAbsolute", is_absolute),
    ("parse", parse),
    ("format", format),
];

/// `isAbsolute(p)`：win32 语义——卷根起始（`C:\x`）、UNC 根
/// （`\\host\share\x`）、或单分隔符起始（`\x` / `/x`）皆为绝对。
/// `C:a` 是驱动相对路径，**不是**绝对路径。
fn is_absolute(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let path = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    Ok(Value::Boolean(win_is_absolute(&path)))
}

/// Node `win32.isAbsolute`。
fn win_is_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    if b.is_empty() {
        return false;
    }
    if is_sep(b[0]) {
        if b.len() == 1 {
            return true;
        }
        if is_sep(b[1]) {
            // UNC：跳过 `\\` 与主机段，再跳过分隔符与共享段，余下非空才算绝对
            let mut i = 2usize;
            while i < b.len() && !is_sep(b[i]) {
                i += 1;
            }
            if i == b.len() {
                return false;
            }
            while i < b.len() && is_sep(b[i]) {
                i += 1;
            }
            if i == b.len() {
                return false;
            }
            while i < b.len() && !is_sep(b[i]) {
                i += 1;
            }
            return i != b.len();
        }
        return true;
    }
    // 设备根：`C:\` `C:/` 绝对；`C:` `C:a` 驱动相对
    b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && is_sep(b[2])
}

/// 当前平台的分隔符 / 路径列表分隔符（`path.sep` / `path.delimiter`）。
pub(crate) const SEP: &str = "\\";
pub(crate) const DELIMITER: &str = ";";

/// `normalize(p)`：Node `win32.normalize` 逐字移植。
///
/// 与 Go `filepath.Clean` 的差异都在可观测输出上：**保留尾部分隔符**
/// （`'a/'` → `'a\\'`）、UNC 根补尾分隔符（`'//srv/share'` →
/// `'\\\\srv\\share\\'`）、无分隔符的 `'C:'` → `'C:.'`，以及
/// CVE-2024-36139 的「非绝对路径改写成可能被 Windows 当绝对路径解释」防护。
fn normalize(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    // Node `validateString(path, 'path')`：缺参/非串一概 TypeError
    // （`path.normalize()` 在 Node 抛 ERR_INVALID_ARG_TYPE，不返回 '.'）
    let Some(first) = args.first() else {
        return Err(
            vm.type_error("The \"path\" argument must be of type string. Received undefined")
        );
    };
    if !vm.is_string_value(*first) {
        let shown = vm.format_value(*first);
        return Err(vm.type_error(&format!(
            "The \"path\" argument must be of type string. Received {shown}"
        )));
    }
    let p = vm.format_value(*first);
    Ok(Value::Object(vm.alloc_string(win_normalize(&p))))
}

/// Node `win32.normalize`（`lib/path.js`）。
fn win_normalize(path: &str) -> String {
    let b = path.as_bytes();
    let len = b.len();
    if len == 0 {
        return ".".to_owned();
    }
    if len == 1 {
        // 单字符：POSIX 分隔符归一为 `\`，否则原样
        return if b[0] == b'/' {
            "\\".to_owned()
        } else {
            path.to_owned()
        };
    }

    let mut root_end = 0usize;
    let mut device: Option<String> = None;
    let mut is_absolute = false;
    let code = b[0];

    if is_sep(code) {
        is_absolute = true;
        if is_sep(b[1]) {
            // 可能的 UNC 根
            let mut j = 2usize;
            let mut last = j;
            while j < len && !is_sep(b[j]) {
                j += 1;
            }
            if j < len && j != last {
                let first_part = &path[last..j];
                last = j;
                while j < len && is_sep(b[j]) {
                    j += 1;
                }
                if j < len && j != last {
                    last = j;
                    while j < len && !is_sep(b[j]) {
                        j += 1;
                    }
                    if j == len || j != last {
                        if first_part == "." || first_part == "?" {
                            // 设备根（`\\.\PHYSICALDRIVE0`）
                            device = Some(format!("\\\\{first_part}"));
                            root_end = 4;
                            if let Some(colon) = path.find(':') {
                                let possible = &path[4..colon + 1];
                                if is_windows_reserved_name(possible, possible.len() as isize - 1) {
                                    device = Some(format!("\\\\?\\{possible}"));
                                    root_end = 4 + possible.len();
                                }
                            }
                        } else if j == len {
                            // 恰好是 UNC 根本身：补尾分隔符后返回
                            return format!("\\\\{first_part}\\{}\\", &path[last..]);
                        } else {
                            // UNC 根 + 余部
                            device = Some(format!("\\\\{first_part}\\{}", &path[last..j]));
                            root_end = j;
                        }
                    }
                }
            }
        } else {
            root_end = 1;
        }
    } else {
        let colon_index = match path.find(':') {
            Some(i) => i as isize,
            None => -1,
        };
        if colon_index > 0 {
            if b[0].is_ascii_alphabetic() && colon_index == 1 {
                device = Some(path[..2].to_owned());
                root_end = 2;
                if len > 2 && is_sep(b[2]) {
                    is_absolute = true;
                    root_end = 3;
                }
            } else if is_windows_reserved_name(path, colon_index) {
                device = Some(path[..colon_index as usize + 1].to_owned());
                root_end = colon_index as usize + 1;
            }
        }
    }

    let mut tail = if root_end < len {
        win_normalize_string(&path[root_end..], !is_absolute)
    } else {
        String::new()
    };
    if tail.is_empty() && !is_absolute {
        tail = ".".to_owned();
    }
    if !tail.is_empty() && is_sep(b[len - 1]) {
        tail.push('\\');
    }
    if !is_absolute && device.is_none() && path.contains(':') {
        // CVE-2024-36139：尾串形如 `C:` 时必须前缀 `.\`，否则 Windows 会
        // 把它解释成绝对路径
        if tail.len() >= 2 && tail.as_bytes()[0].is_ascii_alphabetic() && tail.as_bytes()[1] == b':'
        {
            return format!(".\\{tail}");
        }
        let mut index = path.find(':');
        while let Some(i) = index {
            if i == len - 1 || is_sep(b[i + 1]) {
                return format!(".\\{tail}");
            }
            index = path[i + 1..].find(':').map(|k| k + i + 1);
        }
    }
    if let Some(colon_index) = path.find(':') {
        if is_windows_reserved_name(path, colon_index as isize) {
            let d = device.clone().unwrap_or_default();
            return format!(".\\{d}{tail}");
        }
    }
    match device {
        None => {
            if is_absolute {
                format!("\\{tail}")
            } else {
                tail
            }
        }
        Some(d) => {
            if is_absolute {
                format!("{d}\\{tail}")
            } else {
                format!("{d}{tail}")
            }
        }
    }
}

/// `normalizeString(path, allowAboveRoot, '\\', isPathSeparator)` 的 Windows
/// 实例化（与 POSIX 版同构，仅分隔符谓词不同，输出以 `\` 连接）。
fn win_normalize_string(path: &str, allow_above_root: bool) -> String {
    let mut resolved: Vec<&str> = Vec::new();
    for raw in path.split(is_sep_c) {
        match raw {
            "" | "." => {}
            ".." => match resolved.last() {
                Some(&last) if last != ".." => {
                    resolved.pop();
                }
                _ => {
                    if allow_above_root {
                        resolved.push("..");
                    }
                }
            },
            other => resolved.push(other),
        }
    }
    resolved.join("\\")
}

/// `isWindowsReservedName(name, colonIndex)`：`CON`/`PRN`/`AUX`/`NUL`/
/// `COM1`–`COM9`/`LPT1`–`LPT9`（Node `lib/path.js` 的保留设备名表）。
fn is_windows_reserved_name(name: &str, colon_index: isize) -> bool {
    if colon_index < 0 {
        return false;
    }
    let end = colon_index as usize;
    let Some(part) = name.get(..end) else {
        return false;
    };
    if part.is_empty() {
        return false;
    }
    let upper = part.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    if upper.len() != 4 {
        return false;
    }
    let head = &upper[..3];
    let digit = upper.as_bytes()[3];
    matches!(head, "COM" | "LPT") && (b'1'..=b'9').contains(&digit)
}

/// `relative(from, to)`：先 `Clean` 再按段求差（Node `path.relative`）。
fn relative(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let from = win_normalize(
        &args
            .first()
            .map(|v| vm.format_value(*v))
            .unwrap_or_default(),
    );
    let to = win_normalize(&args.get(1).map(|v| vm.format_value(*v)).unwrap_or_default());
    Ok(Value::Object(vm.alloc_string(win_relative(&from, &to))))
}

/// Windows `relative`：`\`/`/` 均作分隔符，输出以 `\` 连接
///（Node 用 `toNamespacedPath` 之外的同一段差分算法）。
fn win_relative(from: &str, to: &str) -> String {
    let segs = |s: &str| -> Vec<String> {
        s.split(is_sep_c)
            .filter(|x| !x.is_empty() && *x != ".")
            .map(str::to_owned)
            .collect()
    };
    let fs = segs(from);
    let ts = segs(to);
    let mut common = 0usize;
    while common < fs.len() && common < ts.len() && fs[common].eq_ignore_ascii_case(&ts[common]) {
        common += 1;
    }
    let mut out: Vec<String> = Vec::new();
    for _ in common..fs.len() {
        out.push("..".to_owned());
    }
    out.extend(ts[common..].iter().cloned());
    if out.is_empty() {
        // Node：同路径 → 空串（POSIX 的 `relative` 同样返回 `''`，
        // 此前本实现返回 `'.'`）
        return String::new();
    }
    out.join("\\")
}

/// `split(s)` 接受的分隔符谓词（`u8` 形态，供 `str::split`）。
fn is_sep_c(c: char) -> bool {
    c == '\\' || c == '/'
}

/// `join(...parts)`：`filepath.Join`（走 Go windows join：首元素卷保留、
/// 尾部分隔符剥离、驱动相对不插分隔符，结果 Clean）。
fn join(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let elems: Vec<String> = args.iter().map(|v| vm.format_value(*v)).collect();
    let s = vm.alloc_string(win_join(&elems));
    Ok(Value::Object(s))
}

/// `basename(p[, ext])`：Node `win32.basename` 逐字移植（原串切片；
/// `''` / 全分隔符 → `''`；suffix 逐字符比对回退）。
fn basename(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(first) = args.first() else {
        return Ok(Value::Object(vm.alloc_string(String::new())));
    };
    let path = vm.format_value(*first);
    let suffix = args.get(1).map(|v| vm.format_value(*v));
    Ok(Value::Object(vm.alloc_string(
        crate::builtins::path_node::node_basename(
            &path,
            suffix.as_deref(),
            crate::builtins::path_node::is_win_sep,
            true,
        ),
    )))
}

/// `dirname(p)`：Split 后 Clean（`"file.txt"` → `"."`；`"C:foo"` → `"C:."`）。
fn dirname(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let p = match args.first() {
        Some(v) => vm.format_value(*v),
        None => return Ok(Value::Object(vm.alloc_string(".".to_owned()))),
    };
    let s = vm.alloc_string(node_win_dirname(&p));
    Ok(Value::Object(s))
}

/// Node `path.win32.dirname` 的逐字移植（`lib/path.js` win32 `dirname`）。
///
/// 与 Go `filepathlite.Dir` 的关键差异：Node 返回**原串切片**，保留输入
/// 里分隔符的原始写法（`'a/b/c'` → `'a/b'`），并显式处理 UNC/设备卷根
/// （`'C:\a'` → `'C:\'`、`'\\srv\sh\f'` → `'\\srv\sh\'`）。
fn node_win_dirname(path: &str) -> String {
    let b = path.as_bytes();
    let len = b.len();
    if len == 0 {
        return ".".to_owned();
    }
    // 仅一个分隔符：直接返回（避免落到下面的 "." 分支）
    if len == 1 {
        return if is_sep(b[0]) {
            path.to_owned()
        } else {
            ".".to_owned()
        };
    }

    let mut root_end: isize = -1;
    let mut offset = 0usize;
    if is_sep(b[0]) {
        // UNC 根：`\\host\share`
        root_end = 1;
        offset = 1;
        if is_sep(b[1]) {
            let mut j = 2usize;
            let mut last = j;
            while j < len && !is_sep(b[j]) {
                j += 1;
            }
            if j < len && j != last {
                last = j;
                while j < len && is_sep(b[j]) {
                    j += 1;
                }
                if j < len && j != last {
                    last = j;
                    while j < len && !is_sep(b[j]) {
                        j += 1;
                    }
                    if j == len {
                        // 恰好是 UNC 根本身
                        return path.to_owned();
                    }
                    if j != last {
                        // UNC 根 + 余部：跨越根后分隔符，按「普通根」处理
                        root_end = (j + 1) as isize;
                        offset = j + 1;
                    }
                }
            }
        }
    } else if is_windows_device_root(b[0]) && b[1] == b':' {
        // 设备根 `C:` / `C:\`
        let re: isize = if len > 2 && is_sep(b[2]) { 3 } else { 2 };
        root_end = re;
        offset = re as usize;
    }

    let mut end: isize = -1;
    let mut matched_slash = true;
    let mut i = len as isize - 1;
    while i >= offset as isize {
        if is_sep(b[i as usize]) {
            if !matched_slash {
                end = i;
                break;
            }
        } else {
            matched_slash = false;
        }
        i -= 1;
    }
    if end == -1 {
        if root_end == -1 {
            return ".".to_owned();
        }
        end = root_end;
    }
    path[..end as usize].to_owned()
}

/// `isWindowsDeviceRoot`：`A`–`Z` / `a`–`z`。
fn is_windows_device_root(c: u8) -> bool {
    c.is_ascii_alphabetic()
}

/// `extname(p)`：Node `win32.extname` 逐字移植（`preDotState` 状态机；
/// `'..'` 与首点隐藏文件 → `''`）。
fn extname(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let path = match args.first() {
        Some(v) => vm.format_value(*v),
        None => return Ok(Value::Object(vm.alloc_string(String::new()))),
    };
    Ok(Value::Object(vm.alloc_string(
        crate::builtins::path_node::node_extname(&path, crate::builtins::path_node::is_win_sep),
    )))
}

/// `resolve(...parts)`：`filepath.Abs(filepath.Join(...))`（Windows：
/// `GetFullPathName` 语义，相对路径基于当前工作目录）。
fn resolve(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let elems: Vec<String> = args.iter().map(|v| vm.format_value(*v)).collect();
    // 无参 resolve() 直接为 cwd（Go filepath.Abs 语义，不带尾 `.`）
    let joined = if elems.is_empty() {
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    } else {
        win_abs(&win_join(&elems))
    };
    let s = vm.alloc_string(joined);
    Ok(Value::Object(s))
}

// ---- Go 标准库 `path/filepath`（Windows）移植 ----

#[inline]
fn is_sep(c: u8) -> bool {
    c == b'\\' || c == b'/'
}

/// `filepathlite.VolumeNameLen`（Windows）：驱动符 2 字节；UNC `\\host\share`；
/// `\\.\`/`\\?\`/`\??\` 设备路径。
fn volume_name_len(b: &[u8]) -> usize {
    if b.len() >= 2 && b[1] == b':' {
        return 2;
    }
    if b.is_empty() || !is_sep(b[0]) {
        return 0;
    }
    let upper = |c: u8| if c.is_ascii_lowercase() { c - 32 } else { c };
    let has_fold_prefix = |prefix: &[u8]| -> bool {
        if b.len() < prefix.len() {
            return false;
        }
        for (i, &pc) in prefix.iter().enumerate() {
            if is_sep(pc) {
                if !is_sep(b[i]) {
                    return false;
                }
            } else if upper(pc) != upper(b[i]) {
                return false;
            }
        }
        if b.len() > prefix.len() && !is_sep(b[prefix.len()]) {
            return false;
        }
        true
    };
    if has_fold_prefix(b"\\\\.\\UNC") {
        return unc_len(b, b"\\\\.\\UNC\\".len());
    }
    if has_fold_prefix(b"\\.") || has_fold_prefix(b"\\\\?") || has_fold_prefix(b"\\??") {
        if b.len() == 3 {
            return 3; // 恰好 \\. 
        }
        let mut rest = 4usize;
        while rest < b.len() && !is_sep(b[rest]) {
            rest += 1;
        }
        if rest >= b.len() {
            return b.len();
        }
        return rest + 1;
    }
    if b.len() >= 2 && is_sep(b[1]) {
        // \\host\share
        return unc_len(b, 2);
    }
    0
}

/// `uncLen`：自 prefix 起数到第二个分隔符为止的卷前缀长度。
fn unc_len(b: &[u8], prefix_len: usize) -> usize {
    let mut count = 0usize;
    let mut i = prefix_len;
    while i < b.len() {
        if is_sep(b[i]) {
            count += 1;
            if count == 2 {
                return i;
            }
        }
        i += 1;
    }
    b.len()
}

/// `filepathlite.lazybuf` 逐字移植（含卷前缀拼接语义）。
struct LazyBuf<'a> {
    s: &'a [u8],
    buf: Vec<u8>,
    w: usize,
}

impl<'a> LazyBuf<'a> {
    fn new(s: &'a [u8]) -> Self {
        Self {
            s,
            buf: Vec::new(),
            w: 0,
        }
    }

    fn index(&self, i: usize) -> u8 {
        if self.buf.is_empty() {
            self.s[i]
        } else {
            self.buf[i]
        }
    }

    fn append(&mut self, c: u8) {
        if !self.buf.is_empty() {
            self.buf[self.w] = c;
            self.w += 1;
        } else if self.w < self.s.len() && self.s[self.w] == c {
            self.w += 1;
        } else {
            let mut b = vec![0u8; self.s.len()];
            b[..self.w].copy_from_slice(&self.s[..self.w]);
            b[self.w] = c;
            self.buf = b;
            self.w += 1;
        }
    }
}

/// `filepathlite.Clean`（Windows）：卷保留 + `\` 输出 + `postClean`。
fn win_clean(p: &str) -> String {
    let original = p.as_bytes();
    let vol_len = volume_name_len(original);
    let path = &original[vol_len..];
    if path.is_empty() {
        if vol_len > 1 && is_sep(original[0]) && is_sep(original[1]) {
            // UNC 卷恰好就是路径 → 仅把斜杠归一
            return path_bytes_to_string(&from_slash(&original[..vol_len]));
        }
        return format!("{p}.");
    }
    let rooted = is_sep(path[0]);
    let n = path.len();
    let mut out = LazyBuf::new(path);
    let mut dotdot = 0usize;
    let mut r = 0usize;
    if rooted {
        out.append(b'\\');
        r = 1;
        dotdot = 1;
    }
    while r < n {
        let c = path[r];
        if is_sep(c) {
            r += 1;
        } else if c == b'.' && (r + 1 == n || is_sep(path[r + 1])) {
            // "." 元素：整体跳过（Go `filepathlite.Clean` 的
            // `path[r] == '.' && (r+1 == n || IsPathSeparator(path[r+1]))`
            // 分支——漏掉它会让 `filepath.Join("a",".","b")` 输出 `a\.\b`）
            r += 1;
        } else if c == b'.'
            && r + 1 < n
            && path[r + 1] == b'.'
            && (r + 2 == n || is_sep(path[r + 2]))
        {
            r += 2;
            if out.w > dotdot {
                // 可回退：先退一位再从边界位读回（Go lazybuf 语义）
                out.w -= 1;
                while out.w > dotdot && !is_sep(out.index(out.w)) {
                    out.w -= 1;
                }
            } else if !rooted {
                if out.w > 0 {
                    out.append(b'\\');
                }
                out.append(b'.');
                out.append(b'.');
                dotdot = out.w;
            }
        } else {
            if (rooted && out.w != 1) || (!rooted && out.w != 0) {
                out.append(b'\\');
            }
            while r < n && !is_sep(path[r]) {
                out.append(path[r]);
                r += 1;
            }
        }
    }
    if out.w == 0 {
        out.append(b'.');
    }
    // 物化输出缓冲（postClean 需要扫描；Go 仅对已物化缓冲生效）
    let body: Vec<u8> = if out.buf.is_empty() {
        out.s[..out.w].to_vec()
    } else {
        out.buf[..out.w].to_vec()
    };
    let mut body = body;
    if vol_len == 0 {
        post_clean(&mut body);
    }
    let mut result = from_slash(&original[..vol_len]);
    result.extend_from_slice(&body);
    path_bytes_to_string(&result)
}

/// `postClean`：防止相对路径被卷解析劫持（`a/../c:` → `.\c:`；`\a\..\??\..` → `\.\??\..`）。
fn post_clean(out: &mut Vec<u8>) {
    for &c in out.iter() {
        if is_sep(c) {
            break;
        }
        if c == b':' {
            out.splice(0..0, [b'.', b'\\']);
            return;
        }
    }
    if out.len() >= 3 && is_sep(out[0]) && out[1] == b'?' && out[2] == b'?' {
        out.splice(0..0, [b'\\', b'.']);
    }
}

/// `filepathlite.FromSlash`：`/` → `\`。
fn from_slash(b: &[u8]) -> Vec<u8> {
    b.iter()
        .map(|&c| if c == b'/' { b'\\' } else { c })
        .collect()
}

fn path_bytes_to_string(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap_or_default()
}

/// Node `path.win32.join`（`lib/path.js`）：空元素跳过 → 以 `\` 相连 →
/// 防 UNC 误判的首部斜杠压缩 → 保留设备名时只做分隔符归一 → 否则
/// `win32.normalize`。
///
/// 关键点：**结果过 normalize**，因此尾部分隔符会被保留
/// （`join('a/')` → `'a\\'`），UNC 根会补尾分隔符
/// （`join('//srv','share')` → `'\\\\srv\\share\\'`）。Go 的
/// `filepath.Join` 在同样输入下分别给 `'a'` 与 `'\\\\srv\\share'`。
fn win_join(elems: &[String]) -> String {
    let parts: Vec<&String> = elems.iter().filter(|e| !e.is_empty()).collect();
    if parts.is_empty() {
        return ".".to_owned();
    }
    let first_part = parts[0].as_str();
    let joined: String = parts
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\\");

    // 首个非空串以「恰好两个分隔符 + 至少一个非分隔符」开头时，视为用户
    // 有意构造 UNC 路径，不做前导斜杠压缩
    let mut slash_count = 0usize;
    let fb = first_part.as_bytes();
    let mut needs_replace = true;
    if !fb.is_empty() && is_sep(fb[0]) {
        slash_count += 1;
        if fb.len() > 1 && is_sep(fb[1]) {
            slash_count += 1;
            if fb.len() > 2 {
                if is_sep(fb[2]) {
                    slash_count += 1;
                } else {
                    needs_replace = false;
                }
            }
        }
    }
    let mut joined = joined;
    if needs_replace {
        let jb = joined.as_bytes();
        while slash_count < jb.len() && is_sep(jb[slash_count]) {
            slash_count += 1;
        }
        if slash_count >= 2 {
            joined = format!("\\{}", &joined[slash_count..]);
        }
    }

    // 任一段含 Windows 保留设备名时跳过 normalize（`CON`/`COM1` 等）
    let mut segs: Vec<String> = Vec::new();
    let mut cur = String::new();
    let jb = joined.as_bytes();
    let mut i = 0usize;
    while i < jb.len() {
        if jb[i] == b'\\' {
            if !cur.is_empty() {
                segs.push(std::mem::take(&mut cur));
            }
            while i + 1 < jb.len() && jb[i + 1] == b'\\' {
                i += 1;
            }
        } else {
            cur.push(jb[i] as char);
        }
        i += 1;
    }
    if !cur.is_empty() {
        segs.push(cur);
    }
    if segs.iter().any(|p| match p.find(':') {
        Some(ci) => is_windows_reserved_name(p, ci as isize),
        None => false,
    }) {
        // 保留设备名：只把 `/` 归一为 `\`，不做路径折叠
        return joined.replace('/', "\\");
    }

    win_normalize(&joined)
}

/// `filepath.Abs`（Windows）：`GetFullPathName` 语义 + Clean。
///
/// - 卷内根起始（`C:\x`、`\\host\share\x`）→ 直接 Clean；
/// - 无卷根起始（`\x`）→ 当前工作目录所在驱动 + 路径；
/// - 驱动相对（`C:b`）→ 该驱动上的 cwd（同驱动取工作目录，异驱动取驱动根）；
/// - 纯相对 → 工作目录 + 路径。
fn win_abs(p: &str) -> String {
    let path = if p.is_empty() { "." } else { p };
    let b = path.as_bytes();
    let vol_len = volume_name_len(b);
    let rest = &b[vol_len..];
    // 卷内根起始（C:\x、\\host\share\x）→ 直接 Clean；
    // 无卷根起始（\x）属相对，须拼当前驱动
    if vol_len > 0 && !rest.is_empty() && is_sep(rest[0]) {
        return win_clean(path);
    }
    let wd = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let wd_b = wd.as_bytes();
    if vol_len == 0 {
        if !b.is_empty() && is_sep(b[0]) {
            // 根起始无卷：当前驱动 + path
            let drive: &[u8] = if wd_b.len() >= 2 { &wd_b[..2] } else { b"C:" };
            let joined = format!("{}{}", path_bytes_to_string(drive), path);
            return win_clean(&joined);
        }
        if wd_b.is_empty() {
            return win_clean(path);
        }
        let joined = format!("{}\\{path}", wd);
        return win_clean(&joined);
    }
    if vol_len > 2 {
        // 恰好是 UNC 卷本身等：直接 Clean
        return win_clean(path);
    }
    // 驱动相对（"C:b"）
    let drive = b[0].to_ascii_uppercase();
    let wd_drive = if wd_b.len() >= 2 {
        wd_b[0].to_ascii_uppercase()
    } else {
        0
    };
    let tail = path_bytes_to_string(&b[2..]);
    if drive == wd_drive && wd_b.len() >= 2 {
        // 同驱动：该驱动上的 cwd = 工作目录
        let joined = format!("{wd}\\{tail}");
        win_clean(&joined)
    } else {
        // 异驱动：该驱动根
        let joined = format!("{}:\\{tail}", char::from(drive));
        win_clean(&joined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: BuiltinHandler = join;
        let _: BuiltinHandler = basename;
        let _: BuiltinHandler = dirname;
        let _: BuiltinHandler = extname;
        let _: BuiltinHandler = resolve;
    }
}
