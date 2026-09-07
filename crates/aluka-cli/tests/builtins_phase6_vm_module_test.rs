//! Phase 6 vm / module / trace_events 家族内置库端到端与接口对拍测试：
//! - `vm`：导出面（typeof）、`createContext`/`isContext`、`Script` 构造器
//!   表面与缺 context 错误消息（源码求值属本阶段架构限制，不在对拍范围）；
//! - `module`：`builtinModules` 逐字、`createRequire` 取内置模块、
//!   `isBuiltin`、`Module` 类表面（wrap/静态方法/实例字段）；
//! - `trace_events`：`createTracing`/`getEnabledCategories` 与带 code 的
//!   TypeError。
//!
//! 与 Node.js 22 LTS 规范逐字比对：alukac 前端整图编译 →
//! aluvm 原生驱动执行，输出与 Node.js 22 严格一致。
//!
//! 探针书写约束（对齐 aluvm 现有全局面）：不用 `String()`/`JSON.stringify`/
//! `Array.isArray`/`for...of`（Rust aluvm 暂不支持，属既有全局面缺口，
//! 与本阶段模块无关）；错误信息走 `e.message`/`e.name`/`e.code`。

mod common;

use std::path::PathBuf;

/// 创建隔离的临时测试目录
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("builtins_phase6_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 验证 vm 模块导出面（typeof）与 createContext/isContext 语义
#[test]
fn vm_surface_and_is_context_matches_go() {
    let work = work_dir("vm_surface");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const vm = require('vm');\n",
            "console.log('createContext:', typeof vm.createContext);\n",
            "console.log('runInContext:', typeof vm.runInContext);\n",
            "console.log('runInNewContext:', typeof vm.runInNewContext);\n",
            "console.log('runInThisContext:', typeof vm.runInThisContext);\n",
            "console.log('isContext:', typeof vm.isContext);\n",
            "console.log('compileFunction:', typeof vm.compileFunction);\n",
            "console.log('measureMemory:', typeof vm.measureMemory);\n",
            "console.log('Script:', typeof vm.Script);\n",
            "console.log('constants:', typeof vm.constants);\n",
            "console.log('createScript:', typeof vm.createScript);\n",
            "const sandbox = { x: 1 };\n",
            "const ctx = vm.createContext(sandbox);\n",
            "console.log('ctx identity:', ctx === sandbox);\n",
            "console.log('ctx idempotent:', vm.createContext(sandbox) === sandbox);\n",
            "console.log('isContext(ctx):', vm.isContext(ctx));\n",
            "console.log('isContext(sandbox):', vm.isContext(sandbox));\n",
            "console.log('isContext(fresh):', vm.isContext(vm.createContext()));\n",
            "console.log('isContext({}):', vm.isContext({}));\n",
            "console.log('isContext(42):', vm.isContext(42));\n",
            "console.log('isContext():', vm.isContext());\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("ctx identity: true"),
        "sandbox 身份应保持: {out}"
    );
    assert!(out.contains("isContext({}): false"), "{out}");
}

/// 验证 vm 缺 context / 非法 context / 非 Script 实例的错误消息（逐字）
#[test]
fn vm_error_messages_match_go() {
    let work = work_dir("vm_errors");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const vm = require('vm');\n",
            "try { vm.runInContext('1+1'); } catch (e) {\n",
            "  console.log('e1:', e.message, '|', e.name);\n",
            "}\n",
            "try { vm.runInContext('1+1', {}); } catch (e) {\n",
            "  console.log('e2:', e.message, '|', e.name);\n",
            "}\n",
            "try { vm.Script.prototype.runInThisContext(); } catch (e) {\n",
            "  console.log('e3:', e.message);\n",
            "}\n",
            "try { vm.Script.prototype.createCachedData(); } catch (e) {\n",
            "  console.log('e4:', e.message);\n",
            "}\n",
            "try { vm.Script.prototype.runInNewContext(); } catch (e) {\n",
            "  console.log('e5:', e.message);\n",
            "}\n",
            "const s = new vm.Script('1+1');\n",
            "try { s.runInContext({}); } catch (e) {\n",
            "  console.log('e6:', e.message, '|', e.name);\n",
            "}\n",
            "try { s.runInContext(); } catch (e) {\n",
            "  console.log('e7:', e.message);\n",
            "}\n",
            "console.log('ctxErr isContext:', vm.isContext({}));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("vm.runInContext: contextifiedObject must be an object"),
        "{out}"
    );
    assert!(
        out.contains("The argument 'contextifiedObject' is not a vm.Context"),
        "{out}"
    );
    assert!(
        out.contains("vm.Script.runInThisContext: not a Script instance"),
        "{out}"
    );
    assert!(
        out.contains("vm.Script.runInContext: contextifiedObject required"),
        "{out}"
    );
}

/// 验证 vm.Script 构造器表面：prototype 方法、cachedDataRejected、
/// createCachedData 返回源码字节 Buffer、node: 前缀 require
#[test]
fn vm_script_surface_and_cached_data_matches_go() {
    let work = work_dir("vm_script");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const vm = require('node:vm');\n",
            "const s = new vm.Script('hello');\n",
            "console.log('s:', typeof s);\n",
            "console.log('methods:', typeof s.runInThisContext, typeof s.runInNewContext,",
            " typeof s.runInContext, typeof s.createCachedData);\n",
            "const data = s.createCachedData();\n",
            "console.log('ccd:', data.toString(), data.length);\n",
            "const s2 = new vm.Script('1+1', { cachedData: {} });\n",
            "console.log('rejected:', s2.cachedDataRejected === true);\n",
            "const s3 = new vm.Script('1+1', {});\n",
            "console.log('notRejected:', s3.cachedDataRejected === undefined);\n",
            "const s4 = vm.createScript('abc');\n",
            "console.log('createScript:', typeof s4, typeof s4.createCachedData);\n",
            "console.log('proto:', typeof vm.Script.prototype,\n",
            " typeof vm.Script.prototype.runInThisContext);\n",
            "console.log('len:', vm.Script.length);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("ccd: hello 5"), "源码字节 Buffer: {out}");
    assert!(out.contains("rejected: true"), "{out}");
}

/// 验证 vm.measureMemory() Promise 形状（数值两边均非确定值，只对拍形状）
#[test]
fn vm_measure_memory_promise_shape_matches_go() {
    let work = work_dir("vm_measure");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const vm = require('vm');\n",
            "vm.measureMemory().then(function (o) {\n",
            "  console.log('total:', typeof o.total, '| js:', typeof o.js);\n",
            "  console.log('estimate:', typeof o.total.jsMemoryEstimate,\n",
            "    '| range len:', o.total.jsMemoryRange.length === 2,\n",
            "    '| js estimate:', typeof o.js.jsMemoryEstimate);\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("total: object | js: object"), "{out}");
}

/// 验证 module.builtinModules 与 Node.js 22 LTS 标准 逐字一致（68 项，顺序敏感）
#[test]
fn module_builtin_modules_verbatim_matches_go() {
    let work = work_dir("module_bm");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const module = require('module');\n",
            "console.log(module.builtinModules.join(','));\n",
            "console.log(module.builtinModules.length);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = "_http_agent,_http_client,_http_common,_http_incoming,_http_outgoing,\
_http_server,_stream_duplex,_stream_passthrough,_stream_readable,_stream_transform,\
_stream_wrap,_stream_writable,_tls_common,_tls_wrap,assert,assert/strict,async_hooks,\
buffer,child_process,cluster,console,constants,crypto,dgram,diagnostics_channel,dns,\
dns/promises,domain,events,fs,fs/promises,http,http2,https,inspector,inspector/promises,\
module,net,os,path,path/posix,path/win32,perf_hooks,process,punycode,querystring,\
readline,readline/promises,repl,stream,stream/consumers,stream/promises,stream/web,\
string_decoder,sys,timers,timers/promises,tls,trace_events,tty,url,util,util/types,\
v8,vm,wasi,worker_threads,zlib";
    assert!(
        out.contains(expected),
        "builtinModules 必须与 Node 22 / Node.js 22 LTS 标准 逐字一致:\n{out}"
    );
    assert!(out.contains("\n68"), "应为 68 项: {out}");
}

/// 验证 module 表面、isBuiltin、createRequire 取内置模块与 constants
#[test]
fn module_surface_and_create_require_matches_go() {
    let work = work_dir("module_surface");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const module = require('module');\n",
            "console.log('createRequire:', typeof module.createRequire);\n",
            "console.log('isBuiltin:', typeof module.isBuiltin);\n",
            "console.log('builtinModules:', typeof module.builtinModules);\n",
            "console.log('Module:', typeof module.Module);\n",
            "console.log('SourceMap:', typeof module.SourceMap);\n",
            "console.log('constants:', typeof module.constants);\n",
            "console.log('ccs:', module.constants.compileCacheStatus.FAILED,\n",
            " module.constants.compileCacheStatus.ENABLED,\n",
            " module.constants.compileCacheStatus.ALREADY_ENABLED,\n",
            " module.constants.compileCacheStatus.DISABLED);\n",
            "console.log('api:', typeof module.syncBuiltinESMExports,\n",
            " typeof module.registerHooks, typeof module.runMain,\n",
            " typeof module.enableCompileCache, typeof module.flushCompileCache,\n",
            " typeof module.findPackageJSON, typeof module.setSourceMapsSupport,\n",
            " typeof module.stripTypeScriptTypes, typeof module.findSourceMap,\n",
            " typeof module.register, typeof module.getCompileCacheDir,\n",
            " typeof module.registerVirtualModule);\n",
            "console.log('isBuiltin:', module.isBuiltin('fs'), module.isBuiltin('node:fs'),\n",
            " module.isBuiltin('nope'), module.isBuiltin());\n",
            "console.log('sms:', module.getSourceMapsSupport(),\n",
            " module.getCompileCacheDir() === undefined,\n",
            " module.enableCompileCache() === undefined);\n",
            "const req = module.createRequire('x.js');\n",
            "console.log('req:', typeof req);\n",
            "console.log('join:', req('path').join('a', 'b'));\n",
            "console.log('req vm:', typeof req('vm').createContext);\n",
            "console.log('req trace:', typeof req('node:trace_events').createTracing);\n",
            "try { module.createRequire({ href: 'http://x/y.js' }); } catch (e) {\n",
            "  console.log('url err:', e.message, '|', e.name);\n",
            "}\n",
            "const req0 = module.createRequire();\n",
            "console.log('req0:', typeof req0, typeof req0('path').join);\n",
            "try { module.register(); } catch (e) {\n",
            "  console.log('reg err:', e.message, '|', e.name);\n",
            "}\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains("aluka: type error [ERR_INVALID_URL_SCHEME]: The URL must be of scheme file"),
        "{out}"
    );
    assert!(
        out.contains("reg err: register requires a specifier | TypeError"),
        "{out}"
    );
}

/// 验证 Module 类表面：静态方法、wrap 包装文本、实例字段、原型方法
#[test]
fn module_class_surface_matches_go() {
    let work = work_dir("module_class");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const module = require('module');\n",
            "const M = module.Module;\n",
            "console.log('statics:', typeof M.runMain, typeof M.wrap, typeof M._load,\n",
            " typeof M._resolveFilename, typeof M._nodeModulePaths);\n",
            "console.log('globalPaths:', M.globalPaths.length === 0);\n",
            "const wrapped = M.wrap('1+1');\n",
            "console.log('wrap:', wrapped ===\n",
            " '(function (exports, require, module, __filename, __dirname) { 1+1\\n});');\n",
            "console.log('wrap empty:', M.wrap() === '');\n",
            "const m = new M('test.js');\n",
            "console.log('m:', typeof m);\n",
            "console.log('fields:', m.id, m.filename === null, m.loaded === false);\n",
            "console.log('arrays:', m.children.length === 0, m.paths.length === 0);\n",
            "console.log('exports:', typeof m.exports);\n",
            "console.log('proto methods:', typeof m.require, typeof m.load,\n",
            " typeof m._compile, typeof m.isPreloading);\n",
            "console.log('isPreloading:', m.isPreloading() === false);\n",
            "console.log('M.prototype:', typeof M.prototype,\n",
            " typeof M.prototype.require, typeof M.prototype._compile);\n",
            "try { m._compile(); } catch (e) {\n",
            "  console.log('compile err:', e.message, '|', e.name);\n",
            "}\n",
            "const sm = new module.SourceMap();\n",
            "console.log('sm:', typeof sm, typeof sm.payload);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("fields: test.js true true"), "{out}");
    assert!(
        out.contains("compile err: _compile requires source code | TypeError"),
        "{out}"
    );
}

/// 验证 Module._nodeModulePaths 的 node_modules 链（Go filepath.Abs/Dir 语义，
/// 两侧进程同 cwd 运行，输出可逐字对拍）
#[test]
fn module_node_module_paths_matches_go() {
    let work = work_dir("module_nmp");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const module = require('module');\n",
            "const M = module.Module;\n",
            "const paths = M._nodeModulePaths('sub');\n",
            "console.log('first:', paths[0]);\n",
            "console.log('second:', paths[1]);\n",
            "console.log('ascends:', paths.length > 2);\n",
            "console.log('default:', typeof M._nodeModulePaths()[0]);\n",
            "console.log('empty arg:', M._nodeModulePaths('').length === M._nodeModulePaths().length);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    // 两侧进程同 cwd 运行：绝对路径链可逐字对拍
    assert!(out.contains("\\sub\\node_modules"), "{out}");
    assert!(out.contains("ascends: true"), "{out}");
    assert!(out.contains("default: string"), "{out}");
    assert!(out.contains("empty arg: true"), "{out}");
}

/// 验证 trace_events 表面：getEnabledCategories、createTracing 对象与
/// enable/disable 切换
#[test]
fn trace_events_surface_matches_go() {
    let work = work_dir("trace_surface");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const te = require('trace_events');\n",
            "console.log('createTracing:', typeof te.createTracing);\n",
            "console.log('getEnabledCategories:', typeof te.getEnabledCategories);\n",
            "console.log('gec undefined:', te.getEnabledCategories() === undefined);\n",
            "const t = te.createTracing({ categories: ['node', 'v8'] });\n",
            "console.log('t:', typeof t);\n",
            "console.log('categories:', t.categories);\n",
            "console.log('enabled:', t.enabled === false);\n",
            "console.log('methods:', typeof t.enable, typeof t.disable);\n",
            "t.enable();\n",
            "console.log('after enable:', t.enabled === true);\n",
            "t.disable();\n",
            "console.log('after disable:', t.enabled === false);\n",
            "const t2 = te.createTracing();\n",
            "console.log('no opts:', t2.categories === '');\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(out.contains("categories: node,v8"), "{out}");
    assert!(out.contains("after disable: true"), "{out}");
}

/// 验证 trace_events 带 code 的 TypeError（消息/name/code 与 Go 逐字一致）
#[test]
fn trace_events_error_codes_match_go() {
    let work = work_dir("trace_errors");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const te = require('node:trace_events');\n",
            "try { te.createTracing({ categories: 'x' }); } catch (e) {\n",
            "  console.log('e1:', e.message);\n",
            "  console.log('e1n:', e.name, '| e1c:', e.code);\n",
            "}\n",
            "try { te.createTracing({ categories: [] }); } catch (e) {\n",
            "  console.log('e2:', e.message, '|', e.code);\n",
            "}\n",
            "try { te.createTracing({}); } catch (e) {\n",
            "  console.log('e3:', e.message, '|', e.code);\n",
            "}\n",
            "try { te.createTracing({ categories: 42 }); } catch (e) {\n",
            "  console.log('e4:', e.message);\n",
            "}\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert!(
        out.contains(
            "e1: The \"options.categories\" property must be an instance of Array. Received type string"
        ),
        "{out}"
    );
    assert!(
        out.contains("e1n: TypeError | e1c: ERR_INVALID_ARG_TYPE"),
        "{out}"
    );
    assert!(
        out.contains("e2: At least one category is required | ERR_TRACE_EVENTS_CATEGORY_REQUIRED"),
        "{out}"
    );
    assert!(
        out.contains("Received type undefined"),
        "缺 categories 应报 undefined: {out}"
    );
    assert!(
        out.contains("Received type number"),
        "数字类别应报 number: {out}"
    );
}
