// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-timers
try { (async () => console.log(JSON.stringify(await ((() => { return new Promise(r => { const id = setTimeout(() => r("no"), 50); clearTimeout(id); setTimeout(() => r("cleared-ok"), 10) }) })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
