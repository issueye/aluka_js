// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-fs
try { (async () => console.log(JSON.stringify(await ((() => { const fs = require("fs"); fs.appendFileSync("gc_fs_t7.txt", "a"); fs.appendFileSync("gc_fs_t7.txt", "b"); return fs.readFileSync("gc_fs_t7.txt", "utf8") })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
