//! `markdown` 与 `aluka:markdown` 内置模块（Phase 8）：Aluka 扩展 Markdown
//! 解析与 HTML 渲染。
//!
//! 渲染算法逐函数移植 Node.js 22 LTS 标准（`nodeutil/markdown.go`，纯 Go 自研实现）：
//! - [`render`]：Markdown → HTML 片段（标题/列表/代码块/引用/水平线/段落与
//!   行内格式：图片、链接、行内代码、粗体、斜体、删除线）；
//! - [`parse_frontmatter`]：文件头部 `--- ... ---` 键值元数据提取；
//! - `renderToHTML(md, {title})`：完整 HTML 骨架文档。
//!
//! Go 侧基于正则的行内替换按**等价的手写扫描器**移植（最左优先、非重叠、
//! 各分支与 Go 正则的匹配域一致）；两个 specifier（`markdown` 与
//! `aluka:markdown`）共享同一模块单例（Go 实测 `require("markdown") ===
//! require("aluka:markdown")` 为 true）。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// `require("markdown")` 模块条目。
pub const MODULE: ModuleDef = ModuleDef {
    name: "markdown",
    build,
};

/// `require("aluka:markdown")` 模块条目（与 [`MODULE`] 共享同一单例/分派表）。
pub const ALUKA_MODULE: ModuleDef = ModuleDef {
    name: "aluka:markdown",
    build: build_shared,
};

/// HTML 转义（对齐 Go `html.EscapeString`：`& < > ' "`）。
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            other => out.push(other),
        }
    }
    out
}

/// 解析 Markdown 文件头部的 YAML/键值元数据，返回 `(data 有序键值, body)`。
pub fn parse_frontmatter(content: &str) -> (Vec<(String, String)>, String) {
    let mut data: Vec<(String, String)> = Vec::new();
    let Some((raw_yaml, body)) = split_frontmatter(content) else {
        return (data, content.to_owned());
    };
    for line in raw_yaml.split('\n') {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(colon) = line.find(':') {
            if colon > 0 {
                let k = line[..colon].trim();
                let v = line[colon + 1..].trim().trim_matches(['"', '\'']);
                data.push((k.to_owned(), v.to_owned()));
            }
        }
    }
    (data, body.to_owned())
}

/// 切分 `^---\r?\n(.*?)\r?\n---\r?\n(.*)$`（DOTALL、非贪婪——即取第一处
/// 分隔线）。返回 `(raw_yaml, body)`；不匹配返回 None。
fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    // 前置锚：^---\r?\n
    let rest = content.strip_prefix("---")?;
    let after_open = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))?;
    // 扫描第一处 \r?\n---\r?\n（非贪婪 `(.*?)` 的最小化语义）。
    let bytes = after_open.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // 匹配 `\r?\n`
        let nl_len = if bytes[i] == b'\n' {
            1
        } else if bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            2
        } else {
            i += 1;
            continue;
        };
        let start = i + nl_len;
        if after_open[start..].starts_with("---") {
            let after_dashes = &after_open[start + 3..];
            if let Some(body) = after_dashes
                .strip_prefix("\r\n")
                .or_else(|| after_dashes.strip_prefix('\n'))
            {
                return Some((&after_open[..i], body));
            }
        }
        i += 1;
    }
    None
}

/// 行内匹配结果：`(全文起点, 全文终点, 捕获组)`。
type InlineMatch<'a> = (usize, usize, Vec<&'a str>);

/// 行内图片 `!\[([^\]]*)\]\(([^)]+)\)`。
fn find_img(s: &str, from: usize) -> Option<InlineMatch<'_>> {
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate().skip(from) {
        if *b == b'!' {
            if let Some((_, end, groups)) = find_bracket_paren(s, i + 1, true) {
                // 图片整体跨度从 `!` 起。
                return Some((i, end, groups));
            }
        }
    }
    None
}

/// 行内链接 `\[([^\]]+)\]\(([^)]+)\)`。
fn find_link(s: &str, from: usize) -> Option<InlineMatch<'_>> {
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate().skip(from) {
        if *b == b'[' {
            if let Some(found) = find_bracket_paren(s, i, false) {
                return Some(found);
            }
        }
    }
    None
}

/// `[...](...)` 主体匹配；`allow_empty_label` 为图片语义（alt 允许为空）。
fn find_bracket_paren<'a>(
    s: &'a str,
    open: usize,
    allow_empty_label: bool,
) -> Option<InlineMatch<'a>> {
    let bytes = s.as_bytes();
    if open >= bytes.len() || bytes[open] != b'[' {
        return None;
    }
    let mut j = open + 1;
    while j < bytes.len() && bytes[j] != b']' {
        j += 1;
    }
    if j >= bytes.len() {
        return None;
    }
    let label = &s[open + 1..j];
    if label.is_empty() && !allow_empty_label {
        return None;
    }
    if j + 1 >= bytes.len() || bytes[j + 1] != b'(' {
        return None;
    }
    let mut k = j + 2;
    while k < bytes.len() && bytes[k] != b')' {
        k += 1;
    }
    if k >= bytes.len() {
        return None;
    }
    let url = &s[j + 2..k];
    if url.is_empty() {
        return None;
    }
    Some((open, k + 1, vec![label, url]))
}

/// 行内代码 `` `([^`]+)` ``。
fn find_code(s: &str, from: usize) -> Option<InlineMatch<'_>> {
    let bytes = s.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'`' {
                j += 1;
            }
            if j < bytes.len() {
                if j > i + 1 {
                    return Some((i, j + 1, vec![&s[i + 1..j]]));
                }
                // 空代码（`[^`]+` 至少 1 字符）：跳过该反引号继续扫描。
                i += 1;
                continue;
            }
            return None; // 未闭合
        }
        i += 1;
    }
    None
}

/// 包裹标记匹配 `` `**x**` `` / `` `~~x~~` `` 等（内容不含标记首字节）。
fn find_wrapped<'a>(s: &'a str, from: usize, mark: &str) -> Option<InlineMatch<'a>> {
    let bytes = s.as_bytes();
    let first = *mark.as_bytes().first()?;
    let mut i = from;
    while i + 2 * mark.len() <= bytes.len() {
        if bytes[i] == first && &bytes[i..i + mark.len()] == mark.as_bytes() {
            let mut j = i + mark.len();
            while j < bytes.len() && bytes[j] != first {
                j += 1;
            }
            if j + mark.len() <= bytes.len() && &bytes[j..j + mark.len()] == mark.as_bytes() {
                let content = &s[i + mark.len()..j];
                if !content.is_empty() {
                    return Some((i, j + mark.len(), vec![content]));
                }
            }
        }
        i += 1;
    }
    None
}

/// 备选包裹匹配（如 `**x**` 或 `__x__`），取更早出现者（Go 正则交替语义）。
fn find_wrapped_alt<'a>(
    s: &'a str,
    from: usize,
    mark_a: &str,
    mark_b: &str,
) -> Option<InlineMatch<'a>> {
    match (find_wrapped(s, from, mark_a), find_wrapped(s, from, mark_b)) {
        (Some(a), Some(b)) if a.0 <= b.0 => Some(a),
        (Some(_), Some(b)) => Some(b),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// `ReplaceAllStringFunc` 等价循环：最左非重叠逐段替换。
///
/// `find` 从 `pos` 起找首个匹配（返回跨度与捕获组）；`render` 由捕获组构建替换文本。
fn replace_inline<F, G>(s: &str, mut find: F, mut render: G) -> String
where
    F: FnMut(&str, usize) -> Option<InlineMatch<'_>>,
    G: FnMut(&[&str]) -> String,
{
    let mut out = String::new();
    let mut pos = 0;
    while let Some((start, end, groups)) = find(s, pos) {
        out.push_str(&s[pos..start]);
        out.push_str(&render(&groups));
        pos = end;
    }
    out.push_str(&s[pos..]);
    out
}

/// 渲染行内元素（图片、链接、行内代码、粗体、斜体、删除线），顺序对齐 Go。
pub fn render_inline(text: &str) -> String {
    // 1. 图片 ![alt](url)
    let step1 = replace_inline(text, find_img, |g| {
        format!(
            "<img src=\"{}\" alt=\"{}\" />",
            escape_html(g[1]),
            escape_html(g[0])
        )
    });
    // 2. 链接 [text](url)
    let step2 = replace_inline(&step1, find_link, |g| {
        format!("<a href=\"{}\">{}</a>", escape_html(g[1]), g[0])
    });
    // 3. 行内代码 `code`
    let step3 = replace_inline(&step2, find_code, |g| {
        format!("<code>{}</code>", escape_html(g[0]))
    });
    // 4. 粗体 **bold** / __bold__
    let step4 = replace_inline(
        &step3,
        |s, from| find_wrapped_alt(s, from, "**", "__"),
        |g| format!("<strong>{}</strong>", g[0]),
    );
    // 5. 斜体 *italic* / _italic_
    let step5 = replace_inline(
        &step4,
        |s, from| find_wrapped_alt(s, from, "*", "_"),
        |g| format!("<em>{}</em>", g[0]),
    );
    // 6. 删除线 ~~text~~
    replace_inline(
        &step5,
        |s, from| find_wrapped(s, from, "~~"),
        |g| format!("<del>{}</del>", g[0]),
    )
}

/// 有序列表判定：`. ` 前全是数字（对齐 Go `isOrderedList`）。
fn is_ordered_list(s: &str) -> bool {
    let Some(dot) = s.find(". ") else {
        return false;
    };
    if dot == 0 {
        return false;
    }
    s.as_bytes()[..dot].iter().all(|b| b.is_ascii_digit())
}

/// 渲染状态（代码块/列表/引用三类块级上下文）。
#[derive(Default)]
struct RenderState {
    /// 是否处于 ``` 代码块内。
    in_code_block: bool,
    /// 代码块语言标注。
    code_lang: String,
    /// 代码块累积行。
    code_lines: Vec<String>,
    /// 是否处于列表内（ul/ol）。
    in_list: bool,
    /// 列表类型（"ul" | "ol"）。
    list_type: String,
    /// 输出缓冲。
    sb: String,
    /// 是否处于引用块内。
    in_blockquote: bool,
    /// 引用块累积行。
    bq_lines: Vec<String>,
}

impl RenderState {
    /// 关闭当前列表（`</ul>`/`</ol>`）。
    fn flush_list(&mut self) {
        if self.in_list {
            let ty = std::mem::take(&mut self.list_type);
            self.sb.push_str("</");
            self.sb.push_str(&ty);
            self.sb.push_str(
                ">
",
            );
            self.in_list = false;
        }
    }

    /// 关闭当前引用块（内容合并为单段）。
    fn flush_blockquote(&mut self) {
        if self.in_blockquote {
            self.in_blockquote = false;
            let joined = self.bq_lines.join(" ");
            self.bq_lines.clear();
            self.sb.push_str("<blockquote><p>");
            self.sb.push_str(&render_inline(&joined));
            self.sb.push_str(
                "</p></blockquote>
",
            );
        }
    }

    /// 输出代码块（闭合或文末收尾；`with_lang` 决定是否带语言标注）。
    fn flush_code_block(&mut self, with_lang: bool) {
        self.sb.push_str("<pre><code");
        if with_lang && !self.code_lang.is_empty() {
            let lang = escape_html(&self.code_lang);
            self.sb.push_str(&format!(" class=\"language-{lang}\""));
        }
        self.sb.push('>');
        let code = self.code_lines.join("\n");
        self.sb.push_str(&escape_html(&code));
        self.sb.push_str(
            "</code></pre>
",
        );
        self.code_lang = String::new();
        self.code_lines.clear();
    }
}

/// 将 Markdown 文本渲染为 HTML 片段（状态机逐行移植 Go `Render`）。
pub fn render(md: &str) -> String {
    let mut st = RenderState::default();

    for line in md.split('\n') {
        let line = line.trim_end_matches('\r');
        let trimmed = line.trim();

        // 代码块处理 ```
        if trimmed.starts_with("```") {
            st.flush_list();
            st.flush_blockquote();
            if st.in_code_block {
                st.flush_code_block(true);
                st.in_code_block = false;
            } else {
                st.in_code_block = true;
                st.code_lang = trimmed.strip_prefix("```").unwrap_or("").trim().to_owned();
            }
            continue;
        }

        if st.in_code_block {
            st.code_lines.push(line.to_owned());
            continue;
        }

        // 空行
        if trimmed.is_empty() {
            st.flush_list();
            st.flush_blockquote();
            continue;
        }

        // 水平线 ---, ***, ___
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            st.flush_list();
            st.flush_blockquote();
            st.sb.push_str(
                "<hr />
",
            );
            continue;
        }

        // 引用 >（非引用行在后续分支前 flush）
        if trimmed.starts_with('>') {
            st.flush_list();
            st.in_blockquote = true;
            let bq_content = trimmed.strip_prefix('>').unwrap_or("").trim();
            st.bq_lines.push(bq_content.to_owned());
            continue;
        }
        st.flush_blockquote();

        // 标题 # ~ ######
        if trimmed.starts_with('#') {
            st.flush_list();
            let bytes = trimmed.as_bytes();
            let mut level = 0;
            while level < bytes.len() && bytes[level] == b'#' {
                level += 1;
            }
            if level <= 6 && level < bytes.len() && bytes[level] == b' ' {
                let header_text = trimmed[level..].trim();
                st.sb.push_str(&format!(
                    "<h{level}>{}</h{level}>
",
                    render_inline(header_text)
                ));
                continue;
            }
        }

        // 无序列表 - / * / +
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
            if !st.in_list || st.list_type != "ul" {
                st.flush_list();
                st.in_list = true;
                st.list_type = "ul".to_owned();
                st.sb.push_str(
                    "<ul>
",
                );
            }
            let item_text = trimmed[2..].trim();
            st.sb.push_str(&format!(
                "<li>{}</li>
",
                render_inline(item_text)
            ));
            continue;
        }

        // 有序列表 1. / 2.
        if is_ordered_list(trimmed) {
            if !st.in_list || st.list_type != "ol" {
                st.flush_list();
                st.in_list = true;
                st.list_type = "ol".to_owned();
                st.sb.push_str(
                    "<ol>
",
                );
            }
            let dot = trimmed.find('.').expect("isOrderedList 已确认存在");
            let item_text = trimmed[dot + 1..].trim();
            st.sb.push_str(&format!(
                "<li>{}</li>
",
                render_inline(item_text)
            ));
            continue;
        }

        st.flush_list();

        // 普通段落
        st.sb.push_str(&format!(
            "<p>{}</p>
",
            render_inline(trimmed)
        ));
    }

    st.flush_list();
    st.flush_blockquote();
    if st.in_code_block {
        // 未闭合代码块：无语言标注（对齐 Go）。
        st.flush_code_block(false);
    }

    st.sb.trim().to_owned()
}

/// 将 Markdown 渲染为包含完整 HTML 骨架的文档（对齐 Go `RenderFullDocument`）。
pub fn render_full_document(md: &str, title: &str) -> String {
    let body = render(md);
    let title = if title.is_empty() {
        "Aluka Static Document"
    } else {
        title
    };
    format!(
        "<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n    <meta charset=\"UTF-8\">\n    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n    <title>{}</title>\n</head>\n<body>\n    <main class=\"markdown-body\">\n{}\n    </main>\n</body>\n</html>",
        escape_html(title),
        body
    )
}

/// 构建 markdown 模块对象（`markdown` 与 `aluka:markdown` 共用）。
fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = vm.alloc_ordinary();
    for (prop, name) in [
        ("render", "markdown.render"),
        ("renderToHTML", "markdown.renderToHTML"),
        ("parseFrontmatter", "markdown.parseFrontmatter"),
    ] {
        let fn_ref = vm.alloc_native_fn(name);
        set_module_prop(vm, obj, prop, Value::Object(fn_ref))?;
    }
    // 两个 specifier 的分派键都登记（共享同一模块单例）。
    for prefix in ["markdown", "aluka:markdown"] {
        register_handler(registry, prefix, "render", render_handler);
        register_handler(registry, prefix, "renderToHTML", render_to_html);
        register_handler(
            registry,
            prefix,
            "parseFrontmatter",
            parse_frontmatter_handler,
        );
    }
    Ok(obj)
}

/// ALUKA_MODULE 复用 MODULE 单例（Go 实测两 specifier 取到同一对象）；
/// 注册序保证 `markdown` 先建（`builtin_modules!` 数组序），未命中时兜底自建。
fn build_shared(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    match registry.module("markdown") {
        Some(existing) => Ok(existing),
        None => build(vm, registry),
    }
}

/// `render(md)`：无参返回空串（对齐 Go）。
fn render_handler(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() {
        return Ok(Value::Object(vm.alloc_string(String::new())));
    }
    let html_str = render(&vm.format_value(args[0]));
    Ok(Value::Object(vm.alloc_string(html_str)))
}

/// `renderToHTML(md, options?)`：options.title 缺省 "Aluka Site"。
fn render_to_html(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    if args.is_empty() {
        return Ok(Value::Object(vm.alloc_string(String::new())));
    }
    let mut title = "Aluka Site".to_owned();
    if let Some(r) = args.get(1).as_object {
        if matches!(vm.heap.get(r.index()), Some(HeapObject::Ordinary { .. })) {
            let t = vm.get_property(args[1], "title")?;
            if !matches!(t, Value::Undefined) {
                title = vm.format_value(t);
            }
        }
    }
    let doc = render_full_document(&vm.format_value(args[0]), &title);
    Ok(Value::Object(vm.alloc_string(doc)))
}

/// `parseFrontmatter(content)`：返回 `{ data, content }`。
fn parse_frontmatter_handler(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let (data, body) = match args.first() {
        None => (Vec::new(), String::new()),
        Some(v) => parse_frontmatter(&vm.format_value(*v)),
    };
    let data_obj = vm.alloc_ordinary();
    for (k, v) in data {
        let val = Value::Object(vm.alloc_string(v));
        let _ = vm.set_property(Value::Object(data_obj), &k, val);
    }
    let res = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(res), "data", Value::Object(data_obj));
    let content = Value::Object(vm.alloc_string(body));
    let _ = vm.set_property(Value::Object(res), "content", content);
    Ok(Value::Object(res))
}

/// 编译期锚定：处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = render_handler;
        let _: crate::builtins::BuiltinHandler = render_to_html;
        let _: crate::builtins::BuiltinHandler = parse_frontmatter_handler;
    }

    /// Markdown 渲染实测样张。
    #[test]
    fn render_samples_conform() {
        assert_eq!(
            render("# Hello\n\nWorld *em* **strong** `code` ~~del~~"),
            "<h1>Hello</h1>\n<p>World <em>em</em> <strong>strong</strong> <code>code</code> <del>del</del></p>"
        );
        assert_eq!(
            render("- a\n- b\n\n1. x\n2. y\n"),
            "<ul>\n<li>a</li>\n<li>b</li>\n</ul>\n<ol>\n<li>x</li>\n<li>y</li>\n</ol>"
        );
        assert_eq!(
            render("```js\nlet a = 1;\n<tag>\n```"),
            "<pre><code class=\"language-js\">let a = 1;\n&lt;tag&gt;</code></pre>"
        );
        assert_eq!(
            render("> quote line\n> more"),
            "<blockquote><p>quote line more</p></blockquote>"
        );
        assert_eq!(render("---\n\ntext"), "<hr />\n<p>text</p>");
        assert_eq!(
            render("[link](http://x.com?a=1&b=2) and ![img](/i.png)"),
            "<p><a href=\"http://x.com?a=1&amp;b=2\">link</a> and <img src=\"/i.png\" alt=\"img\" /></p>"
        );
        assert_eq!(render("###  no-space"), "<h3>no-space</h3>");
        assert_eq!(
            render("+ plus item\n* star item\n- dash item"),
            "<ul>\n<li>plus item</li>\n<li>star item</li>\n<li>dash item</li>\n</ul>"
        );
        assert_eq!(render("10. ten"), "<ol>\n<li>ten</li>\n</ol>");
        assert_eq!(render("a * b * c"), "<p>a <em> b </em> c</p>");
        assert_eq!(render("1.no space"), "<p>1.no space</p>");
        assert_eq!(render("#### h4 ###tricky"), "<h4>h4 ###tricky</h4>");
        assert_eq!(render(""), "");
        assert_eq!(render("para one\n"), "<p>para one</p>");
    }

    /// frontmatter 与整文档骨架（Node.js 22 LTS 标准 实测值）。
    #[test]
    fn frontmatter_and_full_document_match_go() {
        let (data, body) = parse_frontmatter("---\ntitle: T1\nnum: 42\n# comment\n---\nbody here");
        assert_eq!(
            data,
            vec![
                ("title".to_owned(), "T1".to_owned()),
                ("num".to_owned(), "42".to_owned())
            ]
        );
        assert_eq!(body, "body here");
        let (data2, body2) = parse_frontmatter("---\nq: \"quoted v\"\ns: 'sq'\n---\nB");
        assert_eq!(
            data2,
            vec![
                ("q".to_owned(), "quoted v".to_owned()),
                ("s".to_owned(), "sq".to_owned())
            ]
        );
        assert_eq!(body2, "B");
        let (none, content) = parse_frontmatter("no frontmatter");
        assert!(none.is_empty());
        assert_eq!(content, "no frontmatter");

        let doc = render_full_document("# Ti\ntext", "My T");
        assert!(doc.starts_with(
            "<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n    <meta charset=\"UTF-8\">"
        ));
        assert!(doc.contains("<title>My T</title>"));
        assert!(doc.contains("<h1>Ti</h1>\n<p>text</p>"));
        assert!(render_full_document("x", "").contains("<title>Aluka Static Document</title>"));
    }
}
