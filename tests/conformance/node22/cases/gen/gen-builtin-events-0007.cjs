// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-events
try { (async () => console.log(JSON.stringify(await ((() => { const { EventEmitter } = require("events"); const e = new EventEmitter(); let order = []; e.on("e", (v) => order.push(v)); return new Promise(r => { e.emit("e", 1); r(order.join()) }) })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
