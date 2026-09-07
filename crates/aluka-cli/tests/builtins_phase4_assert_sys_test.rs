//! Phase 4 内置库 e2e 对拍测试：
//! - `node:assert/strict` 严格断言模块（直调判定、ok、equal/strictEqual 严格模式对齐、notStrictEqual、throws）
//! - `node:sys` 兼容别名模块（format、inspect 等实用工具同义转发）
//!
//! 与 Node.js 22 LTS 规范逐字比对：alukac 前端整图编译 →
//! aluvm 原生驱动执行，输出与 Node.js 22 严格一致。

mod common;

use std::path::PathBuf;

/// 创建用例隔离工作目录。
fn 创建工作目录(名称: &str) -> PathBuf {
    let 目录 = std::env::temp_dir().join(format!("内置库第四阶段_{名称}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&目录);
    std::fs::create_dir_all(&目录).expect("创建测试工作目录失败");
    目录
}

/// 核心测试：`assert/strict` 严格断言与 `sys` 格式化端到端对拍。
#[test]
fn 严格断言与系统兼容模块对拍验证() {
    let 工作目录 = 创建工作目录("断言与兼容");
    std::fs::write(
        工作目录.join("probe.js"),
        concat!(
            "const assert = require(\"assert/strict\");\n",
            "if (typeof assert === \"function\") {\n",
            "  assert(true);\n",
            "}\n",
            "assert.ok(true);\n",
            "assert.equal(1, 1);\n",
            "assert.strictEqual(1, 1);\n",
            "assert.notStrictEqual(1, 2);\n",
            "assert.throws(() => { throw new Error(\"测试异常\"); });\n",
            "console.log(\"断言全部通过\");\n",
            "const sys = require(\"sys\");\n",
            "console.log(sys.format(\"hello %s\", \"world\"));\n",
        ),
    )
    .unwrap();
    let 输出 = common::assert_e2e_matches_go(&工作目录, "probe.js");
    assert_eq!(输出, "断言全部通过\nhello world");
}

/// 严格相等模式下 equal 等同于 strictEqual，且 notStrictEqual 正常运作。
#[test]
fn 严格断言相等性细化对拍验证() {
    let 工作目录 = 创建工作目录("严格相等性");
    std::fs::write(
        工作目录.join("probe.js"),
        concat!(
            "const assert = require(\"assert/strict\");\n",
            "assert.strictEqual(\"abc\", \"abc\");\n",
            "assert.strictEqual(100, 100);\n",
            "assert.equal(\"xyz\", \"xyz\");\n",
            "assert.notStrictEqual(1, 2);\n",
            "assert.notStrictEqual(\"hello\", \"world\");\n",
            "console.log(\"严格断言细化验证通过\");\n",
        ),
    )
    .unwrap();
    let 输出 = common::assert_e2e_matches_go(&工作目录, "probe.js");
    assert_eq!(输出, "严格断言细化验证通过");
}

/// `sys` 兼容别名模块的格式化与类型检查对拍。
#[test]
fn 系统兼容模块格式化对拍验证() {
    let 工作目录 = 创建工作目录("系统格式化");
    std::fs::write(
        工作目录.join("probe.js"),
        concat!(
            "const sys = require(\"sys\");\n",
            "console.log(sys.format(\"格式化: %s | %d\", \"测试数据\", 42));\n",
            "console.log(typeof sys.format, typeof sys.inspect);\n",
        ),
    )
    .unwrap();
    let 输出 = common::assert_e2e_matches_go(&工作目录, "probe.js");
    assert_eq!(输出, "格式化: 测试数据 | 42\nfunction function");
}
