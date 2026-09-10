// conformance 生成用例（gen.mjs 产出，勿手改）：misc-console
try { (async () => console.log(JSON.stringify(await ((() => { console.warn("warn-line"); return "warned" })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
