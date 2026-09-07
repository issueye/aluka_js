//! 源码单元：按语言注册的解析入口、模块协议与变换阶段。
//!
//! 基于现代运行时规范设计：
//!
//! - **语言层** [`SourceKind`]：按规范化扩展名识别（`.ts/.mts/.cts` → TS、
//!   `.json` → JSON、其余 → JavaScript），与模块协议相互独立；
//! - **协议层** [`ModuleKind`]：Script / ESM / CommonJS；
//! - **注册表** [`LanguageRegistry`]：扩展名 → 语言的注册表，默认覆盖 JS/TS/JSON，可注册新语言；
//! - **中间表示** [`SourceUnit`]：JS/TS 前端与模块/优化后端之间的稳定 IR，
//!   携带 [`TransformStage`] 单向阶段位标志（只增不减，防止 pass 乱序/重复）；
//! - **TS policy**：strip-only 模式不支持非 declare 的 `enum`/`namespace` 声明
//!   （与 Node 22 type stripping 诊断一致），加载前 token 级检测。
//!
//! 注：[`SourceUnit::has_tla`] 检测顶层 await 状态；`StageTypeStripped`
//! 表示"AST 构建时已按 strip-only 语义处理"，实际的类型剥离发生在 `aluka-compiler`。

use crate::ast::Program;
use crate::lexer::Lexer;
use crate::parser::Parser;
use std::collections::HashMap;
use std::path::Path;

/// 源码语言层（按扩展名注册识别）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// JavaScript
    JavaScript,
    /// TypeScript（类型注解 strip-only）
    TypeScript,
    /// JSON（不做 JS 解析，延迟处理）
    Json,
    /// 自定义 DSL（S-expression / Lisp 等专用领域语言）
    Dsl,
}

impl SourceKind {
    /// 语言名（诊断与缓存键用）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TypeScript => "typescript",
            Self::Json => "json",
            Self::JavaScript => "javascript",
            Self::Dsl => "dsl",
        }
    }
}

/// 模块封装与执行协议（与语言层相互独立）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    /// 普通脚本
    Script,
    /// ES 模块（import/export，允许顶层 await）
    Esm,
    /// CommonJS（require/exports 包装）
    CommonJs,
}

impl ModuleKind {
    /// 协议名（诊断用）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Esm => "esm",
            Self::CommonJs => "cjs",
            Self::Script => "script",
        }
    }
}

/// 单向变换阶段位标志（只增不减）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransformStage(pub u16);

impl std::ops::BitOr for TransformStage {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for TransformStage {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// 已完成前端解析（`program` 已填充）。
pub const STAGE_PARSED: TransformStage = TransformStage(1 << 0);
/// TypeScript 类型已在 AST 构建时按 strip-only 语义处理。
pub const STAGE_TYPE_STRIPPED: TransformStage = TransformStage(1 << 1);
/// 已做摇树优化（tree shaking）。
pub const STAGE_SHAKEN: TransformStage = TransformStage(1 << 2);
/// 已压缩。
pub const STAGE_MINIFIED: TransformStage = TransformStage(1 << 3);
/// ESM 已降级为 CJS 语义。
pub const STAGE_ESM_LOWERED: TransformStage = TransformStage(1 << 4);
/// 已做模块包装（wrap）。
pub const STAGE_WRAPPED: TransformStage = TransformStage(1 << 5);
/// 字节码已做编译期优化。
pub const STAGE_BYTECODE_OPTIMIZED: TransformStage = TransformStage(1 << 6);
/// 字节码已生成编译（BytecodeModule 已构造完成）。
pub const STAGE_BYTECODE_COMPILED: TransformStage = TransformStage(1 << 7);

/// 源码单元构造/解析错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceUnitError {
    /// TS strip-only 模式遇到不支持的声明（与 Node 22 的
    /// ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX 诊断对齐）。
    UnsupportedTsSyntax {
        /// 文件路径
        path: String,
        /// 1 起始的行号
        line: usize,
        /// 诊断消息
        message: String,
    },
    /// 无法读取源文件。
    ReadError {
        /// 文件路径
        path: String,
        /// 底层 IO 错误文本
        message: String,
    },
    /// 阶段标志乱序或重复。
    StageAlreadyApplied {
        /// 文件路径
        path: String,
        /// 试图标记的阶段值
        stage: u16,
        /// 单元当前阶段值
        current: u16,
    },
    /// 单元缺失要求的阶段。
    MissingStages {
        /// 文件路径
        path: String,
        /// 缺失的阶段位集
        missing: u16,
        /// 单元当前阶段值
        current: u16,
    },
    /// 未知语言扩展名（注册表未登记且无默认分类）。
    UnknownExtension {
        /// 文件路径
        path: String,
        /// 规范化后的扩展名
        extension: String,
    },
    /// 源码语法错误（解析容错无法掩盖的非法文法）。
    SyntaxError {
        /// 文件路径
        path: String,
        /// 全部语法错误（`; ` 连接）
        message: String,
    },
}

impl std::fmt::Display for SourceUnitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedTsSyntax {
                path,
                line,
                message,
            } => {
                write!(f, "{path}:{line}: {message}")
            }
            Self::ReadError { path, message } => write!(f, "cannot read {path}: {message}"),
            Self::StageAlreadyApplied {
                path,
                stage,
                current,
            } => {
                write!(
                    f,
                    "{path}: transform stage {stage} already applied (current {current})"
                )
            }
            Self::MissingStages {
                path,
                missing,
                current,
            } => {
                write!(
                    f,
                    "{path}: missing required transform stages {missing} (current {current})"
                )
            }
            Self::UnknownExtension { path, extension } => {
                write!(f, "{path}: unknown source language extension '{extension}'")
            }
            Self::SyntaxError { path, message } => {
                write!(f, "SyntaxError: {message} ({path})")
            }
        }
    }
}

impl std::error::Error for SourceUnitError {}

/// JS/TS 前端与模块/优化后端之间的稳定中间表示。
///
/// `program` 在一条构建管线内由单一所有者按阶段原地变换；JSON 单元不做
/// JS 解析（`program` 为 `None`，由消费方按 `SourceKind::Json` 处理）。
#[derive(Debug)]
pub struct SourceUnit {
    /// 模块标识（虚拟路径）
    pub path: String,
    /// 原始源码（已剥 BOM）
    pub source: String,
    /// 语言层
    pub source_kind: SourceKind,
    /// 模块协议
    pub module_kind: ModuleKind,
    /// JS AST（JSON 单元为 `None`）
    pub program: Option<Program>,
    /// 是否含顶层 await（AST await 解析落地前恒为 `false`）
    pub has_tla: bool,
    /// 已完成的变换阶段
    pub stage: TransformStage,
}

impl SourceUnit {
    /// 把阶段标记到单元上（只增不减）。重复标记返回诊断，防止 pass
    /// 乱序/重复执行破坏单向阶段流。
    pub fn mark_stage(&mut self, stage: TransformStage) -> Result<(), SourceUnitError> {
        if self.stage.0 & stage.0 != 0 {
            return Err(SourceUnitError::StageAlreadyApplied {
                path: self.path.clone(),
                stage: stage.0,
                current: self.stage.0,
            });
        }
        self.stage.0 |= stage.0;
        Ok(())
    }

    /// 校验单元已完成全部给定阶段；缺失时返回诊断。
    pub fn require_stages(&self, stages: TransformStage) -> Result<(), SourceUnitError> {
        let missing = stages.0 & !self.stage.0;
        if missing != 0 {
            return Err(SourceUnitError::MissingStages {
                path: self.path.clone(),
                missing,
                current: self.stage.0,
            });
        }
        Ok(())
    }
}

/// 仅按规范化扩展名识别语言层（无注册表时的默认分类，语义等价
/// [`LanguageRegistry::with_defaults`] 的判定）。
#[must_use]
pub fn detect_source_kind(path: &str) -> SourceKind {
    match normalized_extension(path).as_str() {
        "ts" | "mts" | "cts" | "tsx" => SourceKind::TypeScript,
        "json" => SourceKind::Json,
        "adsl" | "lisp" => SourceKind::Dsl,
        _ => SourceKind::JavaScript,
    }
}

/// 规范化扩展名（小写、不含点）。
fn normalized_extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// 剥削 UTF-8 BOM（`\u{FEFF}`）。
fn strip_bom(src: &str) -> &str {
    src.strip_prefix('\u{FEFF}').unwrap_or(src)
}

/// 扩展名 → 语言的注册表（对齐 Go loader 的 `require.extensions` 设计）。
///
/// 默认注册表覆盖 `.js/.mjs/.cjs` → JS、`.ts/.mts/.cts` → TS、
/// `.json` → JSON、`.adsl/.lisp` → DSL；新语言经 [`LanguageRegistry::register`] 注册后即可被
/// [`LanguageRegistry::classify`] 识别。
#[derive(Debug, Default, Clone)]
pub struct LanguageRegistry {
    by_extension: HashMap<String, SourceKind>,
}

impl LanguageRegistry {
    /// 创建带默认语言注册的表。
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut reg = Self::empty();
        for ext in ["js", "mjs", "cjs"] {
            reg.register(ext, SourceKind::JavaScript);
        }
        for ext in ["ts", "mts", "cts"] {
            reg.register(ext, SourceKind::TypeScript);
        }
        reg.register("json", SourceKind::Json);
        for ext in ["adsl", "lisp"] {
            reg.register(ext, SourceKind::Dsl);
        }
        reg
    }

    /// 创建空表（不含任何语言，适合全自定义场景）。
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// 注册/覆盖一个扩展名的语言。
    pub fn register(&mut self, extension: &str, kind: SourceKind) {
        self.by_extension
            .insert(extension.to_ascii_lowercase(), kind);
    }

    /// 查询扩展名注册的语言。
    #[must_use]
    pub fn kind_of(&self, extension: &str) -> Option<SourceKind> {
        let ext = extension.trim_start_matches('.').to_ascii_lowercase();
        self.by_extension.get(&ext).copied()
    }

    /// 按注册表分类文件路径；未注册扩展名回退 [`SourceKind::JavaScript`]
    /// （与 Go 版 `DetectSourceKind` 的 default 分支一致）。
    #[must_use]
    pub fn classify(&self, path: &str) -> SourceKind {
        self.kind_of(&normalized_extension(path))
            .unwrap_or(SourceKind::JavaScript)
    }

    /// 按注册表严格分类；未注册扩展名返回诊断（严格模式供工具链用）。
    pub fn classify_strict(&self, path: &str) -> Result<SourceKind, SourceUnitError> {
        let ext = normalized_extension(path);
        self.kind_of(&ext)
            .ok_or_else(|| SourceUnitError::UnknownExtension {
                path: path.to_owned(),
                extension: ext,
            })
    }

    /// 获取全局默认注册表单例。
    #[must_use]
    pub fn global() -> &'static Self {
        static REGISTRY: std::sync::OnceLock<LanguageRegistry> = std::sync::OnceLock::new();
        REGISTRY.get_or_init(Self::with_defaults)
    }

    /// 使用当前注册表的语言分类规则解析源码为 SourceUnit。
    pub fn parse_source(
        &self,
        src: &str,
        path: &str,
        module_kind: ModuleKind,
    ) -> Result<SourceUnit, SourceUnitError> {
        let src = strip_bom(src);
        let source_kind = self.classify(path);
        if source_kind == SourceKind::Json {
            return Ok(SourceUnit {
                path: path.to_owned(),
                source: src.to_owned(),
                source_kind,
                module_kind,
                program: None,
                has_tla: false,
                stage: TransformStage(0),
            });
        }
        if source_kind == SourceKind::Dsl {
            return Ok(SourceUnit {
                path: path.to_owned(),
                source: src.to_owned(),
                source_kind,
                module_kind,
                program: None,
                has_tla: false,
                stage: STAGE_PARSED,
            });
        }
        check_unsupported_ts(src, path)?;
        let mut parser = Parser::new(src);
        let program = parser.parse_program();
        let errors = parser.take_errors();
        if !errors.is_empty() {
            return Err(SourceUnitError::SyntaxError {
                path: path.to_owned(),
                message: errors.join("; "),
            });
        }
        let mut stage = STAGE_PARSED;
        if source_kind == SourceKind::TypeScript {
            stage = TransformStage(stage.0 | STAGE_TYPE_STRIPPED.0);
        }
        Ok(SourceUnit {
            path: path.to_owned(),
            source: src.to_owned(),
            source_kind,
            module_kind,
            program: Some(program),
            has_tla: false,
            stage,
        })
    }

    /// 使用当前注册表的语言分类规则从文件系统读取并解析源码。
    pub fn parse_file(
        &self,
        path: &str,
        module_kind: ModuleKind,
    ) -> Result<SourceUnit, SourceUnitError> {
        let src = std::fs::read_to_string(path).map_err(|e| SourceUnitError::ReadError {
            path: path.to_owned(),
            message: e.to_string(),
        })?;
        self.parse_source(&src, path, module_kind)
    }
}

/// TS strip-only 不支持的声明关键字（`declare` 前缀的环境声明豁免）。
const TS_DECL_MARKERS: [&str; 3] = ["enum", "namespace", "module"];

/// 对 TS 源码做 strip-only policy 检测：非 `declare` 前缀的
/// `enum` / `namespace` / `module` 声明报与 Node 22 一致的诊断。
///
/// 仅对 `.ts/.mts/.cts` 生效；
/// 词法错误不在本层报（交给解析器）。
fn check_unsupported_ts(src: &str, path: &str) -> Result<(), SourceUnitError> {
    let ext = normalized_extension(path);
    if !matches!(ext.as_str(), "ts" | "mts" | "cts") {
        return Ok(());
    }
    let mut lexer = Lexer::new(src);
    let mut prev_decl = false;
    loop {
        let token = lexer.next_token();
        if matches!(token.kind, crate::lexer::TokenKind::Eof) {
            return Ok(());
        }
        let is_marker = TS_DECL_MARKERS.iter().any(|m| token.text == *m);
        if is_marker && !prev_decl {
            let line = src[..token.start].matches('\n').count() + 1;
            let what = if token.text == "enum" {
                "TypeScript enum is not supported in strip-only mode"
            } else {
                "TypeScript namespace declaration is not supported in strip-only mode"
            };
            return Err(SourceUnitError::UnsupportedTsSyntax {
                path: path.to_owned(),
                line,
                message: what.to_owned(),
            });
        }
        prev_decl = token.text == "declare";
    }
}

/// 执行一次源码前端处理：BOM 剥离 → 扩展名分类 → JSON 特例 →
/// TS policy 检测 → JS 统一解析 → 阶段标记。
///
/// TS 文件标记 [`STAGE_TYPE_STRIPPED`]（AST 构建时已按 strip-only 语义
/// 识别类型注解；实际剥离发生在 `aluka-compiler`，与 Go 版分工一致）。
pub fn parse_source_unit(
    src: &str,
    path: &str,
    module_kind: ModuleKind,
) -> Result<SourceUnit, SourceUnitError> {
    LanguageRegistry::global().parse_source(src, path, module_kind)
}

/// 读取并解析一个源码文件（runtime loader 与 bundler 共用的统一前端入口）。
pub fn parse_file_unit(path: &str, module_kind: ModuleKind) -> Result<SourceUnit, SourceUnitError> {
    LanguageRegistry::global().parse_file(path, module_kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_classifies_by_normalized_extension() {
        assert_eq!(detect_source_kind("a.ts"), SourceKind::TypeScript);
        assert_eq!(detect_source_kind("b.MTS"), SourceKind::TypeScript);
        assert_eq!(detect_source_kind("c.tsx"), SourceKind::TypeScript);
        assert_eq!(detect_source_kind("d.json"), SourceKind::Json);
        assert_eq!(detect_source_kind("e.js"), SourceKind::JavaScript);
        assert_eq!(detect_source_kind("noext"), SourceKind::JavaScript);
    }

    #[test]
    fn registry_default_and_custom_registration() {
        let reg = LanguageRegistry::with_defaults();
        assert_eq!(reg.classify("a.ts"), SourceKind::TypeScript);
        assert_eq!(reg.classify("a.mjs"), SourceKind::JavaScript);
        assert_eq!(reg.classify("a.JSON"), SourceKind::Json);

        // 新语言注册后即可识别（如未来接入 .vue）
        let mut reg = reg;
        assert_eq!(
            reg.classify("app.vue"),
            SourceKind::JavaScript,
            "未注册回退 JS"
        );
        reg.register("vue", SourceKind::TypeScript);
        assert_eq!(reg.classify("app.vue"), SourceKind::TypeScript);

        // 严格模式：未注册返回诊断；已注册正常返回
        let strict = LanguageRegistry::empty();
        assert!(strict.classify_strict("a.json").is_err(), "空表无任何注册");
        assert_eq!(reg.classify_strict("a.json"), Ok(SourceKind::Json));
        assert!(reg.classify_strict("a.rs").is_err());
    }

    #[test]
    fn parse_source_unit_handles_json_and_bom() {
        let unit = parse_source_unit("{\"a\":1}", "cfg.json", ModuleKind::Script).expect("json");
        assert_eq!(unit.source_kind, SourceKind::Json);
        assert!(unit.program.is_none(), "JSON 不做 JS 解析");
        assert_eq!(unit.stage, TransformStage(0));

        let unit = parse_source_unit("\u{FEFF}let x = 1;", "b.js", ModuleKind::Script)
            .expect("js with BOM");
        assert_eq!(unit.source_kind, SourceKind::JavaScript);
        assert!(unit.program.is_some());
        assert_ne!(unit.stage, TransformStage(0));
        assert_eq!(unit.module_kind, ModuleKind::Script);
    }

    #[test]
    fn typescript_unit_marks_type_stripped_stage() {
        let src = "const n: number = 1;";
        let unit = parse_source_unit(src, "m.ts", ModuleKind::Esm).expect("ts parse");
        assert_eq!(unit.source_kind, SourceKind::TypeScript);
        unit.require_stages(STAGE_PARSED).expect("应有 PARSED 阶段");
        unit.require_stages(STAGE_TYPE_STRIPPED)
            .expect("TS 应标记 strip-only 阶段");
        assert_eq!(unit.module_kind, ModuleKind::Esm);
    }

    #[test]
    fn ts_enum_is_rejected_with_declare_exempt() {
        let err = parse_source_unit("enum Color { Red }", "e.ts", ModuleKind::Script)
            .expect_err("enum 应被 strip-only policy 拒绝");
        assert!(matches!(
            err,
            SourceUnitError::UnsupportedTsSyntax { line: 1, .. }
        ));

        // declare 前缀的环境声明豁免
        parse_source_unit("declare enum Color { Red }", "e2.ts", ModuleKind::Script)
            .expect("declare enum 应豁免");

        // 非 TS 扩展名不检测（enum 只是普通标识符场景交给解析器）
        parse_source_unit("let x = 1; // enum", "p.js", ModuleKind::Script)
            .expect("js 不做 TS policy 检测");
    }

    #[test]
    fn stage_marking_is_additive_and_detects_repeats() {
        let mut unit = parse_source_unit("let a = 1;", "s.js", ModuleKind::Script).expect("ok");
        unit.mark_stage(STAGE_SHAKEN).expect("首次标记应成功");
        assert!(unit.mark_stage(STAGE_SHAKEN).is_err(), "重复标记应报诊断");
        unit.mark_stage(STAGE_MINIFIED).expect("不同阶段可累加");
        unit.require_stages(STAGE_SHAKEN | STAGE_MINIFIED)
            .expect("阶段齐备");
        assert!(
            unit.require_stages(STAGE_WRAPPED).is_err(),
            "缺失阶段应报诊断"
        );
    }

    #[test]
    fn parse_file_unit_reads_from_disk() {
        let dir = std::env::temp_dir().join("aluka_source_unit_test");
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        let path = dir.join("unit_probe.ts");
        std::fs::write(&path, "const v: string = \"x\";").expect("写临时文件");

        let unit = parse_file_unit(path.to_str().expect("utf8 路径"), ModuleKind::CommonJs)
            .expect("文件单元解析");
        assert_eq!(unit.source_kind, SourceKind::TypeScript);
        assert_eq!(unit.module_kind, ModuleKind::CommonJs);
        assert!(unit.program.is_some());
    }
}
