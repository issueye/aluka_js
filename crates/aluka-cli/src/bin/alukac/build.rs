//! `alukac build`：依赖图遍历构建（express 级 node_modules 链路）。
//!
//! 从入口出发解析 require 闭包，编译为镜像 .bc 树：
//! - 相对 require 相对当前文件目录；裸包名沿 node_modules 逐级向上
//!   （Node 解析语义）；包根读 package.json `main`（缺省 index.js）；
//! - 产物镜像源码相对布局：`node_modules/x/index.js` →
//!   `<outdir>/node_modules/x/index.bc`；`.json` 原样拷贝（require
//!   运行时解析为对象）；
//! - 内置模块（fs/path/node:* 等）跳过；解析失败的说明符静默跳过
//!   （动态 require 或误报；运行期真缺失时由 vm 报 Cannot find module）。
//!
//! 与 vm 侧 `resolve_module`（aluka-vm/src/modules.rs）的候选规则互为
//! 镜像，两侧需同步演化。

use aluka_compiler::{compile_source_unit, optimize_ast};
use aluka_parser::source_unit::{LanguageRegistry, ModuleKind};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

/// 构建入口：编译 require 闭包到镜像 .bc 树。
pub fn run_build(input: &Path, output: Option<&Path>, optimize: bool) -> ExitCode {
    let input_abs = absolutize(input);
    let root = input_abs
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let outdir = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join("aluka_build"));
    if let Err(e) = std::fs::create_dir_all(&outdir) {
        eprintln!("错误: 无法创建输出目录 {}: {e}", outdir.display());
        return ExitCode::FAILURE;
    }

    let mut queue: Vec<PathBuf> = vec![input_abs.clone()];
    let mut visited: Vec<PathBuf> = Vec::new();
    let mut compiled = 0usize;
    let mut copied = 0usize;
    let mut failed: Vec<String> = Vec::new();

    while let Some(file) = queue.pop() {
        if visited.contains(&file) {
            continue;
        }
        visited.push(file.clone());

        let src = match std::fs::read_to_string(&file) {
            Ok(s) => s,
            Err(e) => {
                failed.push(format!("{}: 读取失败 {e}", file.display()));
                continue;
            }
        };

        // require 扫描 → 解析 → 入队 / 拷贝 .json
        for spec in scan_requires(&src) {
            if let Some(dep) = resolve_require(&file, &spec) {
                if dep.extension().and_then(|e| e.to_str()) == Some("json") {
                    let dest = outdir.join(rel_from(&dep, &root));
                    if let Some(parent) = dest.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if std::fs::copy(&dep, &dest).is_ok() {
                        copied += 1;
                    }
                    continue;
                }
                // 裸包名依赖：镜像其 package.json（运行期 main 回退需要）
                if !spec.starts_with('.') {
                    let mut comps = dep.components().collect::<Vec<_>>();
                    while let Some(pos) = comps.iter().position(|c| c.as_os_str() == "node_modules")
                    {
                        if pos + 1 >= comps.len() {
                            break;
                        }
                        let pkg_root: PathBuf = comps[..=pos + 1].iter().collect();
                        let pkg_json = pkg_root.join("package.json");
                        if pkg_json.is_file() {
                            let dest = outdir.join(rel_from(&pkg_json, &root));
                            if let Some(parent) = dest.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            let _ = std::fs::copy(&pkg_json, &dest);
                        }
                        // 继续向上找嵌套 node_modules
                        comps.drain(..=pos);
                    }
                }
                queue.push(dep);
            }
        }

        if let Err(err) = compile_one(&file, &outdir, &root, optimize) {
            failed.push(format!("{}: {err}", file.display()));
            continue;
        }
        compiled += 1;
    }

    let entry_rel = rel_from(&input_abs, &root);
    let entry_bc = outdir.join(entry_rel).with_extension("bc");
    println!(
        "构建完成: 编译 {compiled} 个模块, 拷贝 {copied} 个 .json, 失败 {}",
        failed.len()
    );
    for f in failed.iter().take(10) {
        eprintln!("  失败: {f}");
    }
    println!("入口: {}", entry_bc.display());
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// 编译单个源文件到镜像 .bc 路径。
fn compile_one(file: &Path, outdir: &Path, root: &Path, optimize: bool) -> Result<(), String> {
    let rel = rel_from(file, root);
    let dest = outdir.join(rel).with_extension("bc");
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let module_kind = match file
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mjs" | "mts") => ModuleKind::Esm,
        _ => ModuleKind::CommonJs,
    };
    let path_str = file.to_string_lossy();
    let mut unit = LanguageRegistry::global()
        .parse_file(&path_str, module_kind)
        .map_err(|e| format!("解析失败: {e}"))?;
    if optimize {
        if let Some(prog) = &mut unit.program {
            optimize_ast(prog);
        }
    }
    let module = compile_source_unit(&mut unit).map_err(|e| format!("编译失败: {e}"))?;
    module.verify().map_err(|e| format!("校验失败: {e}"))?;
    std::fs::write(&dest, module.serialize()).map_err(|e| format!("写出失败: {e}"))?;
    Ok(())
}

/// require 说明符解析（编译侧，与 vm resolve_module 镜像布局一致）。
fn resolve_require(from: &Path, spec: &str) -> Option<PathBuf> {
    const BUILTINS: &[&str] = &[
        "assert",
        "assert/strict",
        "async_hooks",
        "buffer",
        "child_process",
        "cluster",
        "console",
        "constants",
        "crypto",
        "dgram",
        "diagnostics_channel",
        "dns",
        "domain",
        "events",
        "fs",
        "http",
        "http2",
        "https",
        "inspector",
        "module",
        "net",
        "os",
        "path",
        "path/posix",
        "path/win32",
        "perf_hooks",
        "process",
        "punycode",
        "querystring",
        "readline",
        "repl",
        "stream",
        "stream/consumers",
        "stream/web",
        "string_decoder",
        "sys",
        "test",
        "timers",
        "tls",
        "trace_events",
        "tty",
        "url",
        "util",
        "util/types",
        "v8",
        "vm",
        "worker_threads",
        "zlib",
    ];
    let bare = spec.strip_prefix("node:").unwrap_or(spec);
    if BUILTINS.contains(&bare) {
        return None;
    }
    if spec.starts_with("./") || spec.starts_with("../") || spec.starts_with('/') {
        let base = from
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        source_candidates(&normalize_components(&base.join(spec)))
    } else if spec.starts_with('#') {
        // `#alias`：最近 package.json 的 `imports` 条件映射（M2.1）
        let base = from
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        for dir in base.ancestors() {
            let pkg = dir.join("package.json");
            if !pkg.is_file() {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&pkg) {
                if let Some(parsed) = aluka_module::parse_json(&text) {
                    if let Some(imports) = parsed.get("imports") {
                        if let Some(target) = aluka_module::resolve_imports(
                            imports,
                            spec,
                            aluka_module::ConditionKind::Require,
                        ) {
                            let joined = normalize_components(&dir.join(target));
                            if let Some(p) = source_candidates(&joined) {
                                return Some(p);
                            }
                        }
                    }
                }
            }
        }
        None
    } else {
        // 裸说明符：exports 条件映射优先，无 exports 回退 main/index
        let (name, subpath) = aluka_module::split_package_specifier(bare);
        for dir in from.ancestors() {
            let pkg_root = normalize_components(&dir.join("node_modules").join(&name));
            if !pkg_root.is_dir() {
                continue;
            }
            let pkg_json = pkg_root.join("package.json");
            if pkg_json.is_file() {
                if let Ok(text) = std::fs::read_to_string(&pkg_json) {
                    if let Some(parsed) = aluka_module::parse_json(&text) {
                        if let Some(exports) = parsed.get("exports") {
                            if let Some(target) = aluka_module::resolve_exports(
                                exports,
                                &subpath,
                                aluka_module::ConditionKind::Require,
                            ) {
                                let joined = normalize_components(&pkg_root.join(target));
                                if let Some(p) = source_candidates(&joined) {
                                    return Some(p);
                                }
                                continue;
                            }
                            if subpath != "." {
                                continue;
                            }
                        }
                    }
                }
            }
            let pkg_dir = if subpath == "." {
                pkg_root.clone()
            } else {
                normalize_components(&pkg_root.join(subpath.strip_prefix("./").unwrap_or(&subpath)))
            };
            if let Some(p) = source_candidates(&normalize_components(&pkg_dir)) {
                return Some(p);
            }
        }
        None
    }
}

/// 源文件候选：精确 → +.js → +.cjs → /index.js → /index.cjs → package.json main。
fn source_candidates(p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    // Node CJS 解析语义：扩展名是**追加**而非替换（`./util.inspect` →
    // `util.inspect.js`；`Path::set_extension` 会把含点文件名误拆为
    // `util.js`，故这里用 OsString 追加）
    let mut js = p.as_os_str().to_os_string();
    js.push(".js");
    let js = PathBuf::from(js);
    if js.is_file() {
        return Some(js);
    }
    let mut cjs = p.as_os_str().to_os_string();
    cjs.push(".cjs");
    let cjs = PathBuf::from(cjs);
    if cjs.is_file() {
        return Some(cjs);
    }
    let index = p.join("index.js");
    if index.is_file() {
        return Some(index);
    }
    let index_cjs = p.join("index.cjs");
    if index_cjs.is_file() {
        return Some(index_cjs);
    }
    // 包根：package.json "main"（轻量字段扫描）
    let pkg = p.join("package.json");
    if pkg.is_file() {
        if let Ok(text) = std::fs::read_to_string(&pkg) {
            if let Some(main) = extract_json_string_field(&text, "main") {
                let main_path = normalize_components(&p.join(main.trim()));
                if let Some(found) = source_candidates(&main_path) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// 从 JSON 文本提取顶层字符串字段（轻量扫描，不引入 serde）。
fn extract_json_string_field(text: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    let quote = after.chars().next()?;
    if quote != '"' {
        return None;
    }
    let end = after[1..].find('"')?;
    Some(after[1..1 + end].to_owned())
}

/// require('lit') 说明符扫描：跳过注释与字符串字面量的轻量状态机。
/// 动态拼接的 require 无法静态解析，运行期由 vm 报缺失。
fn scan_requires(src: &str) -> Vec<String> {
    let b: Vec<char> = src.chars().collect();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        let c = b[i];
        if c == '/' && i + 1 < n && b[i + 1] == '/' {
            while i < n && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && b[i + 1] == '*' {
            i += 2;
            while i + 1 < n && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(n);
            continue;
        }
        if c == '"' || c == '\'' || c == '`' {
            let quote = c;
            i += 1;
            while i < n && b[i] != quote {
                if b[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if c == 'f'
            && b[i..].starts_with(&['f', 'r', 'o', 'm'][..])
            && (i == 0 || !is_ident_char(b[i - 1]))
        {
            // ESM `from "spec"`：import ... from / export ... from 的依赖扫描
            let mut j = i + 4;
            while j < n && b[j].is_whitespace() {
                j += 1;
            }
            if j < n && (b[j] == '\'' || b[j] == '"') {
                let q = b[j];
                j += 1;
                let start = j;
                while j < n && b[j] != q {
                    j += 1;
                }
                if j < n {
                    let spec: String = b[start..j].iter().collect();
                    out.push(spec);
                    i = j + 1;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if c == 'i'
            && b[i..].starts_with(&['i', 'm', 'p', 'o', 'r', 't'][..])
            && (i == 0 || !is_ident_char(b[i - 1]))
        {
            // 裸副作用导入：import "spec";
            let mut j = i + 6;
            while j < n && b[j].is_whitespace() {
                j += 1;
            }
            if j < n && (b[j] == '\'' || b[j] == '"') {
                let q = b[j];
                j += 1;
                let start = j;
                while j < n && b[j] != q {
                    j += 1;
                }
                if j < n {
                    let spec: String = b[start..j].iter().collect();
                    out.push(spec);
                    i = j + 1;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if c == 'r'
            && b[i..].starts_with(&['r', 'e', 'q', 'u', 'i', 'r', 'e'][..])
            && (i == 0 || !is_ident_char(b[i - 1]))
            && i + 7 < n
            && !is_ident_char(b[i + 7])
        {
            let mut j = i + 7;
            while j < n && b[j].is_whitespace() {
                j += 1;
            }
            if j < n && b[j] == '(' {
                j += 1;
                while j < n && b[j].is_whitespace() {
                    j += 1;
                }
                if j < n && (b[j] == '\'' || b[j] == '"') {
                    let quote = b[j];
                    j += 1;
                    let start = j;
                    while j < n && b[j] != quote {
                        if b[j] == '\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                    if j < n {
                        out.push(b[start..j].iter().collect());
                        i = j + 1;
                        continue;
                    }
                }
            }
            i += 7;
            continue;
        }
        i += 1;
    }
    out
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// 绝对化路径。
fn absolutize(p: &Path) -> PathBuf {
    if p.is_absolute() {
        normalize_components(p)
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        normalize_components(&cwd.join(p))
    }
}

/// 相对化（词法前缀剥离；root 之外的文件退化为 `_ext/<文件名>`）。
fn rel_from(p: &Path, root: &Path) -> PathBuf {
    match p.strip_prefix(root) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => PathBuf::from("_ext").join(p.file_name().unwrap_or_default()),
    }
}

/// 词法规范化路径（折叠 `.`/`..`）。
fn normalize_components(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
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
    out
}
