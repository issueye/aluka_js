#!/usr/bin/env bash
# 差分测试：逐探针比对 aluka 与 Node 22 的输出
# 用法：bash .work/diff/run_diff.sh
set -u
cd "$(dirname "$0")/../.." || exit 1
ALUKAC=./target/debug/alukac.exe
ALUVM=./target/debug/aluvm.exe
PASS=0
FAIL=0
for f in .work/diff/*.js; do
  name=$(basename "$f" .js)
  "$ALUKAC" "$f" -o ".work/diff/$name.bc" >/dev/null 2>&1
  if [ ! -f ".work/diff/$name.bc" ]; then
    echo "=== $name: ALUKA COMPILE FAIL ==="
    FAIL=$((FAIL+1))
    continue
  fi
  A=$("$ALUVM" ".work/diff/$name.bc" 2>&1)
  N=$(node "$f" 2>&1)
  if [ "$A" == "$N" ]; then
    PASS=$((PASS+1))
  else
    FAIL=$((FAIL+1))
    echo "=== $name: DIFF ==="
    diff <(echo "$N") <(echo "$A") | head -20
  fi
done
echo "---- 差分结果: 一致 $PASS / 不一致 $FAIL ----"
