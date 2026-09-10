#!/usr/bin/env python3
"""生成语料分区器：借用 Rust conformance runner（std::process 路径处理可靠）
做差分，将分歧用例移入 deviations/ 并生成 DEVIATIONS.md 登记清单。

用法（在仓库根目录）：
    python tests/conformance/node22/cases/gen/partition.py
"""
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

GEN_DIR = Path(__file__).resolve().parent
DEV_DIR = GEN_DIR / "deviations"


def run_diff(case_path: Path) -> tuple[str, str]:
    """直接以 python subprocess 差分单用例（node + alukac/aluvm）。"""
    import tempfile

    repo = GEN_DIR.parents[4]
    bins = repo / "target" / "debug"
    alukac = bins / "alukac.exe"
    aluvm = bins / "aluvm.exe"
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        shutil.copy(case_path, tmp_path / case_path.name)
        node = subprocess.run(
            ["node", case_path.name], cwd=tmp_path, capture_output=True,
            text=True, encoding="utf-8", errors="replace", timeout=30,
        )
        bc = tmp_path / f"{case_path.name}.bc"
        comp = subprocess.run(
            [str(alukac), "compile", str(tmp_path / case_path.name), "-o", str(bc)],
            capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=30,
        )
        if comp.returncode != 0:
            vm_out = f"<compile fail> {comp.stderr.strip()[:200]}"
        else:
            vm = subprocess.run(
                [str(aluvm), "run", str(bc)], cwd=tmp_path, capture_output=True,
                text=True, encoding="utf-8", errors="replace", timeout=30,
            )
            vm_out = vm.stdout.strip() or f"<rc={vm.returncode}> {vm.stderr.strip()[:200]}"
    node_out = node.stdout.strip() or f"<rc={node.returncode}> {node.stderr.strip()[:200]}"
    return node_out, vm_out


def main() -> int:
    env = dict(os.environ, ALUKA_CONF_FILTER="gen-")
    proc = subprocess.run(
        [
            "cargo", "test", "-p", "aluka-cli", "--features", "runtime",
            "--test", "conformance_node22_test", "--", "--nocapture",
        ],
        cwd=Path(__file__).resolve().parents[4],  # E:\...\aluka_wt_npm_m6m7
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    out = proc.stdout + proc.stderr

    passed = set(re.findall(r"^PASS gen/(\S+\.cjs)$", out, re.M))
    invalid = set(re.findall(r"^INV\s+(\S+\.cjs)", out, re.M))

    # 失败块：'gen/xxx.cjs: 描述\n  node: ...\n  vm  : ...'
    failures: dict[str, str] = {}
    for m in re.finditer(r"^gen/(\S+\.cjs): (.+?)$\n((?:  .*\n)*)", out, re.M):
        name, desc, detail = m.group(1), m.group(2), m.group(3)
        failures[name] = f"{desc}\n{detail.rstrip()}"

    total = len(passed) + len(invalid) + len(failures)
    if total == 0:
        print("未解析到任何用例结果（runner 输出格式变化？）", file=sys.stderr)
        print(out[-3000:], file=sys.stderr)
        return 1

    # 分歧用例移入 deviations/
    DEV_DIR.mkdir(exist_ok=True)
    for name in sorted(failures) + sorted(invalid):
        src = GEN_DIR / name
        if src.is_file():
            shutil.move(str(src), str(DEV_DIR / name))

    # 历史偏差明细采集（python subprocess 直接驱动三进程；逐条记录 node/vm 输出）
    dev_details: dict[str, str] = {}
    if DEV_DIR.is_dir():
        for dev in sorted(DEV_DIR.glob("gen-*.cjs")):
            node_out, vm_out = run_diff(dev)
            dev_details[dev.name] = "node: " + node_out + chr(10) + "vm  : " + vm_out

    # DEVIATIONS.md 登记（本次失败 + 历史偏差全集）
    lines = [
        "# 生成语料偏差清单（partition.py 自动产出；修复后重跑即自动回归 gen/）",
        "",
        f"> 语料: {len(passed)} 通过 / {len(dev_details) + len(failures)} 偏差 / "
        f"{len(invalid)} 无效对比（本轮扫描 {total}）",
        "",
    ]
    all_failures = {**dev_details, **failures}
    for name in sorted(all_failures):
        lines.append(f"## {name}")
        lines.append("```")
        for ln in all_failures[name].splitlines():
            lines.append(ln)
        lines.append("```")
        lines.append("")
    if invalid:
        lines.append("## 无效对比（node 侧自身失败，不计入语料）")
        lines.append("```")
        for name in sorted(invalid):
            lines.append(name)
        lines.append("```")
        lines.append("")
    (GEN_DIR / "DEVIATIONS.md").write_text("\n".join(lines), encoding="utf-8")

    print(f"partition: {len(passed)} pass, {len(failures)} deviation, {len(invalid)} invalid")
    for name in sorted(failures):
        print(f"  DEV {name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
