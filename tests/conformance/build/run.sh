#!/usr/bin/env bash
# build --compile conformance（docs/build-compile-plan.md）。
#
# M1：单入口产物可执行 + 普通模式回退。
# M2：多文件 + node_modules 静态依赖 + 循环依赖 + 动态 import + 未嵌入报错。
#
# 用法：
#   ALUKA=<aluka 可执行路径> bash run.sh
set -u

ALUKA="${ALUKA:-go run ../../../cmd/aluka}"
# 相对路径的 ALUKA 需转绝对路径（子 shell 中 cd 到临时目录执行产物）。
case "$ALUKA" in
  /* | [A-Za-z]:/*) ;;
  *)
    if [ -f "$ALUKA" ] || [ -d "$ALUKA" ]; then
      ALUKA="$(cd "$(dirname "$ALUKA")" && pwd)/$(basename "$ALUKA")"
    fi
    ;;
esac

PASS=0
FAIL=0

check() { # check <名称> <期望> <实际>
  local name="$1" want="$2" got="$3"
  if [ "$got" = "$want" ]; then
    PASS=$((PASS + 1))
    echo "PASS  $name"
  else
    FAIL=$((FAIL + 1))
    echo "FAIL  $name"
    echo "       want: $want"
    echo "       got:  $got"
  fi
}

DIR="$(mktemp -d)"
trap 'rm -rf "$DIR"' EXIT

# 1) CJS 入口：无 import/export 的 .ts（console.log 直接输出）。
cat > "$DIR/hello.ts" <<'EOF'
console.log('hello from compiled aluka');
EOF
if ! $ALUKA build --compile --outfile "$DIR/hello.exe" "$DIR/hello.ts" >"$DIR/build1.log" 2>&1; then
  FAIL=$((FAIL + 1))
  echo "FAIL  build cjs entry"
  sed 's/^/       /' "$DIR/build1.log" | head -5
else
  check "compile cjs entry runs" "hello from compiled aluka" "$("$DIR/hello.exe" 2>&1)"
fi

# 2) ESM 入口：含 export 的 .ts。
cat > "$DIR/esm.ts" <<'EOF'
export const x = 42;
console.log('esm x = ' + x);
EOF
if ! $ALUKA build --compile --outfile "$DIR/esm.exe" "$DIR/esm.ts" >"$DIR/build2.log" 2>&1; then
  FAIL=$((FAIL + 1))
  echo "FAIL  build esm entry"
  sed 's/^/       /' "$DIR/build2.log" | head -5
else
  check "compile esm entry runs" "esm x = 42" "$("$DIR/esm.exe" 2>&1)"
fi

# 3) 普通 aluka（无 payload）行为不变。
check "normal mode unaffected" "2" "$($ALUKA -e 'console.log(1+1)' 2>&1)"

# 4) 尾部追加非 footer 垃圾 → 回退普通模式（不误判为产物）。
cp "$ALUKA" "$DIR/junk.exe"
printf 'JUNKJUNKJUNKJUNK' >> "$DIR/junk.exe"
check "corrupt footer falls back" "2" "$("$DIR/junk.exe" -e 'console.log(1+1)' 2>&1)"

# 5) M2 多文件 + node_modules 静态依赖（ESM 导入 + CJS require 混合）。
mkdir -p "$DIR/multi/node_modules/smallpkg"
cat > "$DIR/multi/main.ts" <<'EOF'
import { greet } from './util.ts';
import { double } from './num.cjs';
const { magic } = require('smallpkg');
console.log(greet('aluka'), double(21), 'magic=' + magic());
EOF
cat > "$DIR/multi/util.ts" <<'EOF'
export function greet(n) { return 'hi ' + n; }
EOF
cat > "$DIR/multi/num.cjs" <<'EOF'
module.exports = { double: (x) => x * 2 };
EOF
cat > "$DIR/multi/node_modules/smallpkg/package.json" <<'EOF'
{ "name": "smallpkg", "version": "1.0.0", "main": "./index.js" }
EOF
cat > "$DIR/multi/node_modules/smallpkg/index.js" <<'EOF'
module.exports = { magic: () => 7 };
EOF
if ! $ALUKA build --compile --outfile "$DIR/multi.exe" "$DIR/multi/main.ts" >"$DIR/build5.log" 2>&1; then
  FAIL=$((FAIL + 1))
  echo "FAIL  build multi-file"
  sed 's/^/       /' "$DIR/build5.log" | head -5
else
  check "multi-file + node_modules" "hi aluka 42 magic=7" "$("$DIR/multi.exe" 2>&1)"
fi

# 6) M2 循环依赖（a↔b）不栈溢出。
mkdir -p "$DIR/cycle"
cat > "$DIR/cycle/a.ts" <<'EOF'
import { b } from './b.ts';
export const a = 'A' + b;
console.log('a = ' + a);
EOF
cat > "$DIR/cycle/b.ts" <<'EOF'
import { a } from './a.ts';
export const b = 'B';
EOF
if ! $ALUKA build --compile --outfile "$DIR/cycle.exe" "$DIR/cycle/a.ts" >"$DIR/build6.log" 2>&1; then
  FAIL=$((FAIL + 1))
  echo "FAIL  build circular deps"
  sed 's/^/       /' "$DIR/build6.log" | head -5
else
  check "circular deps" "a = AB" "$("$DIR/cycle.exe" 2>&1)"
fi

# 7) M2 动态 import 字面量（产物内模块）。
cat > "$DIR/dyn.ts" <<'EOF'
import('./lazy.ts').then(m => console.log('dynamic: ' + m.msg));
EOF
cat > "$DIR/lazy.ts" <<'EOF'
export const msg = 'lazy loaded';
EOF
if ! $ALUKA build --compile --outfile "$DIR/dyn.exe" "$DIR/dyn.ts" >"$DIR/build7.log" 2>&1; then
  FAIL=$((FAIL + 1))
  echo "FAIL  build dynamic import"
  sed 's/^/       /' "$DIR/build7.log" | head -5
else
  check "dynamic import" "dynamic: lazy loaded" "$("$DIR/dyn.exe" 2>&1)"
fi

# 7b) T2-B4 动态 import 变量形式：产物运行期按 manifest.RootDir 回退磁盘
#     现场加载未嵌入的 .ts/.cjs，createRequire(import.meta.url) 相对解析
#     同样可用（docs/jiti-dynamic-import-plan.md M1）。
mkdir -p "$DIR/dynvar"
cat > "$DIR/dynvar/main.ts" <<'EOF'
globalThis.__which = 'a';
import('./mod-' + globalThis.__which + '.cjs').then(async m => {
  console.log('dynvar-cjs: ' + m.add(2, 3));
  globalThis.__which = 'b';
  const m2 = await import('./mod-' + globalThis.__which + '.ts');
  console.log('dynvar-ts: ' + m2.sq(7));
  const { createRequire } = await import('node:module');
  const reqRel = createRequire(import.meta.url);
  console.log('dynvar-rel: ' + reqRel('./data.cjs'));
});
EOF
cat > "$DIR/dynvar/mod-a.cjs" <<'EOF'
module.exports = { add: (a, b) => a + b };
EOF
cat > "$DIR/dynvar/mod-b.ts" <<'EOF'
export const sq = (x: number): number => x * x;
EOF
cat > "$DIR/dynvar/data.cjs" <<'EOF'
module.exports = 'from-disk';
EOF
if ! $ALUKA build --compile --outfile "$DIR/dynvar.exe" "$DIR/dynvar/main.ts" >"$DIR/build7b.log" 2>&1; then
  FAIL=$((FAIL + 1))
  echo "FAIL  build dynamic import variable"
  sed 's/^/       /' "$DIR/build7b.log" | head -5
else
  check "dynamic import variable + createRequire from disk" "dynvar-cjs: 5
dynvar-ts: 49
dynvar-rel: from-disk" "$("$DIR/dynvar.exe" 2>&1)"
fi

# 8) T2-B4 未嵌入模块：动态 import 变量形式（构建期无法静态收集）在运行期
#    按 manifest.RootDir 回退文件系统现场解析；文件不存在时报清晰错误
#    （含候选绝对路径 "cannot resolve ..."）。旧产物（无 RootDir）保持
#    "cannot load external module" 文案；静态 require 不存在的文件在
#    构建期即失败（正确把关）。
cat > "$DIR/external.ts" <<'EOF'
const name = './not-embedded.ts';
import(name).then(() => console.log('unexpected'), (e) => console.log('ERR: ' + e.message));
EOF
if ! $ALUKA build --compile --outfile "$DIR/external.exe" "$DIR/external.ts" >"$DIR/build8.log" 2>&1; then
  FAIL=$((FAIL + 1))
  echo "FAIL  build external-ref entry"
  sed 's/^/       /' "$DIR/build8.log" | head -5
else
  out="$("$DIR/external.exe" 2>&1)"
  case "$out" in
    *"cannot resolve"*|*"cannot load external module"*) echo "PASS  unembedded module errors clearly"; PASS=$((PASS + 1)) ;;
    *) echo "FAIL  unembedded module errors clearly"; echo "       got: $out"; FAIL=$((FAIL + 1)) ;;
  esac
fi

# 9) M3 JSON 资源（import attributes 静态导入）+ argv + import.meta。
mkdir -p "$DIR/sem"
cat > "$DIR/sem/main.ts" <<'EOF'
import cfg from './data.json' with { type: 'json' };
console.log('json:' + JSON.stringify(cfg));
console.log('argv1:' + process.argv[1]);
console.log('argv2:' + process.argv[2]);
console.log('meta:' + import.meta.url);
console.log('dirname:' + JSON.stringify(import.meta.dirname));
EOF
cat > "$DIR/sem/data.json" <<'EOF'
{ "name": "aluka", "count": 3 }
EOF
if ! $ALUKA build --compile --outfile "$DIR/sem.exe" "$DIR/sem/main.ts" >"$DIR/build9.log" 2>&1; then
  FAIL=$((FAIL + 1))
  echo "FAIL  build semantics"
  sed 's/^/       /' "$DIR/build9.log" | head -5
else
  out="$("$DIR/sem.exe" myarg 2>&1)"
  ok=1
  case "$out" in
    *'json:{"name":"aluka","count":3}'*) ;;
    *) echo "       json: $out"; ok=0 ;;
  esac
  case "$out" in
    *"argv1:main.ts"*) ;;
    *) ok=0 ;;
  esac
  case "$out" in
    *"argv2:myarg"*) ;;
    *) ok=0 ;;
  esac
  case "$out" in
    *"meta:bun://~BUN/main.ts"*) ;;
    *) ok=0 ;;
  esac
  if [ "$ok" = "1" ]; then
    echo "PASS  json resource + argv + import.meta"; PASS=$((PASS + 1))
  else
    echo "FAIL  json resource + argv + import.meta"; echo "       got: $out"; FAIL=$((FAIL + 1))
  fi
fi

# 10) M3 错误堆栈显示虚拟路径（不泄露构建机绝对路径）。
cat > "$DIR/err.ts" <<'EOF'
function boom() { throw new Error('kaboom'); }
boom();
EOF
if ! $ALUKA build --compile --outfile "$DIR/err.exe" "$DIR/err.ts" >"$DIR/build10.log" 2>&1; then
  FAIL=$((FAIL + 1))
  echo "FAIL  build error entry"
  sed 's/^/       /' "$DIR/build10.log" | head -5
else
  out="$("$DIR/err.exe" 2>&1)"
  case "$out" in
    *"err.ts"*"kaboom"*)
      if echo "$out" | grep -q "AppData\|/tmp/\|C:"; then
        echo "FAIL  error stack leaks build-machine path"; echo "       got: $out"; FAIL=$((FAIL + 1))
      else
        echo "PASS  error stack uses virtual path"; PASS=$((PASS + 1))
      fi ;;
    *) echo "FAIL  error stack"; echo "       got: $out"; FAIL=$((FAIL + 1)) ;;
  esac
fi

# 11) M3 顶层 await（TLA）入口。
cat > "$DIR/tla.ts" <<'EOF'
const x = await Promise.resolve(42);
console.log('tla:' + x);
EOF
if ! $ALUKA build --compile --outfile "$DIR/tla.exe" "$DIR/tla.ts" >"$DIR/build11.log" 2>&1; then
  FAIL=$((FAIL + 1))
  echo "FAIL  build tla entry"
  sed 's/^/       /' "$DIR/build11.log" | head -5
else
  check "top-level await" "tla:42" "$("$DIR/tla.exe" 2>&1)"
fi

# 12) M3 动态 import JSON（import attributes 动态形式）。
cat > "$DIR/dynjson.ts" <<'EOF'
import('./data.json', { with: { type: 'json' } }).then(m => console.log('dyn:' + m.default.name));
EOF
cp "$DIR/sem/data.json" "$DIR/data.json"
if ! $ALUKA build --compile --outfile "$DIR/dynjson.exe" "$DIR/dynjson.ts" >"$DIR/build12.log" 2>&1; then
  FAIL=$((FAIL + 1))
  echo "FAIL  build dynamic json"
  sed 's/^/       /' "$DIR/build12.log" | head -5
else
  check "dynamic import json" "dyn:aluka" "$("$DIR/dynjson.exe" 2>&1)"
fi

echo ""
# 13) M4 体积与启动基线（INFO 输出，非断言）。
size_bytes=$(stat -c%s "$DIR/hello.exe" 2>/dev/null || stat -f%z "$DIR/hello.exe" 2>/dev/null)
start_ms=$(date +%s%N)
"$DIR/hello.exe" > /dev/null 2>&1
end_ms=$(date +%s%N)
echo "INFO  artifact size: ${size_bytes} bytes; startup: $(( (end_ms - start_ms) / 1000000 )) ms"

echo "ℹ build conformance: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
  exit 1
fi

# === T2（docs/test-bundle-optimize-plan.md §5.2）=============================

# 14) T2-B1 tree-shaking：未使用模块剪除 + 行为一致。
mkdir -p "$DIR/t2"
printf '%s\n' \
'export function used() { return "USED"; }' \
'export function unused() { return "UNUSED"; }' > "$DIR/t2/lib.js"
printf '%s\n' 'export function neverCalled() { return "DEAD"; }' > "$DIR/t2/dead.js"
printf '%s\n' 'globalThis.__sideEffect = (globalThis.__sideEffect || 0) + 1;' > "$DIR/t2/side.js"
printf '%s\n' \
"import { used } from './lib.js';" \
"import { dead } from './dead.js';" \
"import './side.js';" \
"console.log('result:' + used());" \
"console.log('side:' + globalThis.__sideEffect);" > "$DIR/t2/main.js"
no_shake_log="$("$ALUKA" build --compile --outfile "$DIR/t2/no-shake.exe" --no-tree-shake "$DIR/t2/main.js" 2>&1)"
shake_log="$("$ALUKA" build --compile --outfile "$DIR/t2/shake.exe" "$DIR/t2/main.js" 2>&1)"
no_n="$(echo "$no_shake_log" | grep -oE '[0-9]+ modules' | head -1 | cut -d' ' -f1)"
sh_n="$(echo "$shake_log" | grep -oE '[0-9]+ modules' | head -1 | cut -d' ' -f1)"
if [ -n "$no_n" ] && [ -n "$sh_n" ] && [ "$sh_n" -lt "$no_n" ]; then
  echo "PASS  T2-B1 unused module removed ($no_n → $sh_n modules)"; PASS=$((PASS + 1))
else
  echo "FAIL  T2-B1 unused module removed (no-shake=$no_n shake=$sh_n)"; FAIL=$((FAIL + 1))
fi
shake_out="$("$DIR/t2/shake.exe" 2>&1)"
if [ "$shake_out" = "$(printf 'result:USED\nside:1')" ]; then
  echo "PASS  T2-B1 shake output identical"; PASS=$((PASS + 1))
else
  echo "FAIL  T2-B1 shake output identical"; echo "       got: $shake_out"; FAIL=$((FAIL + 1))
fi

# 15) T2-B1 导出级剪枝：CJS require ESM 导出完整保留（require 使用不可静态分析）。
printf '%s\n' \
"const m = require('./lib.js');" \
"console.log('cjs-require:' + m.used());" > "$DIR/t2/cjsmain.js"
cjs_out="$("$ALUKA" build --compile --outfile "$DIR/t2/cjs.exe" "$DIR/t2/cjsmain.js" >/dev/null 2>&1; "$DIR/t2/cjs.exe" 2>&1)"
if [ "$cjs_out" = "cjs-require:USED" ]; then
  echo "PASS  T2-B1 cjs require esm exports kept"; PASS=$((PASS + 1))
else
  echo "FAIL  T2-B1 cjs require esm exports kept"; echo "       got: $cjs_out"; FAIL=$((FAIL + 1))
fi

# 16) T2-B2 minify：行为与未压缩一致（常量折叠/DCE/未用声明删除）。
printf '%s\n' \
'const DEAD = 100;' \
'function deadFn() { return "X"; }' \
'const folded = 1 + 2 * 3;' \
'if (false) { console.log("NEVER"); }' \
'let x = 5;' \
'function compute(a) {' \
'  if (a > 10) { return "big"; } else { return "small"; }' \
'  console.log("unreachable");' \
'}' \
"console.log('minify:' + folded + ':' + compute(x));" > "$DIR/t2/mini.js"
plain_out="$("$ALUKA" build --compile --outfile "$DIR/t2/plain.exe" "$DIR/t2/mini.js" >/dev/null 2>&1; "$DIR/t2/plain.exe" 2>&1)"
mini_out="$("$ALUKA" build --compile --outfile "$DIR/t2/mini.exe" --minify "$DIR/t2/mini.js" >/dev/null 2>&1; "$DIR/t2/mini.exe" 2>&1)"
if [ -n "$mini_out" ] && [ "$mini_out" = "$plain_out" ]; then
  echo "PASS  T2-B2 minify output identical ($mini_out)"; PASS=$((PASS + 1))
else
  echo "FAIL  T2-B2 minify output identical"; echo "       plain: $plain_out"; echo "       mini:  $mini_out"; FAIL=$((FAIL + 1))
fi

# 17) T2-B3 多入口 --outdir。
"$ALUKA" build --compile --outdir "$DIR/t2/dist" "$DIR/t2/main.js" "$DIR/t2/cjsmain.js" >/dev/null 2>&1
d1="$("$DIR/t2/dist/main" 2>&1 | head -1)"
d2="$("$DIR/t2/dist/cjsmain" 2>&1)"
if [ -x "$DIR/t2/dist/main" ] && [ -x "$DIR/t2/dist/cjsmain" ] \
   && [ "$d1" = "result:USED" ] && [ "$d2" = "cjs-require:USED" ]; then
  echo "PASS  T2-B3 outdir multi-entry both run"; PASS=$((PASS + 1))
else
  echo "FAIL  T2-B3 outdir multi-entry both run"; FAIL=$((FAIL + 1))
fi

# 18) T2-B4 动态 import 常量折叠 + 不可解析警告。
printf '%s\n' \
'async function main() {' \
'  try {' \
"    const m = await import('./dyn-lib.js');" \
"    console.log('dyn:' + m.hello());" \
'  } catch (e) { console.log("DYN-ERR:" + e.message); }' \
'}' \
'main();' > "$DIR/t2/dynmain.js"
printf '%s\n' 'export function hello() { return "HELLO-DYN"; }' > "$DIR/t2/dyn-lib.js"
dyn_out="$("$ALUKA" build --compile --outfile "$DIR/t2/dyn.exe" "$DIR/t2/dynmain.js" >/dev/null 2>&1; "$DIR/t2/dyn.exe" 2>&1)"
if [ "$dyn_out" = "dyn:HELLO-DYN" ]; then
  echo "PASS  T2-B4 dynamic import const-fold runs"; PASS=$((PASS + 1))
else
  echo "FAIL  T2-B4 dynamic import const-fold runs"; echo "       got: $dyn_out"; FAIL=$((FAIL + 1))
fi
printf '%s\n' \
'async function main() {' \
"  const which = process.argv[2] || './dyn-lib.js';" \
'  await import(which);' \
'}' \
'main();' > "$DIR/t2/dynbad.js"
bad_log="$("$ALUKA" build --compile --outfile "$DIR/t2/dynbad.exe" "$DIR/t2/dynbad.js" 2>&1)"
if echo "$bad_log" | grep -q "non-constant specifier" && [ -x "$DIR/t2/dynbad.exe" ]; then
  echo "PASS  T2-B4 unresolvable dynamic import warns (non-fatal)"; PASS=$((PASS + 1))
else
  echo "FAIL  T2-B4 unresolvable dynamic import warns"; FAIL=$((FAIL + 1))
fi

# 19) Bytecode optimization：优化前后行为一致，并在报告中暴露阶段收益。
printf '%s\n' \
'1;' \
'function choose(flag) {' \
'  if (flag) { return "yes"; } else { return "no"; }' \
'}' \
'console.log("byteopt:" + choose(true));' > "$DIR/t2/byteopt.js"
plain_byteopt="$("$ALUKA" build --compile --outfile "$DIR/t2/byteopt-plain.exe" "$DIR/t2/byteopt.js" >/dev/null 2>&1; "$DIR/t2/byteopt-plain.exe" 2>&1)"
optimized_byteopt="$("$ALUKA" build --compile --optimize --outfile "$DIR/t2/byteopt-opt.exe" "$DIR/t2/byteopt.js" >/dev/null 2>&1; "$DIR/t2/byteopt-opt.exe" 2>&1)"
if [ "$optimized_byteopt" = "$plain_byteopt" ] && [ "$optimized_byteopt" = "byteopt:yes" ]; then
  echo "PASS  bytecode optimize output identical"; PASS=$((PASS + 1))
else
  echo "FAIL  bytecode optimize output identical"; FAIL=$((FAIL + 1))
fi
"$ALUKA" build --compile --optimize --analyze=json --analyze-out "$DIR/t2/byteopt-report.json" \
  --outfile "$DIR/t2/byteopt-report.exe" "$DIR/t2/byteopt.js" >/dev/null 2>&1
if grep -q '"bytecodeOptimized"' "$DIR/t2/byteopt-report.json" \
   && grep -q '"bytecode"' "$DIR/t2/byteopt-report.json"; then
  echo "PASS  bytecode analysis report"; PASS=$((PASS + 1))
else
  echo "FAIL  bytecode analysis report"; FAIL=$((FAIL + 1))
fi

# 20) analyze-only 不写 outfile。
"$ALUKA" build --compile --analyze-only --analyze=json --analyze-out "$DIR/t2/analyze-only.json" \
  --outfile "$DIR/t2/analyze-only.exe" "$DIR/t2/byteopt.js" >/dev/null 2>&1
if [ -f "$DIR/t2/analyze-only.json" ] && [ ! -f "$DIR/t2/analyze-only.exe" ]; then
  echo "PASS  analyze-only skips artifact"; PASS=$((PASS + 1))
else
  echo "FAIL  analyze-only skips artifact"; FAIL=$((FAIL + 1))
fi

# 21) payload 预算超限返回退出码 2，且不写产物。
set +e
"$ALUKA" build --compile --max-payload=1B --outfile "$DIR/t2/budget.exe" "$DIR/t2/byteopt.js" >/dev/null 2>&1
budget_rc=$?
set -u
if [ "$budget_rc" -eq 2 ] && [ ! -f "$DIR/t2/budget.exe" ]; then
  echo "PASS  payload budget gate"; PASS=$((PASS + 1))
else
  echo "FAIL  payload budget gate (rc=$budget_rc)"; FAIL=$((FAIL + 1))
fi

echo "ℹ build conformance: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
