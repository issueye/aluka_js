//! 源码文件的模块种类判定（Node.js 22 语义，前端与装配层共用的单一事实源）。
//!
//! Node 的判定规则：
//! 1. 扩展名显式声明优先——`.mjs`/`.mts` → ESM，`.cjs`/`.cts` → CommonJS；
//! 2. 其余（`.js`/`.ts`/`.jsx`/`.tsx`）按**最近的 `package.json`** 的
//!    `"type"` 字段：`"module"` → ESM，其余（含缺省）→ CommonJS。
//!
//! 该规则此前只在 `alukac build` 的镜像构建里成立，`aluka run` 的入口与
//! 依赖解析按扩展名硬判（`.ts` 一律当脚本），致 `"type": "module"` 项目
//! 的 ESM 语法在入口处被当脚本解析（import/export 绑定落空）。

use aluka_parser::source_unit::{LanguageRegistry, ModuleKind};
use std::path::Path;

/// 最近 `package.json` 的 `"type"` 是否为 `"module"`。
#[must_use]
pub fn package_type_is_module(file: &Path) -> bool {
    let mut dir = file.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        let pkg = d.join("package.json");
        if pkg.is_file()
            && let Ok(text) = std::fs::read_to_string(&pkg)
            && let Some(parsed) = aluka_module::parse_json(&text)
            && let Some(t) = parsed.get("type")
            && t.as_str() == Some("module")
        {
            return true;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    false
}

/// 最近 `package.json` 的显式 `"type"`（None = 无 package.json / 无 type 字段）。
fn explicit_package_type(file: &Path) -> Option<bool> {
    let mut dir = file.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        let pkg = d.join("package.json");
        if pkg.is_file()
            && let Ok(text) = std::fs::read_to_string(&pkg)
            && let Some(parsed) = aluka_module::parse_json(&text)
            && let Some(t) = parsed.get("type")
        {
            return Some(t.as_str() == Some("module"));
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    None
}

/// 源码是否含 **ESM-only 语法**：`export` 声明，或非动态调用的 `import`
/// 声明（`import(` 与 `import.meta` 在 CJS 里分别是合法表达式与非法但
/// 非声明的形态；Node detect-module 以「CJS 解析失败」为准，此处用等价
/// 的词法判定——import/export 是保留字，字符串/注释已由词法器消化）。
#[must_use]
fn source_has_esm_syntax(src: &str) -> bool {
    let mut lexer = aluka_parser::lexer::Lexer::new(src);
    let mut pending_import = false;
    loop {
        let token = lexer.next_token();
        match &token.kind {
            aluka_parser::lexer::TokenKind::Eof => return false,
            // `export` 声明；`import` 后随标识符/`{`/`*` 即 import 声明，
            // 后随 `(` 是动态导入（CJS 合法），后随 `.` 是 import.meta（ESM-only）
            aluka_parser::lexer::TokenKind::Ident(w)
            | aluka_parser::lexer::TokenKind::Keyword(w) => {
                if w == "export" {
                    return true;
                }
                if w == "import" {
                    pending_import = true;
                    continue;
                }
                if pending_import {
                    return true;
                }
                pending_import = false;
            }
            aluka_parser::lexer::TokenKind::Punct(p) => {
                if pending_import {
                    if p == "(" {
                        pending_import = false;
                        continue;
                    }
                    return true;
                }
            }
            _ => {
                pending_import = false;
            }
        }
    }
}

/// 按源码文本判定模块种类（入口与依赖解析共用）。
///
/// Node 22 语义：扩展名显式声明优先；`.js`/`.ts` 按最近 package.json 的
/// 显式 `"type"`；**无显式 type 时做 ESM 语法探测**（detect-module，
/// Node 22.7+ 默认开启）——含 `export` 声明或 `import` 声明的文件按 ESM
/// 装载，否则按 CommonJS。
#[must_use]
pub fn module_kind_for_source(file: &Path, src: &str) -> ModuleKind {
    match file
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mjs" | "mts") => ModuleKind::Esm,
        Some("cjs" | "cts") => ModuleKind::CommonJs,
        _ => match explicit_package_type(file) {
            Some(true) => ModuleKind::Esm,
            Some(false) => ModuleKind::CommonJs,
            None => {
                if source_has_esm_syntax(src) {
                    return ModuleKind::Esm;
                }
                // Node detect-module 的解析回退：CJS 解析失败且 ESM 解析成功
                // → 按 ESM（覆盖**顶层 await** 这类词法扫描看不到的形态）；
                // 两者都失败时维持 CJS（报 CJS 口径的语法错误）。
                let parses_as = |kind| {
                    LanguageRegistry::global()
                        .parse_source(src, &file.to_string_lossy(), kind)
                        .is_ok()
                };
                if parses_as(ModuleKind::CommonJs) {
                    ModuleKind::CommonJs
                } else if parses_as(ModuleKind::Esm) {
                    ModuleKind::Esm
                } else {
                    ModuleKind::CommonJs
                }
            }
        },
    }
}

/// 按 Node 语义判定源码文件的模块种类（只看路径与 package.json；
/// 无显式 type 时按 CommonJS——调用方拿到源码后应改用
/// [`module_kind_for_source`] 以获得语法探测）。
#[must_use]
pub fn module_kind_for_path(file: &Path) -> ModuleKind {
    match file
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mjs" | "mts") => ModuleKind::Esm,
        Some("cjs" | "cts") => ModuleKind::CommonJs,
        _ => {
            if package_type_is_module(file) {
                ModuleKind::Esm
            } else {
                ModuleKind::CommonJs
            }
        }
    }
}
