// conformance 生成用例（gen.mjs 产出，勿手改）：misc-console
try { (async () => console.log(JSON.stringify(await ((() => { console.log("num", 1, "str", "s", "bool", false); return "multi" })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
