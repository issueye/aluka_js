#!/usr/bin/env python3
"""M7.2：官方 test262 语料导入器。

从浅克隆的官方 test262 选择目标目录下的用例，按 frontmatter 过滤
（跳过 module/raw/async/worker、含 -- 动态 include 的用例），内联
`includes:` 声明的官方 harness 文件（assert.js/sta.js 等），落库到
tests/conformance/test262/cases/（m72- 前缀防与手写用例冲突）。
"""
import os
import re
import sys
import shutil

SRC = "C:/Users/issue/AppData/Local/Temp/test262/test"
DST = "tests/conformance/test262/cases"
HARNESS = "C:/Users/issue/AppData/Local/Temp/test262/harness"

# 选域：语言核心 + 高覆盖内建（按引擎已有能力优先）
SELECTED_DIRS = [
    "language/types",
    "language/expressions/addition",
    "language/expressions/subtraction",
    "language/expressions/multiplication",
    "language/expressions/division",
    "language/expressions/modulus",
    "language/expressions/comparison",
    "language/expressions/equality",
    "language/expressions/logical-operators",
    "language/expressions/bitwise-operators",
    "language/expressions/conditional-operator",
    "language/expressions/comma-operator",
    "language/expressions/prefix-operator",
    "language/expressions/postfix-operator",
    "language/expressions/unary-operator",
    "language/expressions/void",
    "language/expressions/typeof",
    "language/expressions/grouping",
    "language/expressions/object",
    "language/expressions/array",
    "language/literals",
    "language/asi",
    "language/comments",
    "language/computed-property-names",
    "language/destructuring",
    "language/rest-parameters",
    "language/statementList",
    "language/statements",
    "language/white-space",
    "language/line-terminators",
    "built-ins/Math",
    "built-ins/JSON",
    "built-ins/Number",
    "built-ins/Boolean",
    "built-ins/Infinity",
    "built-ins/NaN",
    "built-ins/undefined",
    "built-ins/Object",
    "built-ins/Array",
    "built-ins/String",
    "built-ins/Symbol",
    "built-ins/Error",
    "built-ins/Function",
    "built-ins/RegExp",
    "built-ins/Date",
    "built-ins/Map",
    "built-ins/Set",
    "built-ins/Promise",
    "built-ins/globalThis",
    "built-ins/eval",
]

# 每目录最多取多少个（保持语料多样性、控制总量）
PER_DIR_CAP = 40

FRONT_RE = re.compile(r"/\*---(.*?)---\*/", re.S)


def parse_front(body_text):
    m = FRONT_RE.search(body_text)
    if not m:
        return None
    return m.group(1)


def front_flags(front):
    m = re.search(r"flags:\s*\[(.*?)\]", front, re.S)
    if not m:
        return []
    return [x.strip() for x in m.group(1).split(",") if x.strip()]


def front_includes(front):
    # includes: [harness/assert.js, harness/sta.js, ...]（可能跨行）
    m = re.search(r"includes:\s*\[(.*?)\]", front, re.S)
    if not m:
        return []
    return [x.strip() for x in m.group(1).split(",") if x.strip()]


def front_negative(front):
    return "negative:" in front


def main():
    total_target = int(sys.argv[1]) if len(sys.argv) > 1 else 1000
    os.makedirs(DST, exist_ok=True)
    # harness 文件缓存
    harness_cache = {}

    def load_harness(rel):
        if rel not in harness_cache:
            with open(os.path.join("C:/Users/issue/AppData/Local/Temp/test262", rel), encoding="utf-8") as f:
                harness_cache[rel] = f.read()
        return harness_cache[rel]

    count = 0
    skipped = {"module": 0, "raw": 0, "async": 0, "worker": 0, "dynamic-includes": 0,
               "canblock": 0, "no-front": 0, "generated": 0}
    per_dir = {}
    names = []

    for d in SELECTED_DIRS:
        base = os.path.join(SRC, d)
        if not os.path.isdir(base):
            continue
        taken = per_dir.get(d, 0)
        for root, _dirs, files in os.walk(base):
            if taken >= PER_DIR_CAP or count >= total_target:
                break
            # 目录内按名字排序保证确定性
            for fn in sorted(files):
                if taken >= PER_DIR_CAP or count >= total_target:
                    break
                if not fn.endswith(".js") or fn.endswith("_FIXTURE.js"):
                    continue
                path = os.path.join(root, fn)
                rel = os.path.relpath(path, SRC)
                with open(path, encoding="utf-8", errors="replace") as f:
                    text = f.read()
                front = parse_front(text)
                if front is None:
                    skipped["no-front"] += 1
                    continue
                flags = front_flags(front)
                if "module" in flags:
                    skipped["module"] += 1
                    continue
                if "raw" in flags:
                    skipped["raw"] += 1
                    continue
                if "async" in flags:
                    skipped["async"] += 1
                    continue
                if "worker" in flags:
                    skipped["worker"] += 1
                    continue
                if any("CanBlock" in fl for fl in flags):
                    skipped["canblock"] += 1
                    continue
                incs = front_includes(front)
                if any("/" not in i or not i.startswith("harness/") for i in incs):
                    skipped["dynamic-includes"] += 1
                    continue
                if "$DONOTEVALUATE" in text and "raw" in flags:
                    skipped["raw"] += 1
                    continue
                if "/generated/" in rel:
                    skipped["generated"] += 1
                    continue

                # 组装输出：frontmatter 原样 + 内联官方 harness + 测试体
                # 老式 ES5.1（Sputnik 移植）用例不声明 includes 但依赖完整
                # harness——统一预载标准集（官方 runner 同口径）
                DEFAULT_HARNESS = [
                    "harness/assert.js",
                    "harness/sta.js",
                    "harness/propertyHelper.js",
                    "harness/compareArray.js",
                    "harness/fnGlobalObject.js",
                    "harness/deepEqual.js",
                    "harness/isConstructor.js",
                    # wellKnownIntrinsicObjects 不预载：其顶层求值
                    # `Object.getPrototypeOf(async function*(){}).prototype`
                    # （引擎 async generator 原型链未支持 -> null -> TypeError）
                    # 仅当用例显式 includes 时内联
                ]
                with open("tests/conformance/test262/harness_compat_shim.js", encoding="utf-8") as sf:
                    compat_shim = sf.read()
                parts = [text]
                extra = (
                    [load_harness(h) for h in DEFAULT_HARNESS]
                    if os.environ.get("M72_PRELOAD") != "0"
                    else []
                )
                extra += [load_harness(inc) for inc in incs]
                if extra:
                    # 内联在 frontmatter 之后（runner 剥 frontmatter 后先
                    # 跑自己的最小 harness，官方 assert.js 再覆盖之）
                    body_start = text.index("---*/") + len("---*/")
                    parts = [text[:body_start], "\n"] + extra + ["\n", text[body_start:]]
                out_text = "".join(parts)

                flat = rel.replace("/", "-").replace("\\", "-")
                out_name = f"m72-{flat}"
                with open(os.path.join(DST, out_name), "w", encoding="utf-8", newline="\n") as f:
                    f.write(out_text)
                names.append(out_name)
                count += 1
                taken += 1
        per_dir[d] = taken
        if count >= total_target:
            break

    print(f"imported: {count}")
    print("skipped:", skipped)
    for d, n in per_dir.items():
        if n:
            print(f"  {d}: {n}")


if __name__ == "__main__":
    main()
