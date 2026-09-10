// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-url
try { (async () => console.log(JSON.stringify(await (new URL("https://x.io/a?b=1&c=2").searchParams.get("c")))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
