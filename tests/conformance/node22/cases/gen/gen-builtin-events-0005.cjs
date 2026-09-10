// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-events
try { (async () => console.log(JSON.stringify(await ((() => { const { EventEmitter } = require("events"); const e = new EventEmitter(); return e.emit("nothing") })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
