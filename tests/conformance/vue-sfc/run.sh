#!/usr/bin/env bash
# vue-sfc conformance：aluka vs node 双跑 @vue/compiler-sfc 探针（驱动式修复 gate）。
# 用法：ALUKA=/tmp/aluka bash tests/conformance/vue-sfc/run.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
ALUKA="${ALUKA:-go run $REPO_ROOT/cmd/aluka}"
NODE="${NODE:-node}"
PROBE="$REPO_ROOT/demo/web-bundle-vue-demo/probe.mjs"
PASS=0
FAIL=0
SKIP=0

if ! command -v "$NODE" >/dev/null 2>&1; then
  echo "SKIP  node unavailable"
  echo "vue-sfc conformance: PASS=$PASS FAIL=$FAIL SKIP=$SKIP"
  exit 0
fi
if [ ! -f "$PROBE" ] || [ ! -f "$REPO_ROOT/demo/web-bundle-vue-demo/node_modules/vue/package.json" ]; then
  echo "SKIP  vue fixture missing"
  echo "vue-sfc conformance: PASS=$PASS FAIL=$FAIL SKIP=$SKIP"
  exit 0
fi

# 基线：node 执行同一探针（模块解析基于脚本文件位置，与 cwd 无关）。
NODE_OUT="$("$NODE" "$PROBE" 2>&1)"
if [ "$?" -ne 0 ] || ! grep -q 'COMPILER_SFC_OK' <<<"$NODE_OUT"; then
  echo "SKIP  node baseline failed (environment issue)"
  sed 's/^/       /' <<<"$NODE_OUT" | head -5
  echo "vue-sfc conformance: PASS=$PASS FAIL=$FAIL SKIP=$SKIP"
  exit 0
fi
BASELINE="$(grep '^sfc-probe' <<<"$NODE_OUT")"

# 实测：aluka 执行同一探针。
ALUKA_OUT="$(cd "$REPO_ROOT" && $ALUKA "$PROBE" 2>&1)"
ALUKA_RC=$?

if [ "$ALUKA_RC" -ne 0 ] || ! grep -q 'COMPILER_SFC_OK' <<<"$ALUKA_OUT"; then
  FAIL=$((FAIL + 1))
  echo "FAIL  aluka compiler-sfc probe"
  sed 's/^/       /' <<<"$ALUKA_OUT" | head -12
elif ! grep -qF "$BASELINE" <<<"$ALUKA_OUT"; then
  FAIL=$((FAIL + 1))
  echo "FAIL  aluka probe output drifted from node baseline"
  echo "       node: $BASELINE"
  echo "       aluka: $(grep '^sfc-probe' <<<"$ALUKA_OUT")"
else
  PASS=$((PASS + 1))
  echo "PASS  aluka executes @vue/compiler-sfc ($BASELINE)"
fi

echo "vue-sfc conformance: PASS=$PASS FAIL=$FAIL SKIP=$SKIP"
[ "$FAIL" -eq 0 ]
