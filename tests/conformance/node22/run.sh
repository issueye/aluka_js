#!/usr/bin/env bash
# Node 22 差分测试（docs/node22-compat-plan.md §6）：
# 同一用例在 aluka 与 node22 下运行，stdout 逐行对比。
# 用例约定：输出规范化为 `result: <值>`（或 `FAIL: <错误>`）单行。
#
# 用法：
#   ALUKA=<aluka 可执行路径> NODE=<node 可执行路径> bash run.sh [用例名]
set -u

ALUKA="${ALUKA:-go run ../../../../cmd/aluka}"
NODE="${NODE:-node}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# 相对路径的 ALUKA 转绝对路径（子 shell cd 到用例目录执行）。
case "$ALUKA" in
  /* | [A-Za-z]:/*) ;;
  *)
    if [ -f "$ALUKA" ] || [ -d "$ALUKA" ]; then
      ALUKA="$(cd "$(dirname "$ALUKA")" && pwd)/$(basename "$ALUKA")"
    fi
    ;;
esac

if ! command -v "$NODE" >/dev/null 2>&1; then
  echo "SKIP  node22 conformance (node not found; set NODE=path)"
  exit 0
fi

PASS=0
FAIL=0

# norm_test_out 归一化测试模式输出（#@test 用例）：
#  - 保留自定义观察行（HOOKS> 等，用例自身输出的 `KEY>value` 行）。
#  - 统计行统一为 `tests=N/pass=N/fail=N/cancelled=N/skipped=N/todo=N`。
#  - 丢弃 TAP 明细（duration_ms/type/location 等）与 spec 结果行
#    （Node 的套件汇总名与 aluka 的扁平完整名格式不同，不做行级对比）。
norm_test_out() {
  echo "$1" \
    | grep -E '^#?[[:space:]]*[A-Z]+>' \
    | sed -E 's/^#?[[:space:]]*(HOOKS>)/\1/'
  echo "$1" \
    | grep -E '^# (tests|pass|fail|cancelled|skipped|todo) +|^ℹ (tests|pass|fail|cancelled|skipped|todo) +' \
    | sed -E 's/^# (tests|pass|fail|cancelled|skipped|todo) +([0-9]+)/\1=\2/; s/^ℹ (tests|pass|fail|cancelled|skipped|todo) +([0-9]+)/\1=\2/'
}

run_case() { # run_case <用例文件>
  local case_file="$1"
  local name
  name="$(basename "$case_file")"
  local dir
  dir="$(dirname "$case_file")"
  local a_out n_out
  # 用例内 require 相对模块：cd 到用例目录执行。
  if grep -q '^//@test' "$case_file"; then
    # 测试模式用例：node --test 与 aluka test（输出归一化后对比）。
    a_out="$(cd "$dir" && $ALUKA test "$name" 2>&1)"
    n_out="$(cd "$dir" && "$NODE" --test "$name" 2>&1)"
    a_out="$(norm_test_out "$a_out")"
    n_out="$(norm_test_out "$n_out")"
  else
    a_out="$(cd "$dir" && $ALUKA "$name" 2>&1)"
    n_out="$(cd "$dir" && "$NODE" "$name" 2>&1)"
  fi
  if [ "$a_out" = "$n_out" ]; then
    PASS=$((PASS + 1))
    echo "PASS  $name"
  else
    FAIL=$((FAIL + 1))
    echo "FAIL  $name"
    echo "       aluka: $(echo "$a_out" | head -1)"
    echo "       node22: $(echo "$n_out" | head -1)"
  fi
}

if [ $# -gt 0 ]; then
  run_case "$SCRIPT_DIR/cases/$1"
else
  for case_file in "$SCRIPT_DIR"/cases/*.cjs; do
    run_case "$case_file"
  done
fi

echo ""
echo "ℹ node22 conformance: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
