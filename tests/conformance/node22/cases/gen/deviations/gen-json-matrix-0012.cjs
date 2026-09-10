// conformance 生成用例（gen.mjs 产出，勿手改）：json-matrix
try { console.log(JSON.stringify(JSON.parse(JSON.stringify({a:1}), (k, v) => (k === "a" ? v * 2 : v)))) } catch (e) { console.log("ERR", e.name) }
