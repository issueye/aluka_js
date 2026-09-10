// conformance 生成用例（gen.mjs 产出，勿手改）：misc-console
try { (async () => console.log(JSON.stringify(await ((() => { console.error("err-line"); return "erred" })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
