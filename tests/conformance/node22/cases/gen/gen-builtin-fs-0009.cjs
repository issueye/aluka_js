// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-fs
try { (async () => console.log(JSON.stringify(await ((() => { const fs = require("fs"); fs.writeFileSync("gc_fs_t6.txt", "a,c\nb,d"); return fs.readFileSync("gc_fs_t6.txt", "utf8").split("\n").length })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
