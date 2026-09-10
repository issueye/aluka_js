// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-fs
try { (async () => console.log(JSON.stringify(await ((() => { const fs = require("fs"); try { fs.readFileSync("no_such_file_gc.txt") } catch (e) { return e.code } })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
