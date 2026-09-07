#!/usr/bin/env bash
# Node.js 官方测试风格子集回归（开发计划 2.27）。
#
# 用法：ALUKA=<aluka 可执行路径> bash run.sh
# 默认用 go run ./cmd/aluka。
set -u

ALUKA="${ALUKA:-go run ../../../cmd/aluka}"

# 相对路径的 ALUKA 必须在 cd 之前转绝对（与 run-probe.sh 一致）。
case "$ALUKA" in
  /* | [A-Za-z]:/*) ;;
  *)
    if [ -f "$ALUKA" ] || [ -d "$ALUKA" ]; then
      ALUKA="$(cd "$(dirname "$ALUKA")" && pwd)/$(basename "$ALUKA")"
    fi
    ;;
esac

cd "$(dirname "$0")"

pass=0
fail=0
failed_tests=()

for f in *.js; do
  if $ALUKA "$f" >/tmp/aluka_conformance.log 2>&1; then
    pass=$((pass + 1))
    echo "PASS  $f"
  else
    fail=$((fail + 1))
    failed_tests+=("$f")
    echo "FAIL  $f"
    sed 's/^/      /' /tmp/aluka_conformance.log | head -10
  fi
done

total=$((pass + fail))
echo "----------------------------------------"
echo "Result: $pass/$total passed ($fail failed)"

if [ "$fail" -gt 0 ]; then
  echo "Failed tests: ${failed_tests[*]}"
  exit 1
fi
exit 0
