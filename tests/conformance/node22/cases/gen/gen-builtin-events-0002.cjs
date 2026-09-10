// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-events
try { (async () => console.log(JSON.stringify(await ((() => { const { EventEmitter } = require("events"); const e = new EventEmitter(); let n = 0; e.once("x", () => n++); e.emit("x"); e.emit("x"); return n })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
