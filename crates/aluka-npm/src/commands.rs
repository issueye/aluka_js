//! 子命令编排：CLI 参数 → 安装/运行/查询动作。
//!
//! 职责边界：本模块只做参数归一、项目目录定位与结果呈现；算法在
//! [`crate::installer`] / [`crate::registry`] / [`crate::semver`]。

use std::path::{Path, PathBuf};

use crate::http::HttpClient;
use crate::installer::{self, InstallOptions};
use crate::registry;

/// CLI 错误（携带退出码语义）。
#[derive(Debug)]
pub struct CommandError {
    /// 展示给用户的错误文本
    pub message: String,
    /// 进程退出码（npm 惯例：1 = 一般错误）
    pub code: i32,
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        CommandError { message, code: 1 }
    }
}

/// 子命令分发入口。
///
/// # 参数
///
/// - `args`：子命令名及其后的全部参数（不含 `aluka-npm` 自身）；
/// - `cwd`：调用方工作目录（项目根定位起点）。
pub fn dispatch(args: &[String], cwd: &Path) -> Result<i32, CommandError> {
    let Some(cmd) = args.first().map(String::as_str) else {
        print_help();
        return Ok(0);
    };
    let rest = &args[1..];
    match cmd {
        "install" | "i" | "add" => cmd_install(rest, cwd),
        "uninstall" | "remove" | "rm" | "un" => cmd_uninstall(rest, cwd),
        "run" | "run-script" => cmd_run(rest, cwd),
        "ls" | "list" => cmd_ls(cwd),
        "init" => cmd_init(rest, cwd),
        "view" | "info" => cmd_view(rest, cwd),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(0)
        }
        "--version" | "-v" | "version" => {
            println!("aluka-npm {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        other => Err(CommandError {
            message: format!("未知命令: {other}（aluka-npm help 查看用法）"),
            code: 1,
        }),
    }
}

/// 从 `--registry` 旗标与环境变量解析 registry 地址。
fn pick_registry(args: &[String]) -> String {
    if let Some(pos) = args.iter().position(|a| a == "--registry") {
        if let Some(v) = args.get(pos + 1) {
            return v.clone();
        }
    }
    args.iter()
        .find_map(|a| a.strip_prefix("--registry="))
        .map(str::to_owned)
        .or_else(|| std::env::var("ALUKA_NPM_REGISTRY").ok())
        .or_else(|| std::env::var("NPM_CONFIG_REGISTRY").ok())
        .unwrap_or_else(|| "https://registry.npmjs.org".to_owned())
}

/// 过滤通用旗标后的位置参数。
fn positional(args: &[String]) -> Vec<String> {
    args.iter()
        .filter(|a| !a.starts_with('-'))
        .cloned()
        .collect()
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// `install`：无 spec = 依声明安装；带 spec = 安装并保存。
fn cmd_install(args: &[String], cwd: &Path) -> Result<i32, CommandError> {
    let project = find_project(cwd)?;
    let opts = InstallOptions {
        registry: pick_registry(args),
        omit_dev: has_flag(args, "--production") || has_flag(args, "--omit=dev"),
        no_save: has_flag(args, "--no-save"),
        ignore_scripts: has_flag(args, "--ignore-scripts"),
    };
    let specs = positional(args);
    let client = HttpClient::new();
    let report = installer::install(&project, &specs, &opts, &client)?;
    if report.lines.is_empty() {
        println!("up to date");
    } else {
        for line in &report.lines {
            println!("{line}");
        }
        println!(
            "added {} package{}{}",
            report.added,
            if report.added == 1 { "" } else { "s" },
            if report.reused > 0 {
                format!(", reused {}", report.reused)
            } else {
                String::new()
            }
        );
    }
    Ok(0)
}

/// `uninstall <name...>`。
fn cmd_uninstall(args: &[String], cwd: &Path) -> Result<i32, CommandError> {
    let project = find_project(cwd)?;
    let names = positional(args);
    if names.is_empty() {
        return Err(CommandError {
            message: "uninstall 需要至少一个包名".to_owned(),
            code: 1,
        });
    }
    installer::uninstall(&project, &names)?;
    println!("removed {}", names.join(", "));
    Ok(0)
}

/// `run <script> [-- args]`：执行 package.json 脚本（pre/post 钩子联动）。
fn cmd_run(args: &[String], cwd: &Path) -> Result<i32, CommandError> {
    let project = find_project(cwd)?;
    let script = positional(args)
        .first()
        .cloned()
        .ok_or_else(|| CommandError {
            message: "run 需要脚本名（aluka-npm run <script>）".to_owned(),
            code: 1,
        })?;
    let extra_args: Vec<String> = args
        .iter()
        .skip_while(|a| a.as_str() != "--")
        .skip(1)
        .cloned()
        .collect();
    let pkg = installer::read_package(&project)?;
    let Some(command) = pkg.scripts.get(&script).cloned() else {
        let available: Vec<&str> = pkg.scripts.keys().map(String::as_str).collect();
        return Err(CommandError {
            message: format!(
                "缺少脚本 `{script}`；可用脚本: {}",
                if available.is_empty() {
                    "（无）".to_owned()
                } else {
                    available.join(", ")
                }
            ),
            code: 1,
        });
    };
    // pre/post 钩子（npm 语义）
    for (phase, key) in [
        ("pre", format!("pre{script}")),
        ("post", format!("post{script}")),
    ] {
        let _ = phase;
        if let Some(hook) = pkg.scripts.get(&key) {
            let code = run_script(&project, hook, &[])?;
            if code != 0 {
                return Ok(code);
            }
        }
    }
    run_script(&project, &command, &extra_args)
}

/// 执行单条脚本（shell + PATH 前置根 .bin；继承 stdout/stderr）。
fn run_script(project: &Path, command: &str, extra_args: &[String]) -> Result<i32, CommandError> {
    let full = if extra_args.is_empty() {
        command.to_owned()
    } else {
        format!("{command} {}", extra_args.join(" "))
    };
    const SEP: &str = if cfg!(windows) { ";" } else { ":" };
    let path_env = std::env::var("PATH").unwrap_or_default();
    let bin_dir = project.join("node_modules/.bin").display().to_string();
    let full_path = format!("{bin_dir}{SEP}{path_env}");
    eprintln!("> {full}");
    #[cfg(windows)]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", &full]);
        c
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", &full]);
        c
    };
    command.current_dir(project).env("PATH", full_path);
    match command.status() {
        Ok(status) => Ok(status.code().unwrap_or(1)),
        Err(e) => Err(CommandError {
            message: format!("脚本进程启动失败: {e}"),
            code: 1,
        }),
    }
}

/// `ls`：安装树。
fn cmd_ls(cwd: &Path) -> Result<i32, CommandError> {
    let project = find_project(cwd)?;
    print!("{}", installer::list(&project)?);
    Ok(0)
}

/// `init -y`：默认 package.json。
///
/// npm 语义：`init` 作用于**当前目录**，不上溯祖先查找项目根——实测 npm 10.8.1：
/// `parent/package.json` 存在时在 `parent/child/` 执行 `npm init -y` 仍写入
/// `parent/child/package.json`。上溯（`npm prefix` 语义）只适用于
/// install/uninstall/run/ls，故此处**不得**复用 `find_project`：否则 cwd 位于任何
/// 含 package.json 的目录之下时，项目根会被解析到祖先并误报「package.json 已存在」。
fn cmd_init(args: &[String], cwd: &Path) -> Result<i32, CommandError> {
    if !has_flag(args, "-y") && !has_flag(args, "--yes") {
        return Err(CommandError {
            message: "当前仅支持 `aluka-npm init -y`（默认值初始化）".to_owned(),
            code: 1,
        });
    }
    installer::init(cwd)?;
    println!("已生成 package.json");
    Ok(0)
}

/// `view <name> [field]`：registry 元数据查询。
fn cmd_view(args: &[String], cwd: &Path) -> Result<i32, CommandError> {
    let _ = cwd;
    let spec = positional(args)
        .first()
        .cloned()
        .ok_or_else(|| CommandError {
            message: "view 需要包名（aluka-npm view <name>[@version] [字段]）".to_owned(),
            code: 1,
        })?;
    let field = positional(args).get(1).cloned();
    let registry_url = pick_registry(args);
    let (name, range) = registry::parse_spec(&spec)?;
    let client = HttpClient::new();
    let pack = registry::Packument::fetch(&client, &registry_url, &name)
        .map_err(|e| CommandError::from(format!("registry 查询失败: {e}")))?;
    // 解析目标版本：范围命中最高；无命中（如 `*` 无预发布匹配）回落 latest tag
    let target = pack
        .resolve(&range)
        .map(|(v, _)| v)
        .or_else(|| pack.dist_tags.get("latest").cloned())
        .ok_or_else(|| CommandError::from(format!("{name}: 无可用版本")))?;
    let Some(meta) = pack.versions.get(&target) else {
        return Err(CommandError {
            message: format!("{name}@{target}: 元数据缺失"),
            code: 1,
        });
    };
    match field.as_deref() {
        None => {
            println!("{name}@{target}");
            println!("description: {}", meta_extra_str(meta, "description"));
            if let Some(d) = &meta.deprecated {
                println!("DEPRECATED: {d}");
            }
            println!("dist-tags: {:?}", pack.dist_tags);
        }
        Some("version") => println!("{target}"),
        Some("dist-tags") => println!("{:?}", pack.dist_tags),
        Some(f) => {
            let v = meta
                .extra
                .get(f)
                .map(serde_json::Value::to_string)
                .unwrap_or_else(|| "undefined".to_owned());
            println!("{v}");
        }
    }
    Ok(0)
}

/// 版本元数据的 extra 面字符串读取。
fn meta_extra_str(meta: &registry::VersionMeta, key: &str) -> String {
    meta.extra
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned()
}

/// 项目根定位：从 cwd 逐级上找 package.json；找不到以 cwd 为准。
fn find_project(cwd: &Path) -> Result<PathBuf, CommandError> {
    let mut dir = Some(cwd.to_path_buf());
    while let Some(d) = dir {
        if d.join("package.json").is_file() {
            return Ok(d);
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    Ok(cwd.to_path_buf())
}

fn print_help() {
    println!(
        "aluka-npm — aluka 包管理器（npm 功能复刻）\n\
         \n\
         用法: aluka-npm <命令> [参数]\n\
         \n\
         命令:\n\
         \x20 install [spec...]   安装依赖（spec 形如 name / name@^1.2.3 / @scope/pkg@1）\n\
         \x20 uninstall <name...> 卸载依赖并清理孤儿\n\
         \x20 run <script> [-- args]  执行 package.json 脚本\n\
         \x20 ls                  列出安装树\n\
         \x20 init -y             生成默认 package.json\n\
         \x20 view <name> [字段]  查询 registry 元数据\n\
         \n\
         选项:\n\
         \x20 --registry <url>    指定 registry（默认 https://registry.npmjs.org）\n\
         \x20 --production        跳过 devDependencies\n\
         \x20 --no-save           不回写 package.json\n\
         \x20 --ignore-scripts    跳过生命周期脚本\n\
         \n\
         环境: ALUKA_NPM_REGISTRY / NPM_CONFIG_REGISTRY / ALUKA_NPM_RUNTIME（bin shim 的 JS 运行时）"
    );
}
