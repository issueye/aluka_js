# YYYY-MM-DD · 每日 TODO 模板

> 总 TODO 见 [../README.md](./README.md)；证据规则见其 §0。
> 上一日：[YYYYMMDD](../YYYYMMDD/README.md)

**当前里程碑**：M1 / M2 / ...　|　**权威 Oracle**：Node.js 22 LTS (v22.23.1+)

---

## 1. 今日目标（可判定完成态）

1. [目标 1：简明清晰、具备判定标准的目标描述]
2. [目标 2：明确预期的输出与行为]

---

## 2. 待办清单（开工先登记）

| # | 待办任务项 | 状态 | 关联总 TODO 编号 |
|---|---|:---:|:---:|
| 1 | [待办项简述] | `[ ]` | M1.x |
| 2 | 门禁验证（fmt / clippy -D warnings / cargo test 全绿） | `[ ]` | 门禁 |
| 3 | 真实证据回填与 diff 复审 | `[ ]` | 证据闭环 |

---

## 3. 达成目标证据（真实证据闭环）

### 待办 1 · [任务名称]

**结论**：达成 / 未达成（卡点记录）

**证据类型**：命令证据 / 产物证据 / 提交证据

```bash
# 粘贴关键命令与精简客观输出
$ cargo test -p ...
test result: ok. X passed; 0 failed; ...
```

---

## 4. 自动化门禁结果（全绿才可交付）

```bash
# 1. 格式化门禁（退出码 0）
cargo fmt --all --check

# 2. 严格 Clippy 门禁（零警告允许）
cargo clippy --all-targets --all-features -- -D warnings

# 3. 全工作区测试套件（100% 通过）
cargo test --workspace --all-features
```

---

## 5. 复审结论与偏差记录

- **`git diff` 复审**：确认所有修改与今日目标严格一致，无无关夹带代码；
- **偏差与卡点**：如遇到与 Node.js 22 LTS 规范分歧，记录在案待后续里程碑解决。
