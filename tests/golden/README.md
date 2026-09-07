# Golden 字节码语料库

> 本目录存放用于 aluka_r（aluvm 与 alukac）进行 ISA 跨实现差分测试的黄金语料。
> 语料覆盖度：**106 / 106 全指令覆盖（100%）**。

## 目录结构
- `sources/`：JavaScript / TypeScript 测试源代码。
- `corpus/`：由 Go 前端编译器产出的对应 `.bc` 字节码二进制。

## 重新生成方法
由于 Go 侧字节码磁盘缓存键包含绝对路径与 mtime，因此跨机器必须能够重新生成。
运行以下命令即可重新生成全套语料并验证：

```bash
cd aluka_r/tools
go run harvest_golden.go
```

产物索引与覆盖率报告将自动更新至 `.work/evidence/20260904/golden-index.tsv`。
