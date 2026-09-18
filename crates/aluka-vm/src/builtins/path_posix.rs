//! `path/posix` 内置模块：POSIX（`/`）分隔符语义的路径操作。
//!
//! 语义实测对齐 Node.js 22 LTS 标准（`nodeos.NewPathPosix`）→ Go 标准库 `path` 包：
//! - `join` 空元素跳过、结果 Clean（`.`/`..` 折叠、`//` 归并）；
//! - `basename` 去尾部斜杠；`basename("")` = `"."`（Node.js 22 LTS 标准 口径）；
//! - `dirname` 为 Split 后 Clean（`"file.txt"` → `"."`）；
//! - `extname` 采用 Node 语义（`.bashrc` 首点隐藏文件 → `""`）；
//! - `resolve` 相对路径基于当前工作目录（`filepath.ToSlash` 转正斜杠）。
//!
//! 处理器签名与注册方式照抄 `querystring.rs` 模板；模块对象挂
//! `NativeFn("path/posix.<方法>")` 属性并登记 `path/posix.<方法>` 分派键。

use crate::builtins::{
    BuiltinHandler, BuiltinRegistry, ModuleDef, register_handler, set_module_prop,
};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// POSIX 分隔符 path 模块（join/basename/dirname/extname/resolve）。
pub const MODULE: ModuleDef = ModuleDef {
    name: "path/posix",
    build,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    for (name, handler) in METHODS {
        let f = vm.alloc_native_fn(&format!("path/posix.{name}"));
        set_module_prop(vm, obj, name, Value::Object(f))?;
        register_handler(registry, "path/posix", name, *handler);
    }
    // `path/posix` 模块自身的 `sep`/`delimiter`（`require('path/posix').sep`）
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
    let root = if p.starts_with('/') { "/" } else { "" }.to_owned();
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
        if !dir.ends_with('/') {
            out.push('/');
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

/// POSIX 分隔符语义的方法表（`path/posix` 与 `path.posix` 共用同一实现）。
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

/// `isAbsolute(p)`：POSIX 语义——首字符为 `/`。
fn is_absolute(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let path = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    Ok(Value::Boolean(path.starts_with('/')))
}

/// POSIX 分隔符 / 路径列表分隔符（`path.posix.sep` / `path.posix.delimiter`）。
pub(crate) const SEP: &str = "/";
pub(crate) const DELIMITER: &str = ":";

/// `normalize(p)`：Node `posix.normalize` 逐字移植——保留**尾部分隔符**
///（`'a/'` → `'a/'`）与根形态（`'/'` → `'/'`），空段折叠但不做 Go 式
/// 的「结果恒无尾斜杠」归一。
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
    Ok(Value::Object(vm.alloc_string(posix_normalize(&p))))
}

/// Node `posix.normalize`。
fn posix_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let b = path.as_bytes();
    let is_absolute = b[0] == b'/';
    let trailing_sep = b[b.len() - 1] == b'/';
    let mut out = posix_normalize_string(path, !is_absolute);
    if out.is_empty() {
        if is_absolute {
            return "/".to_owned();
        }
        return if trailing_sep {
            "./".to_owned()
        } else {
            ".".to_owned()
        };
    }
    if trailing_sep {
        out.push('/');
    }
    if is_absolute { format!("/{out}") } else { out }
}

/// `normalizeString(path, allowAboveRoot, '/', isPosixPathSeparator)` 的 POSIX
/// 实例化：折叠 `.`/`..` 与空段。`allow_above_root` 为 false（绝对路径）
/// 时丢弃越根的 `..`，否则保留（`'../a'` 原样）。
fn posix_normalize_string(path: &str, allow_above_root: bool) -> String {
    let mut resolved: Vec<&str> = Vec::new();
    for raw in path.split('/') {
        match raw {
            "" | "." => {}
            ".." => match resolved.last() {
                // 可回退：抵消上一个普通段
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
    resolved.join("/")
}

/// `join(...parts)`：Node `posix.join` —— 空元素跳过，其余以 `/` 相连后
/// 过 `normalize`（因此保留尾部分隔符；全空 → `'.'`）。
fn join(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let parts: Vec<String> = args
        .iter()
        .map(|v| vm.format_value(*v))
        .filter(|s| !s.is_empty())
        .collect();
    let s = if parts.is_empty() {
        ".".to_owned()
    } else {
        posix_normalize(&parts.join("/"))
    };
    Ok(Value::Object(vm.alloc_string(s)))
}

/// `basename(p[, ext])`：Node `posix.basename` 逐字移植（原串切片，不做
/// Clean；`''` / 全分隔符 → `''`；suffix 逐字符比对回退）。
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
            crate::builtins::path_node::is_posix_sep,
            false,
        ),
    )))
}

/// `dirname(p)`：Node `posix.dirname` 逐字移植（原串切片）。
fn dirname(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let p = match args.first() {
        Some(v) => vm.format_value(*v),
        None => return Ok(Value::Object(vm.alloc_string(".".to_owned()))),
    };
    Ok(Value::Object(vm.alloc_string(node_posix_dirname(&p))))
}

/// Node `posix.dirname`。
fn node_posix_dirname(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let b = path.as_bytes();
    let has_root = b[0] == b'/';
    let mut end: isize = -1;
    let mut matched_slash = true;
    let mut i = b.len() as isize - 1;
    while i >= 1 {
        if b[i as usize] == b'/' {
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
        return if has_root {
            "/".to_owned()
        } else {
            ".".to_owned()
        };
    }
    if has_root && end == 1 {
        return "//".to_owned();
    }
    path[..end as usize].to_owned()
}

/// `extname(p)`：Node `posix.extname` 逐字移植（`preDotState` 状态机，
/// `'..'` 与首点隐藏文件 → `''`；此前经 basename 间接实现，`'..'` 误判为
/// `'.'`）。
fn extname(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let path = match args.first() {
        Some(v) => vm.format_value(*v),
        None => return Ok(Value::Object(vm.alloc_string(String::new()))),
    };
    Ok(Value::Object(vm.alloc_string(
        crate::builtins::path_node::node_extname(&path, crate::builtins::path_node::is_posix_sep),
    )))
}

/// `resolve(...parts)`：绝对化；相对结果基于当前工作目录（ToSlash 转 `/`）。
fn resolve(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let elems: Vec<String> = args.iter().map(|v| vm.format_value(*v)).collect();
    let mut resolved = posix_join(&elems);
    if !resolved.starts_with('/') {
        if let Ok(wd) = std::env::current_dir() {
            let wd_slash = wd.to_string_lossy().replace('\\', "/");
            resolved = posix_join(&[wd_slash, resolved]);
        }
    }
    let s = vm.alloc_string(posix_clean(&resolved));
    Ok(Value::Object(s))
}

/// `relative(from, to)`：Node POSIX 语义——公共目录前缀剥离，剩余 `from`
/// 段以上溯 `../` 补偿（depd 的 formatLocation 用其缩短调用路径显示）。
fn relative(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let from = args
        .first()
        .map(|v| posix_clean(&vm.format_value(*v)))
        .unwrap_or_default();
    let to = args
        .get(1)
        .map(|v| posix_clean(&vm.format_value(*v)))
        .unwrap_or_default();
    let out = posix_relative(&from, &to);
    Ok(Value::Object(vm.alloc_string(out)))
}

fn posix_relative(from: &str, to: &str) -> String {
    if from == to {
        return String::new();
    }
    let from_segs: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    let to_segs: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    // 公共前缀段数
    let mut common = 0usize;
    while common < from_segs.len() && common < to_segs.len() && from_segs[common] == to_segs[common]
    {
        common += 1;
    }
    let mut out: Vec<String> = Vec::new();
    for _ in common..from_segs.len() {
        out.push("..".to_owned());
    }
    for seg in &to_segs[common..] {
        out.push((*seg).to_owned());
    }
    if out.is_empty() {
        return ".".to_owned();
    }
    out.join("/")
}

// ---- Go 标准库 `path` 包移植（逐字对齐） ----

/// Go `path.lazybuf` 逐字移植（未分歧时 index 直读输入串）。
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

    /// 读位置 i 的字节：未分歧读输入串（Go `b.s[i]`），分歧后读物化缓冲。
    fn index(&self, i: usize) -> u8 {
        if self.buf.is_empty() {
            self.s[i]
        } else {
            self.buf[i]
        }
    }

    /// Go `b.append(c)`：逐字节匹配则免拷贝推进，否则物化定长缓冲。
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

/// `path.Join`：连接元素（空元素跳过），结果 Clean；全空 → `""`。
fn posix_join(elems: &[String]) -> String {
    let size: usize = elems.iter().map(|e| e.len()).sum();
    if size == 0 {
        // 全空元素：Go `path.Join` 在此返回 ""，但 Node `path.join('')`
        // 必须为 "."（Join 后仍要过 Clean，Clean("") === "."）。
        // 无参 `path.join()` 同样为 "."。
        return ".".to_owned();
    }
    let mut buf = String::new();
    for e in elems {
        if !buf.is_empty() || !e.is_empty() {
            if !buf.is_empty() {
                buf.push('/');
            }
            buf.push_str(e);
        }
    }
    posix_clean(&buf)
}

/// `path.Clean`：纯词法折叠（`//`→`/`、`.`/`..` 消解；根起始 `..` 熔断）。
fn posix_clean(p: &str) -> String {
    if p.is_empty() {
        return ".".to_owned();
    }
    let s = p.as_bytes();
    let n = s.len();
    let rooted = s[0] == b'/';
    let mut out = LazyBuf::new(s);
    let mut dotdot = 0usize;
    let mut r = 0usize;
    if rooted {
        out.append(b'/');
        r = 1;
        dotdot = 1;
    }
    while r < n {
        let c = s[r];
        if c == b'/' {
            // 空路径元素
            r += 1;
        } else if c == b'.' && (r + 1 == n || s[r + 1] == b'/') {
            // "." 元素
            r += 1;
        } else if c == b'.' && r + 1 < n && s[r + 1] == b'.' && (r + 2 == n || s[r + 2] == b'/') {
            // ".." 元素：回退到上一个分隔符
            r += 2;
            if out.w > dotdot {
                // 可回退：Go 语义是先退一位再从边界位读回
                out.w -= 1;
                while out.w > dotdot && out.index(out.w) != b'/' {
                    out.w -= 1;
                }
            } else if !rooted {
                // 不可回退且非根起始：追加 .. 元素
                if out.w > 0 {
                    out.append(b'/');
                }
                out.append(b'.');
                out.append(b'.');
                dotdot = out.w;
            }
        } else {
            // 真实路径元素
            if (rooted && out.w != 1) || (!rooted && out.w != 0) {
                out.append(b'/');
            }
            while r < n && s[r] != b'/' {
                out.append(s[r]);
                r += 1;
            }
        }
    }
    if out.w == 0 {
        return ".".to_owned();
    }
    if out.buf.is_empty() {
        String::from_utf8(out.s[..out.w].to_vec()).unwrap_or_else(|_| p.to_owned())
    } else {
        String::from_utf8(out.buf[..out.w].to_vec()).unwrap_or_else(|_| p.to_owned())
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
