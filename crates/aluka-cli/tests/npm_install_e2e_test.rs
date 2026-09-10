//! npm 复刻 e2e：真实 registry 安装 → 依赖树/lockfile 校验 → VM 执行 →
//! 与 node 差分对拍。
//!
//! 语料选 `is-odd`（依赖树小：is-odd → is-number，安装秒级）。
//! registry 不可达（离线 CI）时整个测试跳过——与 conformance 的 node
//! 探测口径一致。

use std::process::Command;

fn aluka_bin() -> &'static str {
    env!("CARGO_BIN_EXE_aluka")
}

fn registry_reachable() -> bool {
    // 3s 内能拿到 registry 响应（含 4xx/5xx 也算可达——网络通）
    // 载荷刻意不使用箭头函数：`=>` 里的 `>` 在部分环境（Windows 下经 shell/shim
    // 启动 node）会被当成重定向，导致载荷被截断（node 报 SyntaxError → 本函数恒
    // false、e2e 长期假绿）并在 CWD 生成垃圾文件 `process.exit(1))`。
    Command::new("node")
        .args(["-e", "fetch('https://registry.npmjs.org/-/ping').then(function(){process.exit(0)}).catch(function(){process.exit(1)})"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn npm_install_and_vm_run_matches_node() {
    if !registry_reachable() {
        eprintln!("SKIP: registry 不可达（离线环境）");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("aluka_npm_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("创建临时项目目录");
    let aluka = aluka_bin();

    // 1. init -y 生成 package.json
    let init = Command::new(aluka)
        .args(["npm", "init", "-y"])
        .current_dir(&tmp)
        .output()
        .expect("aluka npm init 可执行");
    assert!(
        init.status.success(),
        "init 失败: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    assert!(tmp.join("package.json").is_file(), "package.json 应生成");

    // 2. 安装 is-odd（固定范围，防漂移）
    let install = Command::new(aluka)
        .args(["npm", "install", "is-odd@^3.0.1", "--no-audit"])
        .current_dir(&tmp)
        .output()
        .expect("aluka npm install 可执行");
    assert!(
        install.status.success(),
        "install 失败: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    // 3. 安装产物校验：目录 + lockfile v3 + integrity 登记
    let pkg_json = tmp.join("node_modules/is-odd/package.json");
    assert!(pkg_json.is_file(), "is-odd 应落盘 node_modules");
    let dep_json = tmp.join("node_modules/is-number/package.json");
    assert!(dep_json.is_file(), "传递依赖 is-number 应提升安装");
    let lock_text = std::fs::read_to_string(tmp.join("package-lock.json")).expect("lockfile 存在");
    let lock: serde_json::Value = serde_json::from_str(&lock_text).expect("lockfile 可解析");
    assert_eq!(lock["lockfileVersion"], 3, "lockfile 应为 v3 口径");
    assert!(
        lock["packages"]["node_modules/is-odd"]["integrity"]
            .as_str()
            .unwrap_or_default()
            .starts_with("sha512-"),
        "lockfile 应登记 sha512 integrity"
    );

    // 4. 幂等重装：up to date（不重复下载）
    let again = Command::new(aluka)
        .args(["npm", "install"])
        .current_dir(&tmp)
        .output()
        .expect("幂等 install 可执行");
    assert!(again.status.success());
    assert!(
        String::from_utf8_lossy(&again.stdout).contains("up to date"),
        "重装应判定 up to date: {}",
        String::from_utf8_lossy(&again.stdout)
    );

    // 5. VM 执行：aluka run 自动构建镜像 → require('is-odd') 真实依赖链
    let app = tmp.join("app.js");
    std::fs::write(
        &app,
        "const isOdd = require('is-odd');\nconsole.log(isOdd(3), isOdd(4));\n",
    )
    .expect("写入 app 失败");
    let run = Command::new(aluka)
        .args(["run", "app.js"])
        .current_dir(&tmp)
        .output()
        .expect("aluka run 可执行");
    assert!(
        run.status.success(),
        "aluka run 失败: {} / {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let vm_out = String::from_utf8_lossy(&run.stdout)
        .trim()
        .replace("\r\n", "\n");

    // 6. node 差分对拍
    let node_out = Command::new("node")
        .arg("app.js")
        .current_dir(&tmp)
        .output()
        .expect("node 可执行");
    assert!(node_out.status.success(), "node 侧自身失败");
    let expect = String::from_utf8_lossy(&node_out.stdout)
        .trim()
        .replace("\r\n", "\n");
    assert_eq!(vm_out, expect, "VM 输出应与 node 逐字一致");
    assert_eq!(vm_out, "true false");

    // 7. npm ls 树形态
    let ls = Command::new(aluka)
        .args(["npm", "ls"])
        .current_dir(&tmp)
        .output()
        .expect("aluka npm ls 可执行");
    let ls_text = String::from_utf8_lossy(&ls.stdout);
    assert!(ls_text.contains("is-odd@"), "ls 应含 is-odd: {ls_text}");

    let _ = std::fs::remove_dir_all(&tmp);
}
