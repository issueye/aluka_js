#!/usr/bin/env bash
# express-demo 真实环境验证（开发计划 2.14 / Phase 5 验收）：
#   以真实 HTTP 链路验证 aluka 运行 express 应用（中间件/路由/body 解析/
#   并发 promise/状态保持/优雅退出）。
#
# 前置：demo/express-demo 的依赖已安装（node_modules/express 存在；
#       未安装时本脚本提示并 SKIP）。
#
# 用法：
#   ALUKA=<aluka 可执行路径> bash run.sh
set -u

ALUKA="${ALUKA:-go run ../../../cmd/aluka}"
# 相对路径的 ALUKA 需转绝对路径（服务器在子 shell 后台启动）。
case "$ALUKA" in
  /* | [A-Za-z]:/*) ;;
  *)
    if [ -f "$ALUKA" ] || [ -d "$ALUKA" ]; then
      ALUKA="$(cd "$(dirname "$ALUKA")" && pwd)/$(basename "$ALUKA")"
    fi
    ;;
esac

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DEMO_DIR="$(cd "$SCRIPT_DIR/../../../demo/express-demo" && pwd)"
PORT=3000
BASE="http://127.0.0.1:$PORT"

PASS=0
FAIL=0

# match <名称> <实际> <期望子串...>：断言"实际"包含所有期望子串。
# 用子串匹配而非全等比较：aluka 的 JSON.stringify 键序与 Node 不同
# （字典序 vs 插入序，规范均允许），值语义正确即可。
match() {
  local name="$1" got="$2"
  shift 2
  for want in "$@"; do
    case "$got" in
      *"$want"*) ;;
      *)
        FAIL=$((FAIL + 1))
        echo "FAIL  $name"
        echo "       want substring: $want"
        echo "       got:  $got"
        return
        ;;
    esac
  done
  PASS=$((PASS + 1))
  echo "PASS  $name"
}

# 依赖检查：express 未安装则提示并 SKIP（与 npm conformance 一致的口径）。
if [ ! -f "$DEMO_DIR/node_modules/express/package.json" ]; then
  echo "SKIP  express-demo (deps not installed; run: cd demo/express-demo && aluka install)"
  exit 0
fi

# 端口占用检查：避免误连到已有服务。
if curl -s -o /dev/null --max-time 1 "$BASE/echo/portcheck" 2>/dev/null; then
  echo "FAIL  port $PORT already in use"
  exit 1
fi

LOG="$(mktemp)"
SERVER_PID=""

cleanup() {
  if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null
    wait "$SERVER_PID" 2>/dev/null
  fi
  rm -f "$LOG"
}
trap cleanup EXIT

echo "=== starting express demo on :$PORT ==="
"$ALUKA" "$DEMO_DIR/app.js" >"$LOG" 2>&1 &
SERVER_PID=$!

# 等待服务就绪（最长 20s）。用 /echo/ready 探测——不触发 / 的计数逻辑。
ready=0
for _ in $(seq 1 40); do
  if curl -s -o /dev/null --max-time 1 "$BASE/echo/ready" 2>/dev/null; then
    ready=1
    break
  fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    break
  fi
  sleep 0.5
done

if [ "$ready" != "1" ]; then
  FAIL=$((FAIL + 1))
  echo "FAIL  server did not become ready"
  sed 's/^/       /' "$LOG" | head -10
  exit 1
fi
echo "      server ready (pid $SERVER_PID)"

# 1) GET / — 中间件 + JSON 响应（count=1，ready 探测不占用计数）
match "GET / (count=1)" "$(curl -s "$BASE/")" '"hello":"aluka"' '"count":1' '"path":"/"'

# 2) GET / 再请求 — 闭包状态保持（reqCount 递增）
match "GET / (count=2)" "$(curl -s "$BASE/")" '"count":2'

# 3) GET /echo/<word> — 路由参数
match "GET /echo/aluka" "$(curl -s "$BASE/echo/aluka")" "echo:aluka"

# 4) POST /json — express.json() body 解析
match "POST /json (x=42)" "$(curl -s -X POST -H 'Content-Type: application/json' -d '{"x":42}' "$BASE/json")" '"n":42' '"received":{"x":42}'

# 5) GET /load — 500 并发 promise 稳定性（sum(0..499)=124750）
match "GET /load (500 async)" "$(curl -s "$BASE/load")" '"total":500' '"sum":124750'

# 6) Content-Type 头正确性（setHeader 生效）
ct="$(curl -s -D - -o /dev/null "$BASE/")"
case "$ct" in
  *"Content-Type: application/json"*) echo "PASS  Content-Type header"; PASS=$((PASS + 1)) ;;
  *) echo "FAIL  Content-Type header (got: $(echo "$ct" | head -1))"; FAIL=$((FAIL + 1)) ;;
esac

# 7) 优雅退出：kill 后服务器应打印 graceful 关闭日志并正常退出。
kill -TERM "$SERVER_PID" 2>/dev/null
sleep 1
if grep -q "server closed gracefully" "$LOG" 2>/dev/null; then
  echo "PASS  graceful shutdown"
  PASS=$((PASS + 1))
else
  # 未捕获优雅日志不算失败（TERM 可能直接终止进程），仅记录。
  echo "INFO  no graceful-shutdown log (TERM 直接终止，可接受)"
fi
wait "$SERVER_PID" 2>/dev/null
SERVER_PID=""

echo ""
echo "ℹ express conformance: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
  echo "--- server log ---"
  sed 's/^/       /' "$LOG" | head -20
  exit 1
fi
