// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-fs
try { (async () => console.log(JSON.stringify(await ((() => { const fs = require("fs"); fs.mkdirSync("gc_fs_d1"); fs.mkdirSync("gc_fs_d1/sub"); fs.writeFileSync("gc_fs_d1/sub/f.txt", "1"); return fs.readdirSync("gc_fs_d1/sub").join() })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
