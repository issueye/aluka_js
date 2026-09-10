// conformance 生成用例（gen.mjs 产出，勿手改）：map-set
try { console.log(JSON.stringify(Object.groupBy([1,2,3,4], x=>x%2 ? "odd" : "even").odd.length)) } catch (e) { console.log("ERR", e.name) }
