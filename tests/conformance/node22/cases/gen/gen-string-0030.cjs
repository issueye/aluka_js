// conformance 生成用例（gen.mjs 产出，勿手改）：string
try { console.log(JSON.stringify("one two".replace(/(\w+) (\w+)/, "$2 $1"))) } catch (e) { console.log("ERR", e.name) }
