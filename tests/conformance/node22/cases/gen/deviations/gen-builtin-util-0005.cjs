// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-util
try { (async () => console.log(JSON.stringify(await ((() => { const u = require("util"); const fn = u.promisify((cb) => cb(null, "ok")); return fn() instanceof Promise })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
