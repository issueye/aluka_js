// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-url
try { (async () => console.log(JSON.stringify(await (require("url").format(new URL("https://x.io/a"))))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
