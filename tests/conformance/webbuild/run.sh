#!/usr/bin/env bash
# aluka build --target=web conformance: real React package + TSX component.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
ALUKA="${ALUKA:-go run $REPO_ROOT/cmd/aluka}"
NODE="${NODE:-node}"
REACT_VERSION="18.3.1"
PASS=0
FAIL=0
SKIP=0

check() {
  local name="$1" want="$2" got="$3"
  if [ "$got" = "$want" ]; then
    PASS=$((PASS + 1))
    echo "PASS  $name"
  else
    FAIL=$((FAIL + 1))
    echo "FAIL  $name"
    echo "       want: $want"
    echo "       got: $got"
  fi
}

skip() {
  SKIP=$((SKIP + 1))
  echo "SKIP  $1"
}

if ! command -v npm >/dev/null 2>&1; then
  skip "real React package (npm unavailable)"
  echo "webbuild conformance: PASS=$PASS FAIL=$FAIL SKIP=$SKIP"
  [ "$FAIL" -eq 0 ]
  exit $?
fi
if ! command -v "$NODE" >/dev/null 2>&1; then
  skip "real React bundle execution (node unavailable)"
  echo "webbuild conformance: PASS=$PASS FAIL=$FAIL SKIP=$SKIP"
  [ "$FAIL" -eq 0 ]
  exit $?
fi

DIR="$(mktemp -d)"
trap 'rm -rf "$DIR"' EXIT
mkdir -p "$DIR/app/node_modules"

if ! (cd "$DIR" && npm pack "react@$REACT_VERSION" --ignore-scripts --silent > react.tgz.name 2> npm.log); then
  skip "real React package (npm pack failed)"
  sed 's/^/       /' "$DIR/npm.log" | head -5
  echo "webbuild conformance: PASS=$PASS FAIL=$FAIL SKIP=$SKIP"
  [ "$FAIL" -eq 0 ]
  exit $?
fi
REACT_TGZ="$(tr -d '\r\n' < "$DIR/react.tgz.name")"
if [ ! -f "$DIR/$REACT_TGZ" ]; then
  echo "FAIL  real React package archive"
  echo "       missing: $DIR/$REACT_TGZ"
  FAIL=$((FAIL + 1))
else
  mkdir -p "$DIR/app/node_modules/react"
  tar -xzf "$DIR/$REACT_TGZ" -C "$DIR/app/node_modules/react" --strip-components=1
  check "React package version" "$REACT_VERSION" "$(node -e "console.log(require(process.argv[1]).version)" "$DIR/app/node_modules/react/package.json")"
fi

cat > "$DIR/app/package.json" <<'EOF'
{"type":"module"}
EOF
cat > "$DIR/app/react-entry.tsx" <<'EOF'
import React from 'react';
export const makeElement = () => <button className="ready">React smoke</button>;
export const reactVersion = React.version;
EOF
cat > "$DIR/app/component.tsx" <<'EOF'
import React from 'react';
export function Card({ label }) {
  return <section data-testid="card"><h1>{label}</h1><span>{React.version}</span></section>;
}
export default Card;
EOF
cat > "$DIR/check.mjs" <<'EOF'
import { pathToFileURL } from 'node:url';
const bundle = await import(pathToFileURL(process.argv[2]).href);
const element = bundle.makeElement();
if (element.type !== 'button') throw new Error(`type=${element.type}`);
if (element.props.className !== 'ready') throw new Error(`class=${element.props.className}`);
if (element.props.children !== 'React smoke') throw new Error(`children=${element.props.children}`);
if (typeof bundle.reactVersion !== 'string' || bundle.reactVersion !== '18.3.1') throw new Error(`version=${bundle.reactVersion}`);
console.log('react bundle ok');
EOF

if ! (cd "$REPO_ROOT" && $ALUKA build --target=web --minify --outfile "$DIR/react-entry.js" "$DIR/app/react-entry.tsx" > "$DIR/build.log" 2>&1); then
  echo "FAIL  build real React entry"
  sed 's/^/       /' "$DIR/build.log" | head -12
  FAIL=$((FAIL + 1))
else
  check "React bundle has ESM export" "yes" "$(grep -q 'export ' "$DIR/react-entry.js" && echo yes || echo no)"
  check "React bundle has no __req runtime" "yes" "$(grep -Rql '__req(' "$DIR/assets" "$DIR/react-entry.js" 2>/dev/null && echo no || echo yes)"
  check "React hashed assets exist" "yes" "$(find "$DIR/assets" -name '*.js' -print -quit 2>/dev/null | grep -q . && echo yes || echo no)"
  check "React bundle executes" "react bundle ok" "$(cd "$DIR/app" && "$NODE" "$DIR/check.mjs" "$DIR/react-entry.js" 2>&1)"
fi

if ! (cd "$REPO_ROOT" && $ALUKA build --target=web --minify --outfile "$DIR/component.js" "$DIR/app/component.tsx" > "$DIR/component-build.log" 2>&1); then
  echo "FAIL  build TSX component"
  sed 's/^/       /' "$DIR/component-build.log" | head -12
  FAIL=$((FAIL + 1))
else
  check "TSX bundle has ESM export" "yes" "$(grep -q 'export ' "$DIR/component.js" && echo yes || echo no)"
fi

cat > "$DIR/dynamic-main.ts" <<'EOF'
export async function load() { return (await import('./dynamic-lazy.ts')).value; }
export const sync = 'main-ok';
EOF
cat > "$DIR/dynamic-lazy.ts" <<'EOF'
export const value = 'lazy-ok';
EOF
if ! (cd "$REPO_ROOT" && $ALUKA build --target=web --minify --outfile "$DIR/dynamic-main.js" "$DIR/dynamic-main.ts" > "$DIR/dynamic-build.log" 2>&1); then
  echo "FAIL  build dynamic import"
  sed 's/^/       /' "$DIR/dynamic-build.log" | head -12
  FAIL=$((FAIL + 1))
else
  check "dynamic hashed module exists" "yes" "$(grep -Rql 'lazy-ok' "$DIR/assets" 2>/dev/null && echo yes || echo no)"
  check "dynamic bundle executes" "lazy-ok" "$("$NODE" --input-type=module -e "import('url').then(async ({pathToFileURL})=>{const m=await import(pathToFileURL(process.argv[1])); console.log(await m.load())})" "$DIR/dynamic-main.js" 2>&1)"
fi

cat > "$DIR/format-main.ts" <<'EOF'
export const x = 3;
export default 4;
EOF
if ! (cd "$REPO_ROOT" && $ALUKA build --target=web --format=cjs --outfile "$DIR/fmt.cjs" "$DIR/format-main.ts" > "$DIR/fmt-cjs.log" 2>&1); then
  echo "FAIL  build --format=cjs"
  sed 's/^/       /' "$DIR/fmt-cjs.log" | head -12
  FAIL=$((FAIL + 1))
else
  check "cjs executes in node" "3 4" "$("$NODE" -e "const m=require(process.argv[1]); console.log(m.x, m.default)" "$DIR/fmt.cjs" 2>&1)"
fi
if ! (cd "$REPO_ROOT" && $ALUKA build --target=web --format=umd --global-name=Thing --outfile "$DIR/fmt.umd.js" "$DIR/format-main.ts" > "$DIR/fmt-umd.log" 2>&1); then
  echo "FAIL  build --format=umd"
  sed 's/^/       /' "$DIR/fmt-umd.log" | head -12
  FAIL=$((FAIL + 1))
else
  check "umd cjs branch" "3 4" "$("$NODE" -e "const m=require(process.argv[1]); console.log(m.x, m.default)" "$DIR/fmt.umd.js" 2>&1)"
  check "umd global branch" "3 4" "$("$NODE" -e "const vm=require('vm'); const src=require('fs').readFileSync(process.argv[1],'utf8'); const ctx=vm.createContext({}); vm.runInContext(src,ctx); console.log(ctx.Thing.x, ctx.Thing.default)" "$DIR/fmt.umd.js" 2>&1)"
fi
if (cd "$REPO_ROOT" && $ALUKA build --compile --format=cjs --outfile "$DIR/x.exe" "$DIR/format-main.ts" > "$DIR/fmt-conflict.log" 2>&1); then
  echo "FAIL  --compile --format should conflict"
  FAIL=$((FAIL + 1))
else
  PASS=$((PASS + 1))
  echo "PASS  format requires web target"
fi

if [ -d "$DIR/app/.aluka-cache" ]; then
  echo "FAIL  web build wrote .aluka-cache"
  FAIL=$((FAIL + 1))
else
  PASS=$((PASS + 1))
  echo "PASS  web build avoids bytecode cache"
fi

echo "webbuild conformance: PASS=$PASS FAIL=$FAIL SKIP=$SKIP"
[ "$FAIL" -eq 0 ]
