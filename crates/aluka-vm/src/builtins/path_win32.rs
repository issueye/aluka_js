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
    let methods: [(&str, BuiltinHandler); 5] = [
        ("join", join),
        ("basename", basename),
        ("dirname", dirname),
        ("extname", extname),
        ("resolve", resolve),
    ];
    for (name, handler) in methods {
        let f = vm.alloc_native_fn(&format!("path/win32.{name}"));
        set_module_prop(vm, obj, name, Value::Object(f))?;
        register_handler(registry, "path/win32", name, handler);
    }
    Ok(obj)
}

/// `join(...parts)`：`filepath.Join`（走 Go windows join：首元素卷保留、
/// 尾部分隔符剥离、驱动相对不插分隔符，结果 Clean）。
fn join(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let elems: Vec<String> = args.iter().map(|v| vm.format_value(*v)).collect();
    let s = vm.alloc_string(win_join(&elems));
    Ok(Value::Object(s))
}

/// `basename(p[, ext])`：末元素（去尾部斜杠；体积名剔除）；ext 匹配时去掉。
fn basename(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() {
        return Ok(Value::Object(vm.alloc_string(String::new())));
    }
    let mut b = win_base(&vm.format_value(args[0]));
    if let Some(ext) = args.get(1) {
        let ext = vm.format_value(*ext);
        if !ext.is_empty() && b.len() > ext.len() && b.ends_with(&ext) {
            b.truncate(b.len() - ext.len());
        }
    }
    Ok(Value::Object(vm.alloc_string(b)))
}

/// `dirname(p)`：Split 后 Clean（`"file.txt"` → `"."`；`"C:foo"` → `"C:."`）。
fn dirname(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let p = match args.first() {
        Some(v) => vm.format_value(*v),
        None => return Ok(Value::Object(vm.alloc_string(".".to_owned()))),
    };
    let s = vm.alloc_string(win_dir(&p));
    Ok(Value::Object(s))
}

/// `extname(p)`：Node 语义（基于 basename 的最后一个 `.`；首点隐藏文件 → `""`）。
fn extname(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() {
        return Ok(Value::Object(vm.alloc_string(String::new())));
    }
    let base = win_base(&vm.format_value(args[0]));
    let s = vm.alloc_string(node_extname(&base));
    Ok(Value::Object(s))
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

/// `filepath.Join`（Windows 实现，go1.25 join/joinNonEmpty）。
fn win_join(elems: &[String]) -> String {
    let mut b: Vec<u8> = Vec::new();
    let mut last_char = 0u8;
    for e in elems {
        let mut eb = e.as_bytes();
        if b.is_empty() {
            // 首个非空元素原样加入；空元素跳过（last_char 不变）
        } else if is_sep(last_char) {
            // 尾部分隔符：剥离下一元素前导分隔符，避免拼出 UNC
            while !eb.is_empty() && is_sep(eb[0]) {
                eb = &eb[1..];
            }
            // `\` + `??...` 需要补 `.\`（Root Local Device 语义）
            if b.len() == 1
                && eb.len() >= 2
                && eb[0] == b'?'
                && eb[1] == b'?'
                && (eb.len() == 2 || is_sep(eb[2]))
            {
                b.extend_from_slice(b".\\");
            }
        } else if last_char == b':' {
            // 驱动相对：不加分隔符
        } else {
            b.push(b'\\');
            last_char = b'\\';
        }
        if !eb.is_empty() {
            b.extend_from_slice(eb);
            last_char = eb[eb.len() - 1];
        }
    }
    if b.is_empty() {
        return String::new();
    }
    win_clean(&path_bytes_to_string(&b))
}

/// `filepathlite.VolumeName`。
fn win_volume_name(p: &str) -> String {
    let b = p.as_bytes();
    let vol_len = volume_name_len(b);
    path_bytes_to_string(&from_slash(&b[..vol_len]))
}

/// `filepathlite.Base`：末元素（去尾部斜杠、剔体积名；空 → `.`；纯斜杠 → `\`）。
fn win_base(p: &str) -> String {
    if p.is_empty() {
        return ".".to_owned();
    }
    let b = p.as_bytes();
    let mut end = b.len();
    while end > 0 && is_sep(b[end - 1]) {
        end -= 1;
    }
    let vol_len = volume_name_len(&b[..end]);
    let mut i = end;
    while i > vol_len && !is_sep(b[i - 1]) {
        i -= 1;
    }
    let elem = &b[i..end];
    if elem.is_empty() {
        // 体积名即全部（如 `C:`/`C:/`）、或全是分隔符 → 根分隔符
        return "\\".to_owned();
    }
    path_bytes_to_string(elem)
}

/// `filepathlite.Dir`：Split 后 Clean（体积拼接；UNC 卷特判）。
fn win_dir(p: &str) -> String {
    let b = p.as_bytes();
    let vol = win_volume_name(p);
    let vol_len = vol.len();
    let mut i = b.len();
    while i > vol_len && !is_sep(b[i - 1]) {
        i -= 1;
    }
    let dir = win_clean(&p[vol_len..i]);
    if dir == "." && vol_len > 2 {
        return vol; // UNC 卷
    }
    format!("{vol}{dir}")
}

/// Node extname：取 basename 中最后一个 `.` 之后；`.` 在首位（隐藏文件）→ `""`。
fn node_extname(base: &str) -> String {
    match base.rfind('.') {
        Some(idx) if idx > 0 => base[idx..].to_owned(),
        _ => String::new(),
    }
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
