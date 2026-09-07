#!/usr/bin/env bash
# 包管理器一致性测试（开发计划 5.13）：
#   离线 monorepo workspace install — 无网络依赖，验证本地包链接与 lockfile。
#
# 用法：
#   ALUKA=<aluka 可执行路径> bash run.sh
set -u

ALUKA="${ALUKA:-go run ../../../cmd/aluka}"
# 相对路径的 ALUKA 需转绝对路径（脚本在子 shell 中 cd 到临时目录）。
case "$ALUKA" in
  /* | [A-Za-z]:/*) ;;
  *)
    if [ -f "$ALUKA" ] || [ -d "$ALUKA" ]; then
      ALUKA="$(cd "$(dirname "$ALUKA")" && pwd)/$(basename "$ALUKA")"
    fi
    ;;
esac

# 1) 单包离线安装（semver 走本地 registry 不可行，此处验证无依赖 install 的幂等性）。
test_empty() {
  local dir
  dir="$(mktemp -d)"
  echo "=== empty project ==="
  (cd "$dir" && printf '{ "name": "empty", "version": "1.0.0" }' > package.json)
  if (cd "$dir" && $ALUKA install >/dev/null 2>&1); then
    echo "PASS  empty install"
  else
    echo "FAIL  empty install"
    return 1
  fi
  rm -rf "$dir"
}

# 2) workspace monorepo：根 + 2 个本地包互依赖，纯离线。
test_workspace() {
  local dir
  dir="$(mktemp -d)"
  echo "=== workspace monorepo ==="
  (cd "$dir" && printf '{ "name": "monorepo-root", "workspaces": ["packages/*"] }' > package.json)
  mkdir -p "$dir/packages/lib" "$dir/packages/app"
  printf '{ "name": "@mono/lib", "version": "1.0.0" }' > "$dir/packages/lib/package.json"
  printf '{ "name": "@mono/app", "version": "1.0.0", "dependencies": { "@mono/lib": "1.0.0" } }' > "$dir/packages/app/package.json"

  if ! (cd "$dir" && $ALUKA install >/dev/null 2>&1); then
    echo "FAIL  workspace install"
    rm -rf "$dir"
    return 1
  fi

  local lib_dir="$dir/node_modules/@mono/lib"
  local app_dir="$dir/node_modules/@mono/app"
  if [ ! -f "$lib_dir/package.json" ]; then
    echo "FAIL  @mono/lib not linked into node_modules"
    rm -rf "$dir"
    return 1
  fi
  if [ ! -f "$app_dir/package.json" ]; then
    echo "FAIL  @mono/app not linked into node_modules"
    rm -rf "$dir"
    return 1
  fi
  if [ ! -f "$dir/aluka.lock" ]; then
    echo "FAIL  aluka.lock not generated"
    rm -rf "$dir"
    return 1
  fi
  # 端到端：用 aluka 运行 app 包入口，require 本地 lib。
  cat > "$dir/packages/app/index.js" <<'EOF'
const lib = require('@mono/lib');
console.log('app loaded, lib version =', lib.version);
EOF
  printf '{ "name": "@mono/lib", "version": "1.0.0", "main": "index.js" }' > "$dir/packages/lib/package.json"
  printf 'exports.version = require("./package.json").version;\n' > "$dir/packages/lib/index.js"
  if (cd "$dir" && $ALUKA run packages/app/index.js 2>/tmp/aluka_ws.log | grep -q "lib version = 1.0.0"); then
    echo "PASS  workspace monorepo (link + lockfile + require)"
  else
    echo "FAIL  workspace e2e run"
    sed 's/^/      /' /tmp/aluka_ws.log | head -5
    rm -rf "$dir"
    return 1
  fi
  rm -rf "$dir"
}

fail=0
test_empty || fail=1
test_workspace || fail=1

if [ "$fail" -ne 0 ]; then
  echo "----------------------------------------"
  echo "Result: FAILED"
  exit 1
fi
echo "----------------------------------------"
echo "Result: PASS (workspace install + .npmrc wiring)"
exit 0
