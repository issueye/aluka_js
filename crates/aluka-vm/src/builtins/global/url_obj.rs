//! WHATWG `URL` 实例：访问器面 + 与 `URLSearchParams` 的双向联动。
//!
//! 表示：实例是普通 Ordinary 对象，内部状态存在隐藏数组 `_urlState` 中
//! （顺序固定为 `[scheme, username, password, hostname, port, pathname,
//! search, hash]`，`scheme` 含尾 `:`，`search`/`hash` 含首 `?`/`#`）。
//! 对外每个键都以**访问器**挂载（`ordinary_define_property` 的 get/set
//! 表），因此 `u.pathname = '/x'` 这类写入会真正改写内部状态并立即反映到
//! `href`，而不是只覆盖一个数据属性——真实包（axios 的 `buildURL`、
//! `new URL(input, base)` 解析）依赖该联动。
//!
//! 全部访问器共用同一对 NativeFn 名（`URL:accessor.get` /
//! `URL:accessor.set`），具体键从**被调用的函数对象**上的 `_urlKey` 读取
//! （`pending_callee()` + `get_native_fn_property`）——否则每个键都要在
//! 分派表里登记一条，且键名含 `.`/`:` 会撞上分派键的拼接规则。

use crate::builtins::{current_receiver, pending_callee};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};

/// 状态数组的槽位下标（顺序即 `_urlState` 布局）。
const S_SCHEME: usize = 0;
const S_USER: usize = 1;
const S_PASS: usize = 2;
const S_HOST: usize = 3;
const S_PORT: usize = 4;
const S_PATH: usize = 5;
const S_SEARCH: usize = 6;
const S_HASH: usize = 7;
const SLOTS: usize = 8;

/// URL 的规范默认端口表（特殊 scheme）。
fn default_port(scheme: &str) -> &'static str {
    match scheme {
        "http:" | "ws:" => "80",
        "https:" | "wss:" => "443",
        "ftp:" => "21",
        _ => "",
    }
}

/// 特殊 scheme（有 host、走默认端口与路径归一）。
fn is_special(scheme: &str) -> bool {
    matches!(
        scheme,
        "http:" | "https:" | "ws:" | "wss:" | "ftp:" | "file:"
    )
}

/// 从状态数组构造 `_urlState` 值的读写器。
pub(crate) fn url_state(vm: &mut Vm, recv: Value) -> Option<Vec<String>> {
    let parts = vm.get_property(recv, "_urlState").ok()?;
    let ValueCase::Object(arr) = parts.case() else {
        return None;
    };
    let elems = match vm.heap.get(arr.index()) {
        Some(HeapObject::Array { elements, .. }) => elements.clone(),
        _ => return None,
    };
    if elems.len() != SLOTS {
        return None;
    }
    Some(elems.iter().map(|v| vm.format_value(*v)).collect())
}

pub(crate) fn write_state(vm: &mut Vm, recv: Value, st: &[String]) {
    let vals: Vec<Value> = st
        .iter()
        .map(|s| Value::Object(vm.alloc_string(s.clone())))
        .collect();
    let arr = vm.alloc_array(vals);
    let _ = vm.set_property(recv, "_urlState", Value::Object(arr));
}

// ===== 序列化 =====

fn serialize_href(st: &[String]) -> String {
    let mut out = String::from(&st[S_SCHEME]);
    let has_auth = !st[S_USER].is_empty() || !st[S_PASS].is_empty();
    let has_host = !st[S_HOST].is_empty();
    if has_host || is_special(&st[S_SCHEME]) {
        out.push_str("//");
    }
    if has_auth {
        out.push_str(&st[S_USER]);
        if !st[S_PASS].is_empty() {
            out.push(':');
            out.push_str(&st[S_PASS]);
        }
        out.push('@');
    }
    out.push_str(&st[S_HOST]);
    if !st[S_PORT].is_empty() {
        out.push(':');
        out.push_str(&st[S_PORT]);
    }
    out.push_str(&st[S_PATH]);
    out.push_str(&st[S_SEARCH]);
    out.push_str(&st[S_HASH]);
    out
}

fn serialize_origin(st: &[String]) -> String {
    // `file:` 与无 host 的非特殊 scheme 都是不透明源（规范 step: opaque origin
    // serializes as "null"）
    if st[S_SCHEME] == "file:" || (!is_special(&st[S_SCHEME]) && st[S_HOST].is_empty()) {
        return "null".to_owned();
    }
    let mut out = format!("//{}", st[S_HOST]);
    if !st[S_PORT].is_empty() {
        out = format!("//{}:{}", st[S_HOST], st[S_PORT]);
    }
    format!("{}{}", st[S_SCHEME], out)
}

/// WHATWG 百分号编码集（不编码的字符之外按 UTF-8 逐字节转义）。
///
/// `set` 取 `""`（路径）、`"query"`、`"fragment"` 三种编码集之一——
/// 三者的差异只在少数几个标点是否编码。
fn percent_encode(s: &str, set: EncodeSet) -> String {
    let needs = |b: u8| -> bool {
        if !(0x20..0x80).contains(&b) || b == 0x7F {
            return true;
        }
        match set {
            // path：space " # < > ? ` { }
            EncodeSet::Path => matches!(
                b,
                b' ' | b'"' | b'#' | b'<' | b'>' | b'?' | b'`' | b'{' | b'}'
            ),
            // userinfo/query：space " # < >（路径集去掉 `?` 等）
            EncodeSet::Query => matches!(b, b' ' | b'"' | b'#' | b'<' | b'>'),
            // fragment：space " < > `
            EncodeSet::Fragment => matches!(b, b' ' | b'"' | b'<' | b'>' | b'`'),
        }
    };
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if needs(b) {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum EncodeSet {
    Path,
    Query,
    Fragment,
}

/// `?` 后内容为空 → 空串（Node：`new URL('https://a.com/?').search === ''`）。
fn normalize_search(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let body = raw.strip_prefix('?').unwrap_or(raw);
    // 内容为空时保留 `?` 本身：href 序列化需要它，而 `.search` 读取端
    // 会把裸 `?` 映射回 `""`（见 read_key）——WHATWG 的 query 存在性与
    // 内容非空是两件事
    format!("?{}", percent_encode(body, EncodeSet::Query))
}

/// `#` 后内容为空 → 空串。
fn normalize_hash(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let body = raw.strip_prefix('#').unwrap_or(raw);
    format!("#{}", percent_encode(body, EncodeSet::Fragment))
}

// ===== 解析 =====

/// 解析 `input`（可选相对 `base` 解析）。返回 `None` 表示无法解析（抛
/// TypeError，与 WHATWG 的 `URL` 构造器失败语义一致）。
pub(crate) fn parse_url(input: &str, base: Option<&[String]>) -> Option<Vec<String>> {
    let trimmed = input.trim();
    let mut st: Vec<String> = vec![String::new(); SLOTS];
    // 先分离 fragment
    let (before_hash, hash) = match trimmed.find('#') {
        Some(i) => (&trimmed[..i], &trimmed[i..]),
        None => (trimmed, ""),
    };
    st[S_HASH] = normalize_hash(hash);

    // scheme 前缀：字母开头 + 字母/数字/+/-/. 后接 `:`
    let scheme_end = before_hash.find(':').filter(|i| {
        *i > 0
            && before_hash.as_bytes()[0].is_ascii_alphabetic()
            && before_hash[..*i]
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
    });
    let (scheme, rest) = match scheme_end {
        Some(i) => (
            before_hash[..i].to_ascii_lowercase() + ":",
            &before_hash[i + 1..],
        ),
        None => (String::new(), before_hash),
    };

    // 无 scheme：相对解析
    if scheme.is_empty() {
        let b = base?;
        st[S_SCHEME].clone_from(&b[S_SCHEME]);
        st[S_USER].clone_from(&b[S_USER]);
        st[S_PASS].clone_from(&b[S_PASS]);
        st[S_HOST].clone_from(&b[S_HOST]);
        st[S_PORT].clone_from(&b[S_PORT]);
        // 路径也继承 base（仅替换路径/查询时，未提及的部分保持 base 值）
        st[S_PATH].clone_from(&b[S_PATH]);
        if rest.is_empty() {
            st[S_SEARCH].clone_from(&b[S_SEARCH]);
            return Some(st);
        }
        if let Some(r) = rest.strip_prefix("//") {
            // 网络路径引用（`new URL('//b.org/p', base)`）：换 authority，
            // 保留 base 的 scheme
            return parse_authority(r, st);
        }
        if rest.starts_with('?') {
            // 仅查询串：路径与 authority 不变（`new URL('?q=2', base)`）
            st[S_SEARCH] = normalize_search(rest);
            return Some(st);
        }
        if let Some(q) = rest.find('?') {
            st[S_SEARCH] = normalize_search(&rest[q..]);
            st[S_PATH] = resolve_path(&b[S_PATH], &rest[..q]);
        } else {
            st[S_PATH] = resolve_path(&b[S_PATH], rest);
        }
        return Some(st);
    }

    st[S_SCHEME] = scheme;
    if rest.is_empty() {
        // `scheme:`：特殊 scheme 的空路径规范化为 `/`
        if is_special(&st[S_SCHEME]) {
            st[S_PATH] = "/".to_owned();
        }
        return Some(st);
    }
    if let Some(r) = rest.strip_prefix("//") {
        return parse_authority(r, st);
    }
    // 无 authority 的相对部分
    let (path_part, search) = match rest.find('?') {
        Some(q) => (&rest[..q], &rest[q..]),
        None => (rest, ""),
    };
    st[S_SEARCH] = normalize_search(search);
    st[S_PATH] = if is_special(&st[S_SCHEME]) {
        normalize_path(path_part)
    } else {
        // 非特殊 scheme（`mailto:` / `data:`）是不透明路径：不做 `.`/`..`
        // 折叠，仅做编码
        percent_encode(path_part, EncodeSet::Path)
    };
    Some(st)
}

/// 解析 authority 段（`host[:port]/path?query`，已剥掉 `//`）。
fn parse_authority(rest: &str, mut st: Vec<String>) -> Option<Vec<String>> {
    let auth_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..auth_end];
    let tail = &rest[auth_end..];

    // userinfo
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
        None => (None, authority),
    };
    if let Some(ui) = userinfo {
        let mut it = ui.splitn(2, ':');
        st[S_USER] = it.next().unwrap_or("").to_owned();
        st[S_PASS] = it.next().unwrap_or("").to_owned();
    }
    // host:port（IPv6 字面量 `[::1]` 需跳过方括号）
    let (host, port) = if let Some(close) = hostport.find(']') {
        match hostport[close..].find(':') {
            Some(rel) => (
                &hostport[..close + 1],
                hostport[close + rel + 1..].to_owned(),
            ),
            None => (hostport, String::new()),
        }
    } else {
        match hostport.find(':') {
            Some(i) => (&hostport[..i], hostport[i + 1..].to_owned()),
            None => (hostport, String::new()),
        }
    };
    st[S_HOST] = host.to_ascii_lowercase();
    // 默认端口收敛为空串
    st[S_PORT] = if port == default_port(&st[S_SCHEME]) {
        String::new()
    } else {
        port
    };

    let (path_part, search) = match tail.find('?') {
        Some(q) => (&tail[..q], &tail[q..]),
        None => (tail, ""),
    };
    st[S_SEARCH] = normalize_search(search);
    // 特殊 scheme 的空路径 → `/`（`https://a.com` 的 pathname 是 `/`）
    st[S_PATH] = if path_part.is_empty() && is_special(&st[S_SCHEME]) {
        "/".to_owned()
    } else {
        normalize_path(path_part)
    };
    Some(st)
}

/// 路径归一：折叠 `.`/`..`，**保留空段**（`/a//b` 保持 `/a//b`，WHATWG
/// 的 path 是分段列表，空段合法），并做路径百分号编码。
fn normalize_path(path: &str) -> String {
    let rooted = path.starts_with('/');
    let parts: Vec<&str> = path.split('/').collect();
    // 根起始时 `split` 的首元素是前导 `/` 的产物，不是路径段
    let segs_in = if rooted { &parts[1..] } else { &parts[..] };
    let mut segs: Vec<String> = Vec::new();
    for raw in segs_in {
        match *raw {
            "" => segs.push(String::new()),
            "." => {}
            ".." => match segs.iter().rposition(|s| !s.is_empty()) {
                // 弹出最近的非空段（空段随其后的分隔符一起消失）
                Some(i) => {
                    segs.remove(i);
                }
                None if !rooted => segs.push("..".to_owned()),
                None => {}
            },
            other => segs.push(percent_encode(other, EncodeSet::Path)),
        }
    }
    let body = segs.join("/");
    if rooted {
        format!("/{body}")
    } else if body.is_empty() {
        String::new()
    } else {
        body
    }
}

/// 相对路径解析：`ref` 相对 `base_path`。`ref` 以 `/` 开头时直接归一。
fn resolve_path(base_path: &str, reference: &str) -> String {
    if reference.starts_with('/') {
        return normalize_path(reference);
    }
    let dir = match base_path.rfind('/') {
        Some(i) => &base_path[..=i],
        None => "/",
    };
    normalize_path(&format!("{dir}{reference}"))
}

// ===== 访问器 =====

/// `read` / `write` 的键 → 读取实现。
fn read_key(st: &[String], key: &str) -> String {
    match key {
        "href" => serialize_href(st),
        "origin" => serialize_origin(st),
        "protocol" => st[S_SCHEME].clone(),
        "username" => st[S_USER].clone(),
        "password" => st[S_PASS].clone(),
        "host" => {
            if st[S_PORT].is_empty() {
                st[S_HOST].clone()
            } else {
                format!("{}:{}", st[S_HOST], st[S_PORT])
            }
        }
        "hostname" => st[S_HOST].clone(),
        "port" => st[S_PORT].clone(),
        "pathname" => st[S_PATH].clone(),
        // 裸 `?` / `#`（存在但内容为空）在读取端表现为空串
        "search" => match st[S_SEARCH].as_str() {
            "?" => String::new(),
            other => other.to_owned(),
        },
        "hash" => match st[S_HASH].as_str() {
            "#" => String::new(),
            other => other.to_owned(),
        },
        _ => String::new(),
    }
}

/// GET 访问器：读 `_urlState` 的派生视图。
pub(crate) fn url_accessor_get(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let key = accessor_key(vm);
    let recv = current_receiver();
    // searchParams：惰性建一个与本源联动的 URLSearchParams
    if key == "searchParams" {
        if let Ok(existing) = vm.get_property(recv, "_urlSearchParams") {
            if existing.is_object() {
                return Ok(existing);
            }
        }
        let init = match url_state(vm, recv) {
            Some(st) => Value::Object(vm.alloc_string(st[S_SEARCH].clone())),
            None => Value::Undefined,
        };
        let usp = crate::builtins::global::web::url_search_params_ctor(vm, &[init])?;
        // 双向联动：searchParams 的每次改写回写本源 search/href
        let _ = vm.set_property(usp, "_uspOwner", recv);
        let _ = vm.set_property(recv, "_urlSearchParams", usp);
        return Ok(usp);
    }
    let Some(st) = url_state(vm, recv) else {
        return Ok(Value::Undefined);
    };
    Ok(Value::Object(vm.alloc_string(read_key(&st, &key))))
}

/// SET 访问器：改写 `_urlState` 对应槽位。`href` 走整串重解析。
pub(crate) fn url_accessor_set(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let key = accessor_key(vm);
    let recv = current_receiver();
    let text = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let Some(mut st) = url_state(vm, recv) else {
        return Ok(Value::Undefined);
    };
    match key.as_str() {
        "href" => {
            let Some(new_st) = parse_url(&text, None) else {
                return Err(vm.type_error("Invalid URL"));
            };
            st = new_st;
        }
        "protocol" => {
            let mut s = text.to_ascii_lowercase();
            if !s.ends_with(':') {
                s.push(':');
            }
            st[S_SCHEME] = s;
        }
        "username" => st[S_USER] = text,
        "password" => st[S_PASS] = text,
        "host" => {
            let (h, p) = split_host_port(&text);
            st[S_HOST] = h.to_ascii_lowercase();
            st[S_PORT] = if p == default_port(&st[S_SCHEME]) {
                String::new()
            } else {
                p
            };
        }
        "hostname" => st[S_HOST] = text.to_ascii_lowercase(),
        "port" => {
            st[S_PORT] = if text == default_port(&st[S_SCHEME]) {
                String::new()
            } else {
                text
            };
        }
        "pathname" => st[S_PATH] = normalize_path(&text),
        "search" => {
            st[S_SEARCH] = if text.is_empty() {
                String::new()
            } else if text.starts_with('?') {
                text
            } else {
                format!("?{text}")
            };
        }
        "hash" => {
            st[S_HASH] = if text.is_empty() {
                String::new()
            } else if text.starts_with('#') {
                text
            } else {
                format!("#{text}")
            };
        }
        _ => {}
    }
    write_state(vm, recv, &st);
    // searchParams 缓存随之失效（其内容由 search 派生）
    let _ = vm.set_property(recv, "_urlSearchParams", Value::Undefined);
    Ok(Value::Undefined)
}

fn split_host_port(host: &str) -> (String, String) {
    if let Some(close) = host.find(']') {
        return match host[close..].find(':') {
            Some(rel) => (
                host[..close + 1].to_owned(),
                host[close + rel + 1..].to_owned(),
            ),
            None => (host.to_owned(), String::new()),
        };
    }
    match host.find(':') {
        Some(i) => (host[..i].to_owned(), host[i + 1..].to_owned()),
        None => (host.to_owned(), String::new()),
    }
}

/// 从被调用的访问器函数对象上读取目标键。
fn accessor_key(vm: &Vm) -> String {
    let ValueCase::Object(r) = pending_callee().case() else {
        return String::new();
    };
    vm.get_native_fn_property(r, "_urlKey")
        .map(|v| match v.case() {
            ValueCase::Object(sr) => match vm.heap.get(sr.index()) {
                Some(HeapObject::String(s)) => s.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        })
        .unwrap_or_default()
}

// ===== 构造器与实例方法 =====

/// 访问器键面（get/set 各挂一个同键访问器）。
const KEYS: &[&str] = &[
    "href", "origin", "protocol", "username", "password", "host", "hostname", "port", "pathname",
    "search", "hash",
];

/// `new URL(input[, base])`。
pub(crate) fn url_ctor(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let input = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let base = match args.get(1).copied() {
        Some(b) if !b.is_undefined() => {
            let btext = vm.format_value(b);
            match parse_url(&btext, None) {
                Some(bst) => Some(bst),
                None => return Err(vm.type_error(&format!("Invalid base URL: {btext}"))),
            }
        }
        _ => None,
    };
    let Some(st) = parse_url(&input, base.as_deref()) else {
        return Err(vm.type_error("Invalid URL"));
    };

    let inst = vm.alloc_ordinary();
    // `_builtinNs` 让 `try_dispatch` 把实例方法派到 "URL:instance.X"
    let ns = Value::Object(vm.alloc_string("URL:instance".to_owned()));
    let _ = vm.set_property(Value::Object(inst), "_builtinNs", ns);
    write_state(vm, Value::Object(inst), &st);
    let _ = vm.set_property(Value::Object(inst), "_urlSearchParams", Value::Undefined);

    for key in KEYS.iter().chain(std::iter::once(&"searchParams")) {
        let is_readonly = *key == "origin" || *key == "searchParams";
        let g = vm.alloc_native_fn("URL:instance.accessor.get");
        let gv = Value::Object(g);
        let kv = Value::Object(vm.alloc_string((*key).to_owned()));
        vm.set_native_fn_property(g, "_urlKey", kv);
        let desc = vm.alloc_ordinary();
        let _ = vm.set_property(Value::Object(desc), "get", gv);
        let _ = vm.set_property(Value::Object(desc), "enumerable", Value::Boolean(true));
        let _ = vm.set_property(Value::Object(desc), "configurable", Value::Boolean(true));
        if !is_readonly {
            let s = vm.alloc_native_fn("URL:instance.accessor.set");
            let sv = Value::Object(s);
            let kv = Value::Object(vm.alloc_string((*key).to_owned()));
            vm.set_native_fn_property(s, "_urlKey", kv);
            let _ = vm.set_property(Value::Object(desc), "set", sv);
        }
        vm.ordinary_define_property(Value::Object(inst), key, Value::Object(desc))?;
    }

    // toString / toJSON 直接返回 href
    for m in ["toString", "toJSON"] {
        let f = vm.alloc_native_fn(&format!("URL:instance.{m}"));
        let _ = vm.set_property(Value::Object(inst), m, Value::Object(f));
    }
    Ok(Value::Object(inst))
}

/// `url.toString()` / `url.toJSON()`。
pub(crate) fn url_to_string(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let recv = current_receiver();
    match url_state(vm, recv) {
        Some(st) => Ok(Value::Object(vm.alloc_string(serialize_href(&st)))),
        None => Ok(Value::Undefined),
    }
}

/// `url.searchParams` 改写后回写 `url.search`（WHATWG 的「URL 与
/// searchParams 双向联动」：两者是同一底层数据的两个视图）。
pub(crate) fn sync_owner_from_usp(vm: &mut Vm, usp: Value) {
    let Ok(owner) = vm.get_property(usp, "_uspOwner") else {
        return;
    };
    if !owner.is_object() {
        return;
    }
    let entries = crate::builtins::global::web::usp_entries(vm, usp);
    let search: String = entries
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                crate::builtins::global::web::form_urlencode(k),
                crate::builtins::global::web::form_urlencode(v)
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    let Some(mut st) = url_state(vm, owner) else {
        return;
    };
    st[S_SEARCH] = if search.is_empty() {
        String::new()
    } else {
        format!("?{search}")
    };
    write_state(vm, owner, &st);
}
