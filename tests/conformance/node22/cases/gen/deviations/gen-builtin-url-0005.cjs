// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-url
try { (async () => console.log(JSON.stringify(await ((() => { const u = new URL("https://x.io/a"); u.searchParams.set("k", "v v"); return u.search })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
