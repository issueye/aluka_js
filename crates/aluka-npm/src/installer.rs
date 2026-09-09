//! 依赖树解析与 node_modules 布局落地（npm 安装语义的工程化实现）。
//!
//! # 解析算法（npm 扁平化/arborist 的确定性子集）
//!
//! BFS 逐层解析：项目直接依赖优先声明根槽位（`node_modules/<name>`），
//! 传递依赖在根槽位版本满足其范围时复用（hoist），否则嵌套安装在
//! 依赖方路径下（`node_modules/a/node_modules/b`）。依赖查找沿 Node
//! 解析链（依赖方自身 node_modules → 逐级上溯至根），与运行时 `require`
//! 的模块解析语义严格一致；磁盘上已存在且满足范围的安装直接复用。
//!
//! # 落地面
//!
//! - tarball 下载 + SRI sha512 完整性校验 + 安全解包（[`crate::tarball`]）；
//! - `.bin` shim（Windows `.cmd` + POSIX shell 双形态，运行时经
//!   `ALUKA_NPM_RUNTIME` 或 PATH 上的 `node` 执行）；
//! - 生命周期脚本（preinstall/install/postinstall，`--ignore-scripts` 跳过）；
//! - package.json 依赖回写 + package-lock v3 固化。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use serde_json::json;

use crate::http::HttpClient;
use crate::lockfile::{LockPackage, Lockfile};
use crate::pkg::PackageJson;
use crate::registry::{self, Packument, VersionMeta};
use crate::semver::{Range, Version};
use crate::tarball;

/// 安装选项。
#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// registry 地址（默认官方源）
    pub registry: String,
    /// 仅生产依赖（`--omit=dev` / `--production`）
    pub omit_dev: bool,
    /// 不回写 package.json（`--no-save`）
    pub no_save: bool,
    /// 跳过生命周期脚本（`--ignore-scripts`）
    pub ignore_scripts: bool,
}

impl Default for InstallOptions {
    fn default() -> Self {
        InstallOptions {
            registry: "https://registry.npmjs.org".to_owned(),
            omit_dev: false,
            no_save: false,
            ignore_scripts: false,
        }
    }
}

/// 安装结果统计。
#[derive(Debug, Default)]
pub struct InstallReport {
    /// 新安装包数
    pub added: usize,
    /// 复用（磁盘已满足 / 会话内已放置）次数
    pub reused: usize,
    /// 跳过生命周期脚本的包数
    pub scripts_skipped: usize,
    /// 人类可读的安装清单（`+ name@version` 行）
    pub lines: Vec<String>,
}

/// 会话内已决定的安装落点：相对安装目录 → (包名, 精确版本)。
type Placements = BTreeMap<String, (String, String)>;

/// 解析并安装 `specs` 到 `project`（npm install 语义）。
///
/// `specs` 为空时安装 package.json 已声明的依赖（deps + devDeps 视
/// `opts.omit_dev`）；非空时为 `name[@range]` 列表（安装并回写 dependencies）。
pub fn install(
    project: &Path,
    specs: &[String],
    opts: &InstallOptions,
    client: &HttpClient,
) -> Result<InstallReport, String> {
    let pkg_path = project.join("package.json");
    let mut pkg = if pkg_path.is_file() {
        PackageJson::load(&pkg_path)?
    } else if specs.is_empty() {
        return Err("未找到 package.json，且未指定要安装的包（先 `aluka-npm init -y`）".to_owned());
    } else {
        PackageJson::default()
    };

    // 1. 根需求集：直接依赖（含本次新增 specs）+ 开发依赖
    let mut root_reqs: Vec<(String, Range)> = Vec::new();
    for (name, range_str) in &pkg.dependencies {
        root_reqs.push((name.clone(), Range::parse(range_str)?));
    }
    let mut new_specs: Vec<(String, String)> = Vec::new();
    for spec in specs {
        let (name, range) = registry::parse_spec(spec)?;
        root_reqs.retain(|(n, _)| n != &name);
        new_specs.push((name.clone(), range.to_string()));
        // 新 spec 立即进入根需求集（本次解析队列）
        root_reqs.push((name, range));
    }
    if !opts.omit_dev {
        for (name, range_str) in &pkg.dev_dependencies {
            if root_reqs.iter().any(|(n, _)| n == name) {
                continue;
            }
            root_reqs.push((name.clone(), Range::parse(range_str)?));
        }
    }

    // 2. BFS 解析依赖树（直接依赖先于传递依赖入队 → 根槽位优先权）
    let mut placements: Placements = BTreeMap::new();
    let mut resolved: BTreeMap<String, VersionMeta> = BTreeMap::new();
    let mut report = InstallReport::default();
    // 需求去重：(包名, 规范化范围, 依赖方目录)
    let mut seen: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut queue: VecDeque<(String, Range, String)> = VecDeque::new();
    for (name, range) in &root_reqs {
        queue.push_back((name.clone(), range.clone(), String::new()));
    }
    while let Some((name, range, requirer)) = queue.pop_front() {
        if !seen.insert((name.clone(), range.to_string(), requirer.clone())) {
            continue;
        }
        // 2a. 解析链上已有满足版本（会话内放置或磁盘实况）→ 复用并沿其
        // 依赖闭包继续下钻校验（npm 语义：部分安装残缺树会被补全）
        if let Some(loc) = find_satisfying(project, &placements, &name, &range, &requirer) {
            report.reused += 1;
            let pkg_json = project.join(&loc).join("package.json");
            if pkg_json.is_file() {
                if let Ok(p) = PackageJson::load(&pkg_json) {
                    for (dn, dr) in &p.dependencies {
                        queue.push_back((dn.clone(), Range::parse(dr)?, loc.clone()));
                    }
                }
            }
            continue;
        }
        // 2b. registry 解析（满足范围的最高版本）
        let pack = fetch_cached(client, &opts.registry, &name)?;
        let Some((version, meta)) = pack.resolve(&range) else {
            return Err(format!("找不到满足 {name}@{range} 的版本"));
        };
        // 2c. 落点决策；根槽位版本满足 = 复用，沿其依赖闭包下钻校验
        let Some(rel_dir) = decide_placement(&placements, &name, &range, &requirer) else {
            report.reused += 1;
            let pkg_json = project
                .join("node_modules")
                .join(&name)
                .join("package.json");
            if pkg_json.is_file() {
                if let Ok(p) = PackageJson::load(&pkg_json) {
                    for (dn, dr) in &p.dependencies {
                        queue.push_back((
                            dn.clone(),
                            Range::parse(dr)?,
                            format!("node_modules/{name}"),
                        ));
                    }
                }
            }
            continue;
        };
        placements.insert(rel_dir.clone(), (name.clone(), version.clone()));
        resolved.insert(format!("{name}@{version}"), meta.clone());
        report.added += 1;
        report.lines.push(format!("+ {}@{}", meta.name, version));
        // 2d. 传递依赖入队（依赖方 = 新落点目录）
        for (dn, dr) in &meta.dependencies {
            queue.push_back((dn.clone(), Range::parse(dr)?, rel_dir.clone()));
        }
        // npm v7+ 语义：对等依赖随安装解析（挂根查找）
        for (dn, dr) in &meta.peer_dependencies {
            queue.push_back((dn.clone(), Range::parse(dr)?, String::new()));
        }
    }

    // 3. 落地：下载 → 校验 → 解包 → bin shim → 生命周期脚本
    let mut lock = Lockfile::new_v3();
    lock.name = pkg.name.clone();
    lock.version = pkg.version.clone();
    lock.root = LockPackage {
        name: pkg.name.clone(),
        version: pkg.version.clone(),
        dependencies: (!pkg.dependencies.is_empty()).then(|| pkg.dependencies.clone()),
        dev_dependencies: (!opts.omit_dev && !pkg.dev_dependencies.is_empty())
            .then(|| pkg.dev_dependencies.clone()),
        ..LockPackage::default()
    };
    for (rel_dir, (name, version)) in &placements {
        let Some(meta) = resolved.get(&format!("{name}@{version}")) else {
            continue;
        };
        let dest = project.join(rel_dir);
        install_one(client, meta, &dest, rel_dir, project, opts, &mut report)?;
        lock.record(
            rel_dir,
            LockPackage {
                version: Some(meta.version.clone()),
                resolved: Some(meta.dist.tarball.clone()),
                integrity: meta.dist.integrity.clone(),
                dependencies: (!meta.dependencies.is_empty()).then(|| meta.dependencies.clone()),
                peer_dependencies: (!meta.peer_dependencies.is_empty())
                    .then(|| meta.peer_dependencies.clone()),
                bin: (!meta.bin.is_empty()).then(|| meta.bin.clone()),
                ..LockPackage::default()
            },
        );
    }
    lock.extra.insert("requires".to_owned(), json!(true));

    // 4. 回写 package.json（新增 specs 记入 dependencies）与 lockfile
    if !specs.is_empty() && !opts.no_save {
        for (name, range) in &new_specs {
            pkg.dependencies.insert(name.clone(), range.clone());
            lock.root
                .dependencies
                .get_or_insert_with(BTreeMap::new)
                .insert(name.clone(), range.clone());
        }
        if pkg.name.is_none() {
            pkg.name = project
                .file_name()
                .map(|s| s.to_string_lossy().replace(' ', "-").to_lowercase());
        }
        if pkg.version.is_none() {
            pkg.version = Some("1.0.0".to_owned());
        }
        std::fs::write(&pkg_path, pkg.to_json_string()?)
            .map_err(|e| format!("package.json 写入失败: {e}"))?;
    }
    std::fs::write(project.join("package-lock.json"), lock.to_json_string()?)
        .map_err(|e| format!("package-lock.json 写入失败: {e}"))?;
    Ok(report)
}

/// 安装单个包：下载 tarball → SRI 校验 → 解包 → bin shim → 生命周期脚本。
fn install_one(
    client: &HttpClient,
    meta: &VersionMeta,
    dest: &Path,
    rel_dir: &str,
    project: &Path,
    opts: &InstallOptions,
    report: &mut InstallReport,
) -> Result<(), String> {
    let bytes = client
        .get_bytes(&meta.dist.tarball)
        .map_err(|e| format!("下载 {} 失败: {e}", meta.dist.tarball))?;
    verify_integrity(&bytes, meta.dist.integrity.as_deref(), &meta.name)?;
    tarball::extract_tgz(&bytes, dest).map_err(|e| e.to_string())?;
    write_bin_shims(project, rel_dir, meta)?;
    if opts.ignore_scripts {
        report.scripts_skipped += 1;
        return Ok(());
    }
    for (phase, cmd) in collect_lifecycle(meta) {
        eprintln!("aluka-npm: {} {} `{}`", phase, meta.name, cmd);
        run_lifecycle(project, dest, &cmd)
            .map_err(|e| format!("{phase} 脚本失败（{}@{}）: {e}", meta.name, meta.version))?;
    }
    Ok(())
}

/// SRI 完整性校验（`sha512-<base64>`；缺失时告警放行并登记口径——不引旧
/// sha1 算法，无 integrity 的包仅提示）。
fn verify_integrity(bytes: &[u8], integrity: Option<&str>, name: &str) -> Result<(), String> {
    use base64::Engine as _;
    use sha2::Digest as _;
    let Some(sri) = integrity else {
        eprintln!("aluka-npm 警告: {name} 无 integrity 字段，跳过完整性校验");
        return Ok(());
    };
    let Some((algo, b64)) = sri.split_once('-') else {
        return Err(format!("{name}: integrity 字段形态非法: {sri}"));
    };
    if algo != "sha512" {
        return Err(format!("{name}: 暂不支持的完整性算法 {algo}（仅 sha512）"));
    }
    let expected = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("{name}: integrity base64 解码失败: {e}"))?;
    let digest = sha2::Sha512::digest(bytes);
    if digest.as_slice() != expected.as_slice() {
        return Err(format!("{name}: 完整性校验失败（sha512 不匹配）"));
    }
    Ok(())
}

/// 收集待执行的生命周期脚本（preinstall → install → postinstall）。
fn collect_lifecycle(meta: &VersionMeta) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for phase in ["preinstall", "install", "postinstall"] {
        if let Some(cmd) = meta
            .extra
            .get("scripts")
            .and_then(|s| s.get(phase))
            .and_then(|v| v.as_str())
            .map(str::to_owned)
        {
            out.push((phase, cmd));
        }
    }
    out
}

/// 在包目录执行生命周期脚本（cwd = 包目录；PATH 前置 .bin 链）。
fn run_lifecycle(project: &Path, pkg_dir: &Path, cmd: &str) -> Result<(), String> {
    const SEP: &str = if cfg!(windows) { ";" } else { ":" };
    let path_env = std::env::var("PATH").unwrap_or_default();
    let full_path = format!(
        "{}{SEP}{}{SEP}{path_env}",
        project.join("node_modules/.bin").display(),
        pkg_dir.join("node_modules/.bin").display()
    );
    #[cfg(windows)]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", cmd]);
        c
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", cmd]);
        c
    };
    command.current_dir(pkg_dir).env("PATH", full_path);
    let status = command
        .status()
        .map_err(|e| format!("脚本进程启动失败: {e}"))?;
    if !status.success() {
        return Err(format!("退出码 {}", status.code().unwrap_or(-1)));
    }
    Ok(())
}

/// 为包的 bin 生成 `.bin` shim（落在该包所在 node_modules 的 .bin 下）。
///
/// 双形态：`<cmd>.cmd`（Windows）与无扩展 shell shim（POSIX，Unix 下置
/// 可执行位）。JS 运行时解析：环境变量 `ALUKA_NPM_RUNTIME` 优先，
/// 否则 PATH 上的 `node`。
fn write_bin_shims(project: &Path, rel_dir: &str, meta: &VersionMeta) -> Result<(), String> {
    if meta.bin.is_empty() {
        return Ok(());
    }
    let bin_dir = project
        .join(rel_dir)
        .parent()
        .map(|p| p.join(".bin"))
        .ok_or_else(|| format!("非法安装路径: {rel_dir}"))?;
    std::fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
    let pkg_dir_name = dir_base(rel_dir);
    for (cmd, script) in &meta.bin {
        let script_rel = format!("{pkg_dir_name}/{}", script.trim_start_matches("./"));
        let sh_body = format!(
            "#!/bin/sh\nbasedir=$(dirname \"$0\")\nexec \"${{ALUKA_NPM_RUNTIME:-node}}\" \"$basedir/{script_rel}\" \"$@\"\n"
        );
        let sh_path = bin_dir.join(cmd);
        std::fs::write(&sh_path, sh_body).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&sh_path, std::fs::Permissions::from_mode(0o755));
        }
        let cmd_body = format!(
            "@ECHO OFF\r\nSETLOCAL\r\nSET \"RUNTIME=%ALUKA_NPM_RUNTIME%\"\r\nIF \"%RUNTIME%\"==\"\" SET \"RUNTIME=node\"\r\n\"%RUNTIME%\" \"%~dp0\\{script_rel}\" %*\r\n"
        );
        std::fs::write(bin_dir.join(format!("{cmd}.cmd")), cmd_body).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 读取项目 package.json（供 run/ls 等命令的模型面）。
pub fn read_package(project: &Path) -> Result<PackageJson, String> {
    PackageJson::load(&project.join("package.json"))
}

/// 依据包目录名提取包名（`node_modules/a` → `a`）。
fn dir_base(rel_dir: &str) -> String {
    rel_dir
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(rel_dir)
        .to_owned()
}

/// 依赖链查找：从依赖方目录出发沿 Node 解析链上溯（自身 node_modules →
/// 逐级到根），返回首个满足范围的落点键。
///
/// 查找源：会话内已放置表 + 磁盘实况（`node_modules/<pkg>/package.json`
/// 的 version 字段）——磁盘复用使重复安装免下载。
fn find_satisfying(
    project: &Path,
    placements: &Placements,
    name: &str,
    range: &Range,
    requirer: &str,
) -> Option<String> {
    let mut dir = requirer.to_owned();
    loop {
        let candidate = if dir.is_empty() {
            format!("node_modules/{name}")
        } else {
            format!("{dir}/node_modules/{name}")
        };
        if let Some((_, v)) = placements.get(&candidate) {
            if Version::parse(v).is_ok_and(|ver| range.satisfies(&ver)) {
                return Some(candidate);
            }
        }
        let pkg_json = project.join(&candidate).join("package.json");
        if pkg_json.is_file() {
            if let Ok(p) = PackageJson::load(&pkg_json) {
                if let Some(v) = p.version {
                    if Version::parse(&v).is_ok_and(|ver| range.satisfies(&ver)) {
                        return Some(candidate);
                    }
                }
            }
        }
        if dir.is_empty() {
            return None;
        }
        // 上溯一层：剥离末段 node_modules/<pkg>
        match dir.rfind("node_modules/") {
            Some(pos) => dir = dir[..pos].to_owned(),
            None => dir = String::new(),
        }
    }
}

/// 落点决策：根槽位空闲 → 提升安装；根版本满足范围 → 无新落点（复用）；
/// 根版本不满足 → 直接依赖赢根槽位（BFS 序保证直接依赖先处理），
/// 传递依赖嵌套到依赖方路径下。
fn decide_placement(
    placements: &Placements,
    name: &str,
    range: &Range,
    requirer: &str,
) -> Option<String> {
    let root_key = format!("node_modules/{name}");
    match placements.get(&root_key) {
        None => Some(root_key),
        Some((_, existing)) => {
            let existing_ok = Version::parse(existing).is_ok_and(|v| range.satisfies(&v));
            if existing_ok {
                None
            } else if requirer.is_empty() {
                Some(root_key)
            } else {
                Some(format!("{requirer}/node_modules/{name}"))
            }
        }
    }
}

/// registry packument 进程内缓存（单次 install 会话内同名包只拉一次）。
fn fetch_cached(client: &HttpClient, registry: &str, name: &str) -> Result<Packument, String> {
    use std::cell::RefCell;
    thread_local! {
        static CACHE: RefCell<BTreeMap<String, Packument>> = const { RefCell::new(BTreeMap::new()) };
    }
    CACHE.with(|c| {
        if let Some(p) = c.borrow().get(name) {
            return Ok(p.clone());
        }
        let pack = Packument::fetch(client, registry, name).map_err(|e| e.to_string())?;
        c.borrow_mut().insert(name.to_owned(), pack.clone());
        Ok(pack)
    })
}

/// 卸载包：从 package.json 移除声明 → 重算可达闭包 → 删除孤儿目录 →
/// 重写 lockfile。
pub fn uninstall(project: &Path, names: &[String]) -> Result<(), String> {
    let pkg_path = project.join("package.json");
    let mut pkg = PackageJson::load(&pkg_path)?;
    for name in names {
        pkg.dependencies.remove(name);
        pkg.dev_dependencies.remove(name);
    }
    std::fs::write(&pkg_path, pkg.to_json_string()?)
        .map_err(|e| format!("package.json 写入失败: {e}"))?;

    // 可达闭包：从剩余声明出发沿磁盘 package.json 依赖边遍历
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for n in pkg.dependencies.keys().chain(pkg.dev_dependencies.keys()) {
        queue.push_back(n.clone());
    }
    while let Some(n) = queue.pop_front() {
        let key = format!("node_modules/{n}");
        if !reachable.insert(key) {
            continue;
        }
        let pkg_json = project.join("node_modules").join(&n).join("package.json");
        if pkg_json.is_file() {
            if let Ok(p) = PackageJson::load(&pkg_json) {
                for dn in p.dependencies.keys() {
                    queue.push_back(dn.clone());
                }
            }
        }
    }

    let nm = project.join("node_modules");
    if nm.is_dir() {
        for entry in nm.read_dir().map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let Some(fname) = path.file_name().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            if fname == ".bin" || fname.starts_with('.') {
                continue;
            }
            if fname.starts_with('@') {
                // scoped：下钻一层逐包判定
                if let Ok(rd) = path.read_dir() {
                    for sub in rd.flatten() {
                        let sp = sub.path();
                        if let Some(sname) =
                            sp.file_name().map(|s| s.to_string_lossy().into_owned())
                        {
                            let key = format!("node_modules/{fname}/{sname}");
                            if !reachable.contains(&key) {
                                let _ = std::fs::remove_dir_all(&sp);
                            }
                        }
                    }
                }
                continue;
            }
            if !reachable.contains(&format!("node_modules/{fname}")) {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
    if project.join("node_modules/.bin").is_dir() {
        // shim 以包目录相对路径编码，孤儿包的 shim 随重装覆盖；此处不做
        // 逐 shim 判定（与 npm 重建语义等价的保守口径：保留）
    }
    if lock_path_is_present(project) {
        let mut lf = Lockfile::load(&project.join("package-lock.json"))?;
        lf.packages
            .retain(|k, _| k.is_empty() || reachable.contains(k));
        std::fs::write(project.join("package-lock.json"), lf.to_json_string()?)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn lock_path_is_present(project: &Path) -> bool {
    project.join("package-lock.json").is_file()
}

/// npm ls：打印安装树（lockfile 为准 + 磁盘实况兜底）。
pub fn list(project: &Path) -> Result<String, String> {
    let mut out = String::new();
    let pkg = PackageJson::load(&project.join("package.json")).ok();
    let lock_path = project.join("package-lock.json");
    let lock = if lock_path.is_file() {
        Some(Lockfile::load(&lock_path)?)
    } else {
        None
    };
    let name = pkg
        .as_ref()
        .and_then(|p| p.name.clone())
        .unwrap_or_else(|| "<unnamed>".to_owned());
    let version = pkg
        .as_ref()
        .and_then(|p| p.version.clone())
        .unwrap_or_else(|| "1.0.0".to_owned());
    out.push_str(&format!("{name}@{version}\n"));
    let mut roots: Vec<String> = pkg
        .as_ref()
        .map(|p| p.dependencies.keys().cloned().collect())
        .unwrap_or_default();
    roots.sort();
    let total = roots.len();
    for (i, n) in roots.iter().enumerate() {
        let branch = if i + 1 == total {
            "└── "
        } else {
            "├── "
        };
        render_node(&mut out, project, lock.as_ref(), n, branch, "");
    }
    Ok(out)
}

fn render_node(
    out: &mut String,
    project: &Path,
    lock: Option<&Lockfile>,
    name: &str,
    branch: &str,
    indent: &str,
) {
    let key = format!("node_modules/{name}");
    let ver = lock
        .and_then(|l| l.packages.get(&key))
        .and_then(|p| p.version.clone())
        .or_else(|| {
            PackageJson::load(&project.join(&key).join("package.json"))
                .ok()
                .and_then(|p| p.version)
        })
        .unwrap_or_else(|| "?".to_owned());
    out.push_str(&format!("{indent}{branch}{name}@{ver}\n"));
    // 嵌套子依赖：磁盘实况
    let child_nm = project.join(&key).join("node_modules");
    let mut children: Vec<String> = child_nm
        .read_dir()
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|f| f != ".bin" && f != "package.json")
                .collect()
        })
        .unwrap_or_default();
    children.sort();
    let child_indent = format!(
        "{indent}{}",
        if branch == "└── " {
            "    "
        } else {
            "│   "
        }
    );
    let total = children.len();
    for (i, c) in children.iter().enumerate() {
        let b = if i + 1 == total {
            "└── "
        } else {
            "├── "
        };
        render_node(
            out,
            project,
            lock,
            &format!("{name}/node_modules/{c}"),
            b,
            &child_indent,
        );
    }
}

/// npm init -y：生成默认 package.json（npm 字段全集）。
pub fn init(project: &Path) -> Result<String, String> {
    let path = project.join("package.json");
    if path.is_file() {
        return Err("package.json 已存在".to_owned());
    }
    let dir_name = project
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_owned());
    let name = dir_name.to_lowercase().replace(' ', "-");
    let pkg = json!({
        "name": name,
        "version": "1.0.0",
        "description": "",
        "main": "index.js",
        "scripts": { "test": "echo \"Error: no test specified\" && exit 1" },
        "keywords": [],
        "author": "",
        "license": "ISC"
    });
    let text = serde_json::to_string_pretty(&pkg).map_err(|e| e.to_string())? + "\n";
    std::fs::write(&path, &text).map_err(|e| format!("package.json 写入失败: {e}"))?;
    Ok(text)
}
