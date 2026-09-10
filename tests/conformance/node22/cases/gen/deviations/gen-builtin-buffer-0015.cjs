// conformance 生成用例（gen.mjs 产出，勿手改）：builtin-buffer
try { (async () => console.log(JSON.stringify(await (require("buffer").Buffer.from("abc").readUInt8(0)))))().catch(e => console.log("ERR", e.name)) } catch (e) { console.log("ERR", e.name) }
