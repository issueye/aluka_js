#!/usr/bin/env bash
# npm 包兼容性测试框架（开发计划 3.16/3.17）。
#
# 用法：
#   ALUKA=<aluka 可执行路径> bash run.sh            # 跑全部候选包
#   ALUKA=<aluka 可执行路径> bash run.sh semver     # 只测指定包
#
# 原理：在临时目录 npm install 候选包，逐个用 aluka 加载并执行入口。
# 网络可用时自动安装；网络不可用时跳过（记录 SKIP）。
set -u

ALUKA="${ALUKA:-go run ../../../cmd/aluka}"
NPM="${NPM:-npm}"

# 候选包（纯 JS、无原生依赖，适合运行时加载测试）。
PACKAGES=(
  semver
  ms
  debug
  is-odd
  chalk@4
)

run_one() {
  local pkg="$1"
  local dir
  dir="$(mktemp -d)"
  echo "=== $pkg ==="
  if ! (cd "$dir" && $NPM install --silent "$pkg" >/dev/null 2>&1); then
    echo "SKIP  $pkg (npm install failed)"
    rm -rf "$dir"
    return
  fi
  local main
  # require.resolve 需要裸包名（去掉 @version 后缀，如 chalk@4 → chalk）。
  local bare="${pkg%@*}"
  main="$(cd "$dir" && node -e "try{console.log(require.resolve('$bare'))}catch(e){console.log('')}" 2>/dev/null)"
  if [ -z "$main" ] || [ ! -f "$main" ]; then
    # 回退：尝试常见入口（main 字段可能是目录，如 chalk@4 的 "source"）。
    for cand in "$dir/node_modules/$pkg/index.js" "$dir/node_modules/$pkg/source/index.js" "$dir/node_modules/$pkg/src/index.js" "$dir/node_modules/$pkg/lib/index.js"; do
      if [ -f "$cand" ]; then
        main="$cand"
        break
      fi
    done
  fi
  if [ -f "$main" ]; then
    if $ALUKA "$main" >/tmp/aluka_npm.log 2>&1; then
      echo "PASS  $pkg"
    else
      echo "FAIL  $pkg"
      sed 's/^/      /' /tmp/aluka_npm.log | head -6
    fi
  else
    echo "FAIL  $pkg (no entry found)"
  fi
  rm -rf "$dir"
}

if [ $# -gt 0 ]; then
  run_one "$1"
else
  for pkg in "${PACKAGES[@]}"; do
    run_one "$pkg"
  done
fi
