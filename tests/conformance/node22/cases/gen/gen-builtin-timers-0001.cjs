// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-timers
try { (async () => console.log(JSON.stringify(await ((() => { return new Promise(r => setTimeout(() => r("t-done"), 5)) })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
