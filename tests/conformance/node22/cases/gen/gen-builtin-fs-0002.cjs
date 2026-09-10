// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-fs
try { (async () => console.log(JSON.stringify(await ((() => { const fs = require("fs"); fs.writeFileSync("gc_fs_t2.txt", "ab"); return fs.readFileSync("gc_fs_t2.txt").length })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
