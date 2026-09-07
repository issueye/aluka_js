//! Phase 4 内置库 e2e 对拍测试：
//! - `fs/promises`（`node:fs/promises` 与 `fs.promises` 原生 Promise 异步文件接口）
//!
//! 包含 `writeFile`、`readFile`、`readdir`、`stat`、`mkdir`、`rm` 的 async/await 端到端验证，
//! 严格通过 `common::assert_e2e_matches_go` 与 Node.js 22 LTS 标准 逐字 100% 对齐。

mod common;

use std::path::PathBuf;

/// 创建测试隔离临时目录。
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("builtins_phase4_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 验证 `node:fs/promises` 基础读写、目录读取、状态获取与删除流程。
#[test]
fn fs_promises_core_lifecycle_e2e_matches_go() {
    let work = work_dir("fs_promises_core");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const fsp = require(\"node:fs/promises\");\n",
            "const fsSync = require(\"fs\");\n",
            "console.log(typeof fsp.readFile, typeof fsp.writeFile, typeof fsp.readdir, typeof fsp.stat, typeof fsp.mkdir, typeof fsp.rm);\n",
            "console.log(typeof fsSync.promises.readFile, typeof fsSync.promises.writeFile);\n",
            "async function main() {\n",
            "  await fsp.writeFile(\"data.txt\", \"aluka async fs\");\n",
            "  const text = await fsp.readFile(\"data.txt\", \"utf8\");\n",
            "  console.log(\"read text:\", text);\n",
            "  const buf = await fsp.readFile(\"data.txt\");\n",
            "  console.log(\"read buf:\", buf.toString(), buf.length);\n",
            "  const st = await fsp.stat(\"data.txt\");\n",
            "  console.log(\"stat:\", st.isFile(), st.isDirectory(), st.size, typeof st.mtimeMs);\n",
            "  const list = await fsp.readdir(\".\");\n",
            "  let found = false;\n",
            "  for (let i = 0; i < list.length; i++) {\n",
            "    if (list[i] === \"data.txt\") found = true;\n",
            "  }\n",
            "  console.log(\"readdir has file:\", found);\n",
            "  await fsp.rm(\"data.txt\");\n",
            "  console.log(\"cleaned successfully\");\n",
            "}\n",
            "main();\n",
        ),
    )
    .unwrap();

    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        "function function function function function function\nfunction function\nread text: aluka async fs\nread buf: aluka async fs 14\nstat: true false 14 number\nreaddir has file: true\ncleaned successfully"
    );
}

/// 验证 `fs.promises` 递归创建目录、多文件写入排序读取与级联递归删除。
#[test]
fn fs_promises_recursive_and_dir_e2e_matches_go() {
    let work = work_dir("fs_promises_recursive");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const { promises: fsp } = require(\"node:fs\");\n",
            "async function main() {\n",
            "  await fsp.mkdir(\"dir_a/dir_b\", { recursive: true });\n",
            "  await fsp.writeFile(\"dir_a/dir_b/z.txt\", \"content z\");\n",
            "  await fsp.writeFile(\"dir_a/dir_b/a.txt\", \"content a\");\n",
            "  const list = await fsp.readdir(\"dir_a/dir_b\");\n",
            "  console.log(\"sorted list:\", list.join(\",\"));\n",
            "  const stDir = await fsp.stat(\"dir_a/dir_b\");\n",
            "  console.log(\"dir stat:\", stDir.isFile(), stDir.isDirectory());\n",
            "  await fsp.rm(\"dir_a\", { recursive: true });\n",
            "  console.log(\"recursive rm done\");\n",
            "}\n",
            "main();\n",
        ),
    )
    .unwrap();

    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        "sorted list: a.txt,z.txt\ndir stat: false true\nrecursive rm done"
    );
}
