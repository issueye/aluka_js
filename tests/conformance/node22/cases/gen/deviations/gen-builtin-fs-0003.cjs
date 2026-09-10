// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-fs
try { (async () => console.log(JSON.stringify(await ((() => { const fs = require("fs"); fs.writeFileSync("gc_fs_t3.txt", "x"); fs.unlinkSync("gc_fs_t3.txt"); return fs.existsSync("gc_fs_t3.txt") })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
