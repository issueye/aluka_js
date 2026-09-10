// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-util
try { (async () => console.log(JSON.stringify(await (require("util").types.isPromise(Promise.resolve(1))))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
