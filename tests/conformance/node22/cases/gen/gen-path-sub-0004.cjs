// conformance 生成用例（gen.mjs 产出，勿手改）：path-sub
try { (async () => console.log(JSON.stringify(await (require("path").posix.relative("/a/b", "/a/c")))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
