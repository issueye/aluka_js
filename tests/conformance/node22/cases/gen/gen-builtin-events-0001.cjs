// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-events
try { (async () => console.log(JSON.stringify(await ((() => { const { EventEmitter } = require("events"); const e = new EventEmitter(); let v = 0; e.on("x", (a) => { v = a }); e.emit("x", 7); return v })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
