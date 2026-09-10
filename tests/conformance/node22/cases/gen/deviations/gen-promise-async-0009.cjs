// conformance 生成用例（gen.mjs 产出，勿手改）：promise-async
try { console.log(JSON.stringify(Promise.any([Promise.reject("a"), Promise.resolve("b")]))) } catch (e) { console.log("ERR", e.name) }
