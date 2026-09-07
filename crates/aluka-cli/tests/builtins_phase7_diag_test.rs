//! Phase 7/8 诊断家族内置库端到端与接口对拍测试：
//! - `diagnostics_channel`（发布/订阅/退订、hasSubscribers、tracingChannel、bindStore/runStores）
//! - `async_hooks`（executionAsyncId/triggerAsyncId、AsyncResource 生命周期、AsyncLocalStorage 往返）
//! - `inspector`（Session 连接状态机与 post 应答、管理 API 面）
//! - `inspector/promises`（Promise 版 Session 与管理 API 面）
//! - `domain`（run 错误传播、enter/exit 栈、bind/intercept、add 错误路由、EventEmitter 面）
//!
//! 与 Node.js 22 LTS 规范逐字比对：alukac 前端整图编译 →
//! aluvm 原生驱动执行，输出与 Node.js 22 严格一致。
//!
//! 探针写法约束（引擎既有能力边界，非本批模块语义）：
//! 不使用 `JSON.stringify` / `Object.keys` / `Array.isArray` / 全局 `String()`
//! / console 打印裸对象字面量 / 顶层未 await 的 `.then` 排空。

mod common;

use std::path::PathBuf;

/// 创建隔离的临时测试目录
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("builtins_phase7_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 诊断通道：发布/订阅/退订/hasSubscribers 与命名通道同一性
#[test]
fn diagnostics_channel_pubsub_matches_go() {
    let work = work_dir("channel_pubsub");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const dc = require(\"node:diagnostics_channel\");\n",
            "console.log(\"hasSub:\", dc.hasSubscribers(\"svc\"));\n",
            "const ch = dc.channel(\"svc\");\n",
            "console.log(\"name:\", ch.name, \"ownHas:\", ch.hasSubscribers);\n",
            "const seen = [];\n",
            "ch.subscribe(function (msg, name) { seen.push(msg + \"@\" + name); });\n",
            "console.log(\"after sub:\", ch.hasSubscribers, dc.hasSubscribers(\"svc\"));\n",
            "ch.publish(\"m1\");\n",
            "ch.publish();\n",
            "console.log(\"seen:\", seen.join(\",\"));\n",
            "const ch2 = dc.channel(\"svc\");\n",
            "console.log(\"same:\", ch2 === ch, \"other:\", dc.channel(\"z\").name);\n",
            "console.log(\"unsub unknown:\", ch.unsubscribe(function () {}));\n",
            "const fn = function () {};\n",
            "ch.subscribe(fn);\n",
            "console.log(\"unsub known:\", ch.unsubscribe(fn));\n",
            "console.log(\"after unsub:\", ch.hasSubscribers);\n",
            "console.log(\"typeof Channel:\", typeof dc.Channel);\n",
            "try { dc.Channel(); } catch (e) { console.log(\"ctor:\", e.message, e.name); }\n",
            "try { ch.subscribe(\"nope\"); } catch (e) { console.log(\"suberr:\", e.message); }\n",
            "try { ch.runStores({ x: 1 }); } catch (e) { console.log(\"rserr:\", e.message); }\n",
            "try { dc.subscribe(\"only-name\"); } catch (e) { console.log(\"moderr:\", e.message); }\n",
            "console.log(\"mod unsub:\", dc.unsubscribe(\"nothing\", function () {}));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("seen: m1@svc,undefined@svc"),
        "订阅回调应收到 (message, name)：{out}"
    );
}

/// 诊断通道：tracingChannel 聚合 + bindStore/runStores 与 AsyncLocalStorage 协作
#[test]
fn diagnostics_channel_tracing_and_stores_matches_go() {
    let work = work_dir("channel_tracing");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const dc = require(\"node:diagnostics_channel\");\n",
            "const ah = require(\"node:async_hooks\");\n",
            "const als = new ah.AsyncLocalStorage();\n",
            "const ch = dc.channel(\"svc\");\n",
            "const log = [];\n",
            "ch.subscribe(function (msg, name) { log.push(msg + \" store:\" + (als.getStore() === undefined ? \"undef\" : typeof als.getStore())); });\n",
            "ch.bindStore(als);\n",
            "ch.publish(\"m1\");\n",
            "const r = ch.runStores({ id: 7 }, function () {\n",
            "    log.push(\"inRun:\" + (typeof als.getStore()));\n",
            "    ch.publish(\"m2\");\n",
            "    return \"ret\";\n",
            "}, \"extra\");\n",
            "log.push(\"ret:\" + r, \"outside:\" + als.getStore());\n",
            "ch.unbindStore(als);\n",
            "ch.publish(\"m3\");\n",
            "console.log(log.join(\" | \"));\n",
            "const tc = dc.tracingChannel(\"db\");\n",
            "console.log(\"tcHas:\", tc.hasSubscribers);\n",
            "const evs = [];\n",
            "tc.subscribe(function (msg) { evs.push(\"t:\" + typeof msg); });\n",
            "console.log(\"tcHas2:\", tc.hasSubscribers);\n",
            "console.log(\"startName:\", tc.start.name, \"identity:\", dc.channel(\"tracing:db:start\") === tc.start);\n",
            "const out = tc.traceSync(function (a) { return a; }, { phase: \"ctx\" }, 1, 2);\n",
            "console.log(\"traceSync:\", out);\n",
            "function boom() { throw new Error(\"tb\"); }\n",
            "try { tc.traceSync(boom, { phase: \"ctx2\" }); } catch (e) { console.log(\"caught:\", e.message); }\n",
            "console.log(\"evs:\", evs.join(\",\"), \"finalHas:\", tc.hasSubscribers);\n",
            "console.log(\"trace fns:\", typeof tc.tracePromise, typeof tc.traceCallback, typeof tc.unsubscribe);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("m1 store:undef"),
        "publish 时订阅者在调用方上下文：{out}"
    );
    assert!(
        out.contains("inRun:object"),
        "runStores 内 ALS store 可见：{out}"
    );
    assert!(
        out.contains("m2 store:object"),
        "runStores 上下文内的发布可见 store：{out}"
    );
}

/// async_hooks：AsyncLocalStorage 往返 + AsyncResource 生命周期钩子
#[test]
fn async_hooks_als_and_resource_matches_go() {
    let work = work_dir("async_hooks");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const ah = require(\"node:async_hooks\");\n",
            "console.log(\"top:\", ah.executionAsyncId(), ah.triggerAsyncId());\n",
            "const store = new ah.AsyncLocalStorage();\n",
            "console.log(\"empty:\", store.getStore());\n",
            "store.run(42, function (a) { console.log(\"run:\", store.getStore(), a); });\n",
            "console.log(\"after run:\", store.getStore());\n",
            "store.enterWith(\"x\");\n",
            "console.log(\"enterWith:\", store.getStore());\n",
            "store.exit(function () { console.log(\"exit:\", store.getStore()); });\n",
            "console.log(\"after exit:\", store.getStore());\n",
            "store.disable();\n",
            "console.log(\"disabled:\", store.getStore());\n",
            "store.run(\"y\", function () { console.log(\"disabledRun:\", store.getStore()); });\n",
            "const a1 = new ah.AsyncLocalStorage();\n",
            "const a2 = new ah.AsyncLocalStorage();\n",
            "a1.run(\"outer\", function () {\n",
            "    a2.run(\"inner\", function () { console.log(\"nest:\", a1.getStore(), a2.getStore()); });\n",
            "    console.log(\"afterA2:\", a2.getStore());\n",
            "});\n",
            "console.log(\"topA1:\", a1.getStore());\n",
            "a1.run(\"v\", function (x, y) { console.log(\"runArgs:\", x, y); }, \"p\", \"q\");\n",
            "const events = [];\n",
            "const hook = ah.createHook({\n",
            "    init: function (id, type, trigger) { events.push(\"init:\" + id + \":\" + type + \":\" + trigger); },\n",
            "    before: function (id) { events.push(\"before:\" + id); },\n",
            "    after: function (id) { events.push(\"after:\" + id); },\n",
            "    destroy: function (id) { events.push(\"destroy:\" + id); }\n",
            "});\n",
            "hook.enable();\n",
            "const res = new ah.AsyncResource(\"TYP\", 3);\n",
            "console.log(\"resIds:\", res.asyncId(), res.triggerAsyncId());\n",
            "res.runInAsyncScope(function () { console.log(\"scope:\", ah.executionAsyncId(), ah.triggerAsyncId()); }, null, \"arg1\");\n",
            "res.emitDestroy();\n",
            "res.emitDestroy();\n",
            "hook.disable();\n",
            "const res2 = new ah.AsyncResource(\"T2\");\n",
            "res2.runInAsyncScope(function () {});\n",
            "console.log(\"events:\", events.join(\" | \"));\n",
            "console.log(\"execRes:\", typeof ah.executionAsyncResource());\n",
            "console.log(\"providers:\", ah.asyncWrapProviders.PROMISE, ah.asyncWrapProviders.ZLIB);\n",
            "console.log(\"resBind:\", res2.bind(function (u, v) { return u * v; })(3, 4));\n",
            "try { res.runInAsyncScope(\"nofn\"); } catch (e) { console.log(\"rerr:\", e.message); }\n",
            "try { store.run(1); } catch (e) { console.log(\"alsrun:\", e.message); }\n",
            "try { store.exit(); } catch (e) { console.log(\"alsexit:\", e.message); }\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("run: 42 undefined"),
        "run 的额外实参不透传给回调本身：{out}"
    );
    assert!(
        out.contains("events: init:2:TYP:1 | before:2 | after:2 | destroy:2"),
        "生命周期钩子按序触发且 emitDestroy 幂等：{out}"
    );
}

/// inspector：Session 连接状态机、post 应答与事件面
#[test]
fn inspector_session_surface_matches_go() {
    let work = work_dir("inspector");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const insp = require(\"node:inspector\");\n",
            "console.log(\"admin:\", insp.url(), insp.open(), insp.close(), insp.waitForDebugger());\n",
            "console.log(\"consoleFns:\", typeof insp.console.log, typeof insp.console.error, typeof insp.console.trace);\n",
            "console.log(\"netFns:\", typeof insp.Network.dataReceived, typeof insp.Network.requestWillBeSent, typeof insp.NetworkResources.put);\n",
            "insp.console.warn(\"swallowed\");\n",
            "insp.Network.dataReceived();\n",
            "const s = new insp.Session();\n",
            "console.log(\"fns:\", typeof s.on, typeof s.emit, typeof s.connect, typeof s.disconnect, typeof s.post, typeof s.connectToMainThread);\n",
            "let caught = null;\n",
            "try { s.post(\"Runtime.evaluate\"); } catch (e) { caught = e; }\n",
            "console.log(\"notConnected:\", caught.message, \"|\", caught.code, \"|\", caught.name);\n",
            "s.connect();\n",
            "let cb = null;\n",
            "s.post(\"Domain.method\", { p: 1 }, function (err, res) { cb = [err, res.method, res.status]; });\n",
            "console.log(\"postCb:\", cb[0], cb[1], cb[2]);\n",
            "s.disconnect();\n",
            "let caught2 = null;\n",
            "try { s.post(\"X\"); } catch (e) { caught2 = e.message; }\n",
            "console.log(\"afterDisconnect:\", caught2);\n",
            "s.connectToMainThread();\n",
            "let ok = null;\n",
            "s.post(\"Y\", function (err, res) { ok = res.status; });\n",
            "console.log(\"reconnected:\", ok);\n",
            "const s2 = insp.Session();\n",
            "console.log(\"noNew:\", typeof s2.post);\n",
            "const got = [];\n",
            "s.on(\"x\", function (a) { got.push(a); });\n",
            "s.emit(\"x\", 1);\n",
            "console.log(\"events:\", got.join(\",\"), s.listenerCount(\"x\"));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains(
            "notConnected: Session is not connected | ERR_INSPECTOR_NOT_CONNECTED | Error"
        )
    );
}

/// inspector/promises：Promise 版 Session 与管理面
#[test]
fn inspector_promises_session_surface_matches_go() {
    let work = work_dir("inspector_promises");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const ip = require(\"inspector/promises\");\n",
            "console.log(\"admin:\", ip.url(), ip.open(), ip.close(), ip.waitForDebugger());\n",
            "console.log(\"consoleFns:\", typeof ip.console.log, \"net:\", typeof ip.Network.dataReceived, \"res:\", typeof ip.NetworkResources.put);\n",
            "ip.console.warn(\"swallowed\");\n",
            "const s = new ip.Session();\n",
            "console.log(\"fns:\", typeof s.connect, typeof s.disconnect, typeof s.connectToMainThread, typeof s.post, typeof s.on);\n",
            "s.connect();\n",
            "s.disconnect();\n",
            "s.connectToMainThread();\n",
            "const p = s.post(\"Domain.m\", { a: 1 });\n",
            "console.log(\"postType:\", typeof p);\n",
            "let ran = 0;\n",
            "s.on(\"evt\", function (v) { ran = v; });\n",
            "s.emit(\"evt\", 7);\n",
            "console.log(\"emitter:\", ran);\n",
            "async function main() {\n",
            "    const s2 = new ip.Session();\n",
            "    console.log(\"before await\");\n",
            "    try { await s2.post(\"X\"); } catch (e) { }\n",
            "    console.log(\"after await\");\n",
            "}\n",
            "main();\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("postType: object"), "post 返回 Promise：{out}");
    assert!(out.contains("after await"), "await 排空后继续执行：{out}");
}

/// domain：run 成功返回 / run 抛错向调用方传播且保持 enter / intercept 路由 /
/// add 后 emitter error 事件转发到 domain
#[test]
fn domain_run_and_error_capture_matches_go() {
    let work = work_dir("domain_capture");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const domain = require(\"node:domain\");\n",
            "console.log(\"alias:\", domain.createDomain === domain.create, \"active:\", domain.active);\n",
            "const d = domain.create();\n",
            "console.log(\"membersLen:\", d.members.length, \"domainProp:\", d.domain);\n",
            "d.on(\"error\", function (er) { console.log(\"domain error:\", er.message); });\n",
            "console.log(\"runRet:\", d.run(function (a, b) { return a + b; }, 1, 2));\n",
            "console.log(\"afterRun:\", domain.active, \"proc:\", process.domain);\n",
            "try {\n",
            "    d.run(function () { throw new Error(\"boom\"); });\n",
            "} catch (e) {\n",
            "    console.log(\"caught:\", e.message);\n",
            "}\n",
            "console.log(\"afterThrow activeIsD:\", domain.active === d, \"procIsD:\", process.domain === d);\n",
            "const w2 = d.intercept(function (x) { console.log(\"intercept cb:\", x); });\n",
            "w2(undefined, \"hello\");\n",
            "const w1 = d.intercept(function (x) { console.log(\"cb should not run\"); });\n",
            "w1(new Error(\"E1\"), \"ignored\");\n",
            "const events = require(\"events\");\n",
            "const ee = new events.EventEmitter();\n",
            "d.add(ee);\n",
            "console.log(\"eeDomain:\", ee.domain === d, \"members:\", d.members.length, \"first:\", d.members[0] === ee);\n",
            "ee.emit(\"error\", new Error(\"E2\"));\n",
            "ee.on(\"error\", function () { console.log(\"ee own error handler\"); });\n",
            "ee.emit(\"error\", new Error(\"E3\"));\n",
            "console.log(\"eeListeners:\", ee.listenerCount(\"error\"));\n",
            "d.remove(ee);\n",
            "console.log(\"afterRemove:\", d.members.length, ee.domain);\n",
            "ee.emit(\"error\", new Error(\"E4\"));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("caught: boom"),
        "run 内同步抛错向调用方传播：{out}"
    );
    assert!(
        out.contains("afterThrow activeIsD: true procIsD: true"),
        "失败 run 不退出 domain（Go/Node 栈污染语义）：{out}"
    );
    assert!(
        out.contains("domain error: E2"),
        "add 后 emitter error 路由到 domain：{out}"
    );
    assert!(
        out.contains("eeListeners: 2"),
        "转发监听器使 error 计数 +1（Go 同款差异）：{out}"
    );
}

/// domain：enter/exit 栈语义（truncate 照 Go）、bind 重用与不退出、
/// _errorHandler、EventEmitter 方法面与 error 无监听器抛原值
#[test]
fn domain_lifecycle_and_emitter_surface_matches_go() {
    let work = work_dir("domain_lifecycle");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const domain = require(\"node:domain\");\n",
            "const d1 = domain.create();\n",
            "const d2 = new domain.Domain();\n",
            "console.log(\"instanceof:\", d1 instanceof domain.Domain, d2 instanceof domain.Domain);\n",
            "d1.enter();\n",
            "console.log(\"d1:\", domain.active === d1, process.domain === d1);\n",
            "d2.enter();\n",
            "console.log(\"d2:\", domain.active === d2);\n",
            "d1.exit();\n",
            "console.log(\"truncated:\", domain.active === undefined, process.domain === undefined);\n",
            "const d4 = domain.create();\n",
            "d4.on(\"error\", function (e) { console.log(\"d4 error:\", e.message); });\n",
            "const bound = d4.bind(function (x) { return \"bound:\" + x + \":\" + (process.domain === d4); });\n",
            "console.log(bound(5), bound(6));\n",
            "const bad = d4.bind(function () { throw new Error(\"be\"); });\n",
            "try { bad(); } catch (e) { console.log(\"bindThrow:\", e.message, \"stillActive:\", domain.active === d4); }\n",
            "d4.exit();\n",
            "console.log(\"afterExit:\", domain.active === undefined);\n",
            "const w = d4.intercept(function (x) { console.log(\"nope\"); });\n",
            "console.log(\"routes:\", w(new Error(\"IE\")));\n",
            "const wi = d4.intercept(function (x) { return \"ok:\" + x; });\n",
            "console.log(\"pass:\", wi(undefined, 9), \"back:\", domain.active === undefined);\n",
            "const d5 = domain.create();\n",
            "d5.on(\"error\", function (e) { console.log(\"d5 error:\", e.message); });\n",
            "console.log(\"eh:\", d5._errorHandler(new Error(\"E5\")), d5._errorHandler());\n",
            "console.log(\"emitOther:\", d5.emit(\"other\"), d5.listenerCount(\"other\"));\n",
            "d5.once(\"tick\", function (n) { console.log(\"tick once:\", n); });\n",
            "d5.emit(\"tick\", 1); d5.emit(\"tick\", 2);\n",
            "console.log(\"tickCount:\", d5.listenerCount(\"tick\"));\n",
            "d5.setMaxListeners(3);\n",
            "console.log(\"max:\", d5.getMaxListeners());\n",
            "try { d5.emit(\"error\", new Error(\"raw\")); } catch (e) { console.log(\"rawThrow:\", e.message); }\n",
            "const d6 = domain.create();\n",
            "try { d6.emit(\"error\", new Error(\"nolistener\")); } catch (e) { console.log(\"noListener:\", e.message, e.name); }\n",
            "const d7 = domain.create();\n",
            "d7.on(\"error\", function () { console.log(\"d7 this:\", this === d7); });\n",
            "d7.emit(\"error\", new Error(\"E7\"));\n",
            "console.log(\"addPlain:\", d7.add(\"plain\"), d7.members.length);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("truncated: true true"),
        "exit 弹出本 domain 及其上全部条目（照 Go）：{out}"
    );
    assert!(
        out.contains("bound:5:true bound:6:true"),
        "bind 包装可重用且回调内可见 process.domain：{out}"
    );
    assert!(
        out.contains("noListener: nolistener Error"),
        "error 无监听器时 emit 抛原值：{out}"
    );
}
