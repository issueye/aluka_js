// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-fs
try { (async () => console.log(JSON.stringify(await ((() => { const fs = require("fs"); fs.writeFileSync("gc_fs_t8.json", JSON.stringify({ok:1})); return JSON.parse(fs.readFileSync("gc_fs_t8.json", "utf8")).ok })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
