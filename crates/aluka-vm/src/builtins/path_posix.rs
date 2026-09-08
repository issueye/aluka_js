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
    let methods: [(&str, BuiltinHandler); 6] = [
        ("join", join),
        ("basename", basename),
        ("dirname", dirname),
        ("extname", extname),
        ("resolve", resolve),
        ("relative", relative),
    ];
    for (name, handler) in methods {
        let f = vm.alloc_native_fn(&format!("path/posix.{name}"));
        set_module_prop(vm, obj, name, Value::Object(f))?;
        register_handler(registry, "path/posix", name, handler);
    }
    Ok(obj)
}

/// `join(...parts)`：`path.Join`（空元素跳过；结果 Clean）。
fn join(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let elems: Vec<String> = args.iter().map(|v| vm.format_value(*v)).collect();
    let s = vm.alloc_string(posix_join(&elems));
    Ok(Value::Object(s))
}

/// `basename(p[, ext])`：末元素（去尾部斜杠）；提供 ext 且尾部匹配时去掉。
fn basename(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() {
        return Ok(Value::Object(vm.alloc_string(String::new())));
    }
    let mut b = posix_base(&vm.format_value(args[0]));
    if let Some(ext) = args.get(1) {
        let ext = vm.format_value(*ext);
        if !ext.is_empty() && b.len() > ext.len() && b.ends_with(&ext) {
            b.truncate(b.len() - ext.len());
        }
    }
    Ok(Value::Object(vm.alloc_string(b)))
}

/// `dirname(p)`：Split 后 Clean（无分隔符时 `"."`）。
fn dirname(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let p = match args.first() {
        Some(v) => vm.format_value(*v),
        None => return Ok(Value::Object(vm.alloc_string(".".to_owned()))),
    };
    let s = vm.alloc_string(posix_dir(&p));
    Ok(Value::Object(s))
}

/// `extname(p)`：Node 语义（基于 basename 的最后一个 `.`；首点隐藏文件 → `""`）。
fn extname(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() {
        return Ok(Value::Object(vm.alloc_string(String::new())));
    }
    let base = posix_base(&vm.format_value(args[0]));
    let s = vm.alloc_string(node_extname(&base));
    Ok(Value::Object(s))
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
        return String::new();
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

/// `path.Base`：末元素；空串 → `"."`；纯斜杠 → `"/"`。
fn posix_base(p: &str) -> String {
    if p.is_empty() {
        return ".".to_owned();
    }
    let bytes = p.as_bytes();
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == b'/' {
        end -= 1;
    }
    let mut start = 0usize;
    let mut i = end;
    while i > 0 {
        if bytes[i - 1] == b'/' {
            start = i;
            break;
        }
        i -= 1;
    }
    if start == end {
        return "/".to_owned();
    }
    String::from_utf8(bytes[start..end].to_vec()).unwrap_or_else(|_| p.to_owned())
}

/// `path.Dir`：Split 后 Clean。
fn posix_dir(p: &str) -> String {
    let bytes = p.as_bytes();
    let mut i = bytes.len();
    while i > 0 && bytes[i - 1] != b'/' {
        i -= 1;
    }
    posix_clean(&p[..i])
}

/// Node extname：取 basename 中最后一个 `.` 之后；`.` 在首位（隐藏文件）→ `""`。
fn node_extname(base: &str) -> String {
    match base.rfind('.') {
        Some(idx) if idx > 0 => base[idx..].to_owned(),
        _ => String::new(),
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
