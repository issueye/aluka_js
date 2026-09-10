// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-timers
try { (async () => console.log(JSON.stringify(await ((() => { let n = 0; const id = setInterval(() => { n++; if (n === 2) { clearInterval(id) } }, 5); return new Promise(r => setTimeout(() => r(n), 40)) })()))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
