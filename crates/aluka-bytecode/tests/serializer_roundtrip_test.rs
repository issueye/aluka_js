//! 验证 BytecodeModule::serialize 与 deserialize 的 Round-trip 完整无损性。

use aluka_bytecode::BytecodeModule;
use std::fs;
use std::path::PathBuf;

fn get_golden_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.join("../../tests/golden/corpus")
}

#[test]
fn test_all_33_golden_corpus_roundtrip_serialization() {
    let golden_dir = get_golden_dir();
    let entries = fs::read_dir(&golden_dir).expect("读取 golden 语料目录失败");

    let mut tested_count = 0;
    for entry in entries {
        let entry = entry.expect("读取目录项失败");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("bc") {
            let original_bytes = fs::read(&path).expect("读取 .bc 文件失败");

            // 1. 初次反序列化
            let module1 = BytecodeModule::deserialize(&original_bytes)
                .unwrap_or_else(|e| panic!("初次反序列化 {:?} 失败: {:?}", path.file_name(), e));

            // 2. 重新序列化
            let serialized = module1.serialize();
            assert!(
                !serialized.is_empty(),
                "序列化产物不得为空: {:?}",
                path.file_name()
            );

            // 3. 再次反序列化回读
            let module2 = BytecodeModule::deserialize(&serialized)
                .unwrap_or_else(|e| panic!("回读反序列化 {:?} 失败: {:?}", path.file_name(), e));

            // 4. 断言内存结构完全等价
            assert_eq!(
                module1,
                module2,
                "Round-trip 序列化后模块结构必须完全一致: {:?}",
                path.file_name()
            );

            // 5. 校验再序列化后的模块通过 Verifier 全部安全校验
            module2
                .verify()
                .unwrap_or_else(|e| panic!("再序列化模块校验失败 {:?}: {:?}", path.file_name(), e));

            tested_count += 1;
        }
    }

    assert!(
        tested_count >= 33,
        "必须测试全部至少 33 个黄金语料，实测: {}",
        tested_count
    );
    println!("Round-trip 序列化全量验证通过: 共精确回读并校验了 {tested_count} 个 .bc 模块！");
}
