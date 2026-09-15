# 2026-09-15 路 缁疆 TODO锛圡7.2 杞節鍗佷簲锛氱湡瀹為」鐩樆濉炵己闄蜂慨澶嶁€斺€斾簨浠跺惊鐜?瀹氭椂鍣?async 璇箟/HTTP 瀹炰緥闈級

> 鎬?TODO 瑙?[../README.md](../README.md)锛涗笂涓€杞 [./README-round94.md](./README-round94.md)銆?
> 璇佹嵁瑙勫垯瑙?[../README.md](../README.md) 搂0锛涢棬绂佸懡浠よ AGENTS.md 搂3銆?

**褰撳墠閲岀▼纰?*锛歁7锛圡7.2 鐪熷疄鐢熸€佹壙杞斤級銆€|銆€**鏉冨▉ Oracle**锛歂ode.js 22 LTS锛堟湰鏈?v22.3.0锛?

**鏈疆鐩爣**锛氫慨澶嶈疆涔濆崄鍥涚櫥璁扮殑 3 椤归樆濉炵己闄凤紙璺ㄦā鍧楄娲惧彂 / HTTP 鍥炶皟涓嶆墽琛?/ 瀛愮洰褰曞叆鍙ｏ級锛?
浣?`demo/taskboard-demo` 绔埌绔窇閫氬苟涓?Node 閫愬瓧鑺傚鎷嶃€?

**缁撹**锛?*杈炬垚**鈥斺€擿node e2e.js` 涓?`aluka run e2e.js` 鍚?54 琛屻€乣exit 0`锛?*閫愬瓧鑺備竴鑷达紙IDENTICAL锛?*銆?
鏈疆鍏变慨澶?**5 绫荤己闄?*锛? 椤逛负鐪熷疄椤圭洰闃诲椤癸紝1 椤逛负璇婃柇涓彂鐜扮殑鏍稿績璇箟缂洪櫡锛夛紝
鍏朵腑 2 椤规槸杞節鍗佸洓鏍瑰洜鎶ュ憡鐨?*淇**锛堝師鍒ゆ柇琚柊璇佹嵁鎺ㄧ炕锛岃瑙?搂2锛夈€?

---

## 1. 待办与结果


| # | 寰呭姙 | 缁撴灉 |
|---|---|:---:|
| 1 | 瀹氫綅骞朵慨澶嶃€岃法妯″潡闂寘/绫昏皟鐢ㄨ鎵ц鍙︿竴妯″潡鍑芥暟浣撱€?| `[x]` 鏍瑰洜**淇**涓?`resolve_callable` 閬楃暀鍚彂寮忥紙搂2.1锛?|
| 2 | 瀹氫綅骞朵慨澶嶃€孒TTP 鍥炶皟浣撲笉鎵ц / listen 姘镐笉 resolve銆?| `[x]` 鏍瑰洜鏄?*瀹氭椂鍣ㄨ皟搴︿笁缂洪櫡**锛埪?.2锛?|
| 3 | 淇 async 鍑芥暟鍚屾鎶涢敊璇箟锛堣鑼冭姹傝繑鍥?rejected Promise锛?| `[x]` 搂2.3 |
| 4 | 琛?HTTP 瀹炰緥浜嬩欢闈紙`removeAllListeners`/`listenerCount` 绛夛級 | `[x]` 搂2.4 |
| 5 | 鐪熷疄椤圭洰 e2e 涓?Node 閫愬瓧鑺傚鎷?| `[x]` **IDENTICAL**锛埪?锛?|
| 6 | 闂ㄧ锛坒mt / clippy / 鍏ㄩ噺 test锛?| `[x]` 搂4 |
| 7 | 璇佹嵁鍥炲～涓?`git diff` 澶嶅 | `[x]` 搂5 |

---

## 2. 缂洪櫡鏍瑰洜涓庝慨澶?

### 2.1 銆愪弗閲嶃€慲resolve_callable` 閬楃暀鍚彂寮忔妸鏅€氬璞¤褰撳嚱鏁版ā鏉匡紙**淇杞節鍗佸洓鐨勬牴鍥犲垽鏂?*锛?

杞節鍗佸洓鍒ゅ畾涓恒€宍run_module` 鏁翠綋鏇挎崲 `module_functions` 瀵艰嚧闂寘绱㈠紩澶辨晥銆嶃€傛湰杞敤
`ALUKA_ERR_TRACE` + 瀹氭椂鍣?璋冪敤杩借釜鍙戠幇锛?*`require` 璺緞瀹為檯璧?append-only + 閲嶅畾浣?*
锛坄modules.rs:179-245`锛歚fn_base`/`class_base` 閲嶅啓 `MakeClosure`/`MakeClass`銆?
`constructor_index`/`method.func_index`锛沗eval.rs::append_module_inner` 鍚屾锛夛紝
绱㈠紩鍒嗛厤缁?`ALUKA_REQ_DEBUG` 瀹炴祴姝ｇ‘锛坈onfig=1..2 / logger=3..17 / errors=18..26 /
store=27..42 / service=43..63锛夈€?

**鐪熷疄鏍瑰洜**锛堝喅瀹氭€ц瘉鎹紝`ALUKA_ERR_TRACE`锛夛細

```text
[req-dbg] invoke func_idx=25 name="ConflictError_constructor"   鈫?new ConflictError 鉁?
[req-dbg] invoke func_idx=20 name="AppError_constructor"        鈫?super(m) 鈫?AppError 鉁?
[req-dbg] invoke func_idx=55 name="TaskService_remove"          鈫?AppError 鐨?super(m) 鈫?鏈簲鍐呭缓 Error锛?
```

`call.rs::resolve_callable` 鏈変竴鏉?*娌℃湁浠讳綍鍒涘缓鑰?*鐨勯仐鐣欏洖閫€锛?

```rust
if (r.0 as usize) < self.module_functions.len() {
    return (Some(r.0 as usize), Vec::new());   // 鎶娿€屽爢绱㈠紩銆嶅綋銆屽嚱鏁版ā鏉跨储寮曘€?
}
```

`Error` 杩欎釜 `NativeCtor` 鐨?*鍫嗙储寮?55 < 琛ㄩ暱 64**锛屼簬鏄鍔寔鎴?`module_functions[55]`
= `TaskService_remove` 骞舵墽琛屽叾鍑芥暟浣撱€傝〃闀块殢妯″潡杩藉姞澧為暱 鈬?**椤哄簭/甯冨眬鏁忔劅**
锛堣疆涔濆崄鍥涜瀵熷埌鐨勩€岄『搴忎緷璧栥€嶃€屽姞涓€涓嚱鏁板悗琛ㄧ幇鍙樺寲銆嶆鏄鍥狅級銆?

**淇**锛氬垹闄よ鍥為€€锛坄call.rs::resolve_callable`銆乣interpreter.rs` CALL_METHOD 涓?CALL
涓ゅ鍚屾鍒嗘敮锛夛紝骞惰ˉ娉ㄩ噴璇存槑涓轰綍鍗遍櫓銆傛ā鏉夸竴寰嬬粡 `alloc_closure*` 鍖呰锛屾棤鍚堟硶鍒涘缓鑰呫€?

**楠岃瘉**锛歚pb-c.js`锛堝師澶辫触锛変笌 `pb-step5.js`锛堟渶灏忚Е鍙戯細`require` 鍓?5 涓ā鍧楀悗鏋勯€?
`ConflictError`锛夌幇鍧囪緭鍑?`ok: "ConflictError" "CONFLICT"`锛屼笌 Node 涓€鑷达紱
`pb-h.js`锛堝師瀵圭収锛変繚鎸侀€氳繃銆?

### 2.2 銆愪弗閲嶃€戝畾鏃跺櫒璋冨害涓夌己闄凤紙HTTP `listen` 鍥炶皟琚タ姝荤殑鐪熷洜锛?

`demo/taskboard-demo` 鐨?`TaskServer.listen()` 姘镐笉 resolve銆傞€愭鎻掓々鍙栬瘉
锛坄tools/probe-timer-vs-io.js` / `probe-timer-order.js` / `probe-nested-timer.js` /
`probe-http-close.js`锛夊悗瀹氫綅鍒?`drain_macro_tasks`/`wait_until_due` 鐨勪笁涓己闄凤細

| # | 缂洪櫡 | 鐜拌薄锛堟帰閽堬級 | 淇 |
|---|---|---|---|
| a | 鍒版湡鏃堕棿 = **闃熷熬 due + delay**锛屼笖鎺掔┖ `now` 浠?**0** 璧风畻 | `setTimeout(f,3000)` 鍚庢敞鍐岀殑 `setTimeout(g,300)` 鎺掑埌 3300ms 鈫?**椤哄簭棰犲€?*锛圢ode锛?00 鍏堬級 | 鏂板鎸佷箙**铏氭嫙鏃堕挓** `macro_clock_ms`锛歚due = clock + delay`锛屾帓绌轰粠璇ユ椂閽熻捣绠楀苟鍐欏洖 |
| b | 绛夊緟鏈熼棿**鍙车浜嬩欢婧愩€佷笉鎺掑井浠诲姟** | `listen` 鍥炶皟閲?`resolve()` 鐨?`.then` 缁綋瑕佺瓑瀹氭椂鍣ㄥ叏閮ㄥ埌鏈?| 鏂板 `pump_io_and_microtasks()`锛氭车鍚庣珛鍗?`drain_microtasks()`锛堢瓑寰呭垎鐗囧唴鍚屾牱璋冪敤锛?|
| c | 蹇収寮忔帓绌猴細鎵ц鏈熼棿鏂版敞鍐岀殑浠诲姟瑕佺瓑鏈疆缁撴潫锛涗笖绛夊緟涓嶉噸璇勪及 | 瀹氭椂鍣ㄥ洖璋冨唴鍐嶆敞鍐岀殑瀹氭椂鍣ㄣ€乣setImmediate`銆乣server.close(cb)` **鍏ㄩ儴涓嶈Е鍙?*锛涢暱瀹氭椂鍣紙濡?6000ms watchdog锛変細鍏堣窇骞?`process.exit` 楗挎鏂颁换鍔?| 鈶?姣忚疆杩唬鍚堝苟 `self.macro_tasks` 鏂颁换鍔★紱鈶?姣忎釜鍥炶皟鍚?`drain_microtasks()`锛涒憿 `wait_until_due` 鍙戠幇銆屾洿鏃╁埌鏈熺殑鏂颁换鍔°€嶅嵆璁╁嚭锛宒rain 鎶婂綋鍓嶄换鍔℃斁鍥炲苟鎸夋渶鏃╁埌鏈熼噸閫?|

鍙︽妸 7 澶勫垎鏁ｇ殑瀹忎换鍔＄櫥璁扮粺涓€涓?`Vm::schedule_macro_task(delay, cb, args, repeating)`
锛坄interpreter.rs` 鐨?setTimeout/setInterval/setImmediate銆乣timers.rs`銆乣http/state.rs`銆?
`http/client.rs`銆乣http2.rs`銆乣zlib.rs`銆乣crypto/async_cb.rs`锛夛紝娑堥櫎閲嶅鐨?
last_due 璁＄畻銆?

**楠岃瘉**锛坄tools/probe-timer-order.js`锛屼笌 Node 瀵圭収锛夛細

```text
淇鍓? registered +0ms / timer(3000) +5206ms / timer(300) +5506ms   鈫?椤哄簭棰犲€?
淇鍚? registered +1ms / timer(300)  +531ms  / timer(3000) +3232ms  鈫?椤哄簭姝ｇ‘锛圢ode: +305/+3003锛?
```

`tools/probe-http-close.js`锛氫慨澶嶅墠 `closed` 浠庝笉瑙﹀彂锛涗慨澶嶅悗涓?Node 鍚屽簭
锛坄listening 鈫?close() called 鈫?closed`锛夈€俙tools/probe-nested-arrow-promise.js`锛?
`Promise` 鎵ц鍣ㄥ唴宓屽绠ご + 涓夊厓琛ㄨ揪寮忚皟鐢?`resolve/reject` 褰㈡€佷笌 Node 涓€鑷淬€?

### 2.3 銆愭牳蹇冭涔夈€慳sync 鍑芥暟鍚屾鎶涢敊搴旇浆涓?rejected Promise

```text
async function f() { throw new Error('x'); }
f().catch(e => ...)      // Node锛歝atch 鍛戒腑
```

`invoke_function` 鐨?async 鍖呰鍙鐞?`Ok(v)`锛歚throw` 鍦ㄩ娆?`await` 涔嬪墠鍙戠敓鏃朵互
`Err(Thrown)` 閫冮€稿埌璋冪敤鏂广€傜湡瀹為」鐩〃鐜帮細`async route()` 鐨?404 寮傚父绌块€?
`handle().catch(...)`锛屽彉鎴愭ā鍧楃骇鏈崟鑾烽敊璇紙`demo/taskboard-demo` 鐨?`/tasks/T999` 璺緞锛夈€?
**淇**锛氭柊澧?`Err(VmError::Thrown(reason)) if tmpl.is_async` 鍒嗘敮 鈫?
`alloc_rejected_promise(reason)`锛堣鑼?AsyncFunctionStart/AsyncBlockStart锛夈€?
**楠岃瘉**锛歚tools/probe-async-throw.js` 涓夋潯锛堝悓姝ユ姏 / await 鍚庢姏 / 璺?async 鍐呭眰鎶涳級
鍏ㄩ儴涓?Node 涓€鑷达紙`CAUGHT-1/2/3`锛夈€?

### 2.4 銆愬唴缃潰缂哄彛銆慔TTP 瀹炰緥浜嬩欢鏂规硶缂哄け

413 璺緞鎶?`req.removeAllListeners is not a function`銆侶TTP 瀹炰緥瀵硅薄璧?`_builtinNs`
瀹炰緥鐩戝惉鍣ㄨ〃锛坄http:server`/`http:response`/`http:message`/`http:request`锛夛紝
鍘熶粎瑕嗙洊 `on/addListener/once/off/removeListener`銆?*淇**锛氭柊澧?
`state::remove_all_listeners`锛堟寜浜嬩欢鍚嶆垨鍏ㄦ竻锛変笌 `state::listener_count`锛?
骞跺湪鍥涗釜瀹炰緥鍛藉悕绌洪棿 + 鍥涗釜瀹炰緥瀵硅薄鐨勬柟娉曡〃琛?`removeAllListeners`/`listenerCount`锛?
椤哄甫涓?`HeapObject::EventEmitter` 鍒嗘淳琛?`addListener`锛坄on` 鍒悕锛?`listenerCount`/`listeners`銆?
**楠岃瘉**锛歚tools/probe-http-413.js` 杈撳嚭涓?Node 涓€鑷达紙`413 {"error":"PAYLOAD_TOO_LARGE",鈥`锛夈€?
## 3. 杈炬垚璇佹嵁锛氱湡瀹為」鐩笌 Node 閫愬瓧鑺備竴鑷?

```text
$ cd demo/taskboard-demo && node e2e.js   > node.txt      锛坋xit 0锛?
$ aluka run e2e.js                        > aluka.txt     锛坋xit 0锛?

node-lines=54   aluka-lines=54
E2E: IDENTICAL
```

瑕嗙洊鑼冨洿锛?4 琛岀‘瀹氭€ц緭鍑猴紝鍚厤缃?/ 鏃ュ織 / **鐪熷疄 npm 渚濊禆 `ms`** / JSON 鏂囦欢瀛樺偍 /
涓氬姟瑙勫垯 / **鐪熷疄 HTTP 寰€杩?9 涓鐐?* / 10 璺苟鍙?/ 鏀跺熬鎸佷箙鍖栨牳瀵癸級锛?

```text
### config       host/port/logLevel銆佹淳鐢熷瓧娈点€侀敭搴忋€佺粨鏋勫寲鏃ュ織锛堝惈 with() 娲剧敓涓庣骇鍒繃婊わ級
### dependency   ms(60000)=1m / ms("2d")=172800000 / ms(1500,{long:true})=2 seconds
### store        鍒涘缓/钀界洏/閲嶈浇銆佹寚绾广€佸敮涓€鎬х害鏉燂紙409锛夈€佸瓧娈垫牎楠岋紙400 + details锛?
### service      鐘舵€佹祦杞€佺粺璁★紙counts/completion/topTags锛夈€佽繃婊や笌鎺掑簭銆丯otFound锛?04锛?
### http         GET /health銆丟ET /tasks?status=銆丳OST(201+Location)銆丟ET/PATCH/DELETE /tasks/:id銆?
                 invalid(400)銆乀999(404)銆乼oo large(413)銆丳UT(405)
### concurrency  10 璺苟琛?GET 鍏ㄩ儴 200 涓?id 搴忓垪涓€鑷?
### teardown     serverClosed銆佹寔涔呭寲 4 鏉°€乧reatedAt 鍥哄畾鏃堕挓绋冲畾
```

## 4. 闂ㄧ锛堝叏缁匡級

```text
$ cargo fmt --all --check                                    FMT_EXIT=0
$ cargo clippy --all-targets --all-features -- -D warnings    CLIPPY_EXIT=0
$ cargo test --workspace --all-features --no-fail-fast -- \
    --skip tty_surface_e2e_matches_go --skip readline_eof_close_e2e_matches_go
TEST_EXIT=0
suites=92  passed=652  failed=0
test node22_conformance_matches_node_stdout ... ok     鈫?Node 22 鍏ㄩ噺宸垎
test test262_subset_conformance ... ok                 鈫?test262 瀛愰泦 1130/1154
```

> 涓や緥 console 鏁忔劅鐢ㄤ緥鐨勮烦杩囧悓杞節鍗佸洓锛圵MI 鑴辩浼氳瘽鏃犳帶鍒跺彴锛涘凡鍦ㄥ墠鍙板崟鐙楠岋級銆?

**鎺㈤拡鍥炲綊**锛堜慨澶嶅悗涓?Node 閫愯 diff锛夛細`tools/cap-probe.js`銆乣probe-this.js`銆?
`probe-this2.js`銆乣probe-rm.js`銆乣probe-crossmod.js`銆乣pb-h.js`銆乣pb-c.js`銆?
`pb-step5.js` 鍏ㄩ儴 **IDENTICAL**锛沗probe-errors.js` 浠呭樊 `Error` 鑷湁 `stack`锛堟棦鏈夛紝搂6锛夛紱
`probe-timer-*` / `probe-http-close` / `probe-nested-timer` / `probe-nested-arrow-promise`
宸紓浠呭湪**姣鏁板€?*锛圓luka 鏇村揩锛夛紝浜嬩欢椤哄簭涓€鑷淬€?

## 5. `git diff` 澶嶅

```text
 crates/aluka-vm/src/call.rs                        |  33 ++--  resolve_callable 鍘婚仐鐣欏洖閫€ + async 鍚屾鎶涢敊鍖呰
 crates/aluka-vm/src/microtask.rs                   |  90 ++++--  铏氭嫙鏃堕挓 / 閫愬洖璋冨井浠诲姟 / 绛夊緟璁╁嚭 / 缁熶竴鐧昏
 crates/aluka-vm/src/interpreter.rs                 |  99 ++++--  EventEmitter 鏂规硶琛ュ叏 + schedule_macro_task + CALL 鍥為€€绉婚櫎
 crates/aluka-vm/src/builtins/http/{state,server,mod,client}.rs | 151 ++++++--  瀹炰緥浜嬩欢闈㈣ˉ鍏?
 crates/aluka-vm/src/builtins/{timers,http2,zlib}.rs / crypto/async_cb.rs | 53 ++--  鐧昏缁熶竴
 + demo/taskboard-demo/锛氭帰閽堝綊妗ｏ紙probe-timer-order / probe-timer-vs-io / probe-http-close /
   probe-http-413 / probe-async-throw / probe-nested-timer / probe-nested-arrow-promise锛?
```

閫愬潡瀹℃牳缁撹锛氫粎鍚湰杞?5 绫讳慨澶?+ 鎺㈤拡褰掓。锛?*鏃犲す甯︽敼鍔?*锛屾棤璋冭瘯娈嬬暀
锛坄ALUKA_TIMER_DBG` / `ALUKA_REQ_DEBUG` 涓存椂鎻掓々宸插叏閮ㄧЩ闄わ紝浠呬繚鐣欎粨搴撴棦鏈夌殑
`ALUKA_REQ_DEBUG` / `ALUKA_ERR_TRACE` 璇婃柇寮€鍏筹級銆?

## 6. 浠嶆湭淇锛堢櫥璁帮級

| # | 椤?| 璇存槑 |
|---|---|---|
| 1 | `Error` 瀹炰緥缂鸿嚜鏈?`stack` 灞炴€?| `Object.getOwnPropertyNames(err)` 灏?`stack`锛圢ode 鏈夛級锛沗probe-errors.js` 鍞竴宸紓 |
| 2 | 鍏ュ彛浣嶄簬瀛愮洰褰曟椂璺ㄧ洰褰?`require` 涓嶅彲瑙ｆ瀽 | 杞節鍗佸洓 搂4.2 鏈慨锛歚aluka run tools/x.js` 鐨?`../src/*` 钀藉叆 `_ext/`锛涘悓鏍峰奖鍝?`aluka test test/` |
| 3 | `process.argv` 鏈€忎紶鍛戒护琛屽弬鏁?| 杞節鍗佸洓 搂4.3 |
| 4 | 鏍圭洰褰?`aluka.exe` 闄堟棫 / `--capabilities` 鎶?`native: 0` | 杞節鍗佷笁 搂5 鏃㈡湁鐧昏 |
| 5 | 瀹氭椂鍣?浜嬩欢寰幆銆岀湡瀹炴椂閽熴€嶄繚鐪熷害 | 鐜颁负铏氭嫙鏃堕挓鎺ㄨ繘锛堥『搴忔纭€佹暟鍊兼洿蹇級锛汵ode 鐨?I/O 闃舵涓庡畾鏃跺櫒闃舵鐨勭簿纭氦閿欐湭閫愪竴瀵归綈 |


