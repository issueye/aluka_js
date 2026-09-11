//! CJS 模块系统最小集（M1）：`require` / `exports` / 循环依赖占位。
//!
//! 对齐 Go 版 CJS 包装约定（`internal/runtime/module/cjs.go`）：模块字节码的
//! `<main>` 返回 7 参数包装闭包
//! `(require, module, exports, __filename, __dirname, __import, __importMeta)`，
//! 宿主负责按位传参调用。加载流程：
//!
//! 1. `require(spec)` 解析 `spec.bc`（相对基准目录；`.js` 视为 `.bc`）；
//! 2. 读文件 → 反序列化 → Verifier 校验 → 执行 `<main>` 取模块闭包；
//! 3. 预建 `exports` / `module` 对象并**先登记缓存再执行**（循环依赖的经典
//!    CJS 行为：后加载方拿到未完成的 `exports`）；
//! 4. 以 7 参调用模块闭包，返回 `module.exports`（允许模块重赋值）。
//!
//! 字节码分发约定（M1）：`require("./dep")` 解析为基准目录下的 `dep.bc`；
//! `require("./dep.js")` 同样解析 `dep.bc`（`.js` → `.bc` 替换）。嵌套相对
//! 路径以入口目录为基准（子目录递归 require 的按模块目录解析留 M2）。

use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_bytecode::BytecodeModule;
use aluka_core::ObjectRef;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// CJS 模块作用域记录：与函数表 append 平行（`fn_start..fn_start+fn_count`
/// 区间归属本模块）。注入名（`exports`/`module`/`require`/`__filename`/
/// `__dirname`）按模块隔离存储，模块 main 与其子函数经「函数 → 模块」归属
/// 命中各自作用域。
///
/// 不能继续用共享全局表：模块 main 结束后加载器若恢复全局，本模块子函数
/// 稍后执行 `LOAD_GLOBAL exports` 会读到**当时活跃模块**的注入值（M2.4
/// 实测：debug.js 的 selectColor 在 finalhandler 加载期间被调用，读到
/// finalhandler 的 exports={} → `exports.colors` undefined 崩溃）。
#[derive(Default, Clone)]
pub(crate) struct ModuleScopeRecord {
    /// 本模块函数模板区间起点（module_functions 全局表索引）
    pub(crate) fn_start: u32,
    /// 本模块函数模板数
    pub(crate) fn_count: u32,
    /// 注入名 → 值（模块加载时写入，main 的 STORE_GLOBAL 亦落此处）
    pub(crate) vars: HashMap<String, Value>,
}

/// CJS 注入名集合：LOAD_GLOBAL/STORE_GLOBAL 命中这些名字时按函数归属
/// 路由到模块作用域（其余名字仍走共享全局表）。
pub(crate) const CJS_INJECTED_NAMES: [&str; 5] =
    ["exports", "module", "require", "__filename", "__dirname"];

impl Vm {
    /// 开启 CJS 模块上下文：注入 `require` 原生函数并记录入口基准目录。
    ///
    /// 之后 [`Vm::run_module`] 遇到「入口函数返回闭包」时按 7 参 CJS 签名
    /// 调用；未调用本方法时保持既有行为（无参调用，golden 语料零回归）。
    pub fn setup_cjs(&mut self, entry_path: &Path) {
        let base = entry_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        self.base_dir = Some(base.clone());
        self.entry_file = entry_path.display().to_string();
        let require = self.alloc_native_fn("require");
        self.require_fn = Some(require);
        // 内联入口形态的全局绑定（module.exports 重赋值 / exports 挂载）
        let exports = Value::Object(self.alloc_ordinary());
        let module_obj = Value::Object(self.alloc_ordinary());
        let _ = self.set_property(module_obj, "exports", exports);
        let filename = Value::Object(self.alloc_string(entry_path.display().to_string()));
        let dirname = Value::Object(self.alloc_string(base.clone().display().to_string()));
        self.set_global("exports", exports);
        self.set_global("module", module_obj);
        self.set_global("__filename", filename);
        self.set_global("__dirname", dirname);
    }

    /// 内置模块名 → 模块对象（`node:` 前缀剥离；M2：`fs`/`path`/`os`）。
    fn builtin_module(&self, name: &str) -> Option<Value> {
        let name = name.strip_prefix("node:").unwrap_or(name);
        if let Some(m) = self.builtin_registry.module(name) {
            return Some(Value::Object(m));
        }
        match name {
            "fs" => self.fs_object.map(Value::Object),
            "path" => self.path_module.map(Value::Object),
            "os" => self.os_module.map(Value::Object),
            "stream" => self.stream_module.map(Value::Object),
            "events" => self.events_module.map(Value::Object),
            _ => None,
        }
    }

    /// `require(specifier)`：解析、加载（带缓存与循环依赖占位）并返回 exports。
    pub(crate) fn call_require(&mut self, specifier: Value) -> Result<Value, VmError> {
        let spec = self.format_value(specifier);
        // 内置模块优先（fs/path/process 等不经文件系统）
        if let Some(m) = self.builtin_module(&spec) {
            // worker 进程内首次 `require('cluster')`：按 Node `_setupWorker` 语义
            // 挂接 process ↔ cluster.worker 桥接（详见
            // `builtins::cluster::on_cluster_required`）。
            crate::builtins::cluster::on_cluster_required(
                self,
                spec.strip_prefix("node:").unwrap_or(&spec),
            );
            return Ok(m);
        }
        let resolved = self
            .resolve_module(&spec)
            .ok_or_else(|| self.module_not_found(&spec))?;
        if std::env::var("ALUKA_REQ_DEBUG").is_ok() {
            eprintln!("[req-debug] spec={spec:?} -> {:?}", resolved);
        }
        let key = resolved.display().to_string();
        if let Some(cached) = self.module_exports.get(&key) {
            return Ok(*cached);
        }

        // .json 模块：原文读入 → JSON.parse（exports 即解析后的对象；
        // build 镜像对 .json 原样拷贝，不经字节码编译）
        if resolved.extension().and_then(|e| e.to_str()) == Some("json") {
            let text = std::fs::read_to_string(&resolved).map_err(|e| {
                let msg = self.alloc_string(format!("Cannot read module '{spec}': {e}"));
                VmError::Thrown(Value::Object(msg))
            })?;
            let s_val = Value::Object(self.alloc_string(text));
            let parsed = self.json_parse(&[s_val])?;
            self.module_exports.insert(key, parsed);
            return Ok(parsed);
        }

        // 读文件 → 反序列化 → 校验
        let data = std::fs::read(&resolved).map_err(|e| {
            let msg = self.alloc_string(format!("Cannot read module '{spec}': {e}"));
            VmError::Thrown(Value::Object(msg))
        })?;
        let module = BytecodeModule::deserialize(&data).map_err(|e| {
            let msg = self.alloc_string(format!("module '{spec}' deserialize: {e}"));
            VmError::Thrown(Value::Object(msg))
        })?;
        module.verify().map_err(|e| {
            let msg = self.alloc_string(format!("module '{spec}' verify: {e}"));
            VmError::Thrown(Value::Object(msg))
        })?;

        // 预建模块上下文（exports 先进缓存：循环依赖方拿到未完成 exports）
        let exports = Value::Object(self.alloc_ordinary());
        let module_obj = Value::Object(self.alloc_ordinary());
        // 加载期间钉扎 module 对象（其 exports 指针在收尾读取时仍需有效）
        if let Value::Object(r) = module_obj {
            self.gc_pinned.push(r.0);
        }
        self.set_property(module_obj, "exports", exports)?;
        self.module_exports.insert(key.clone(), exports);

        // 函数表合并：依赖模块的函数/类模板追加进全局表，并重写其
        // TemplateIdx 索引（MakeClosure/MakeClass）。append-only 保证已
        // 加载模块的索引不变——依赖模块导出的闭包（func_idx 指向全局表）
        // 在表切换后依然有效，这是单表 VM 支持多模块的关键。
        let fn_base = self.module_functions.len() as u32;
        let class_base = self.module_classes.len() as u32;
        if std::env::var("ALUKA_REQ_DEBUG").is_ok() {
            eprintln!(
                "[req-dbg] append {resolved:?} fn_base={fn_base} tpl_count={}",
                module.functions.len()
            );
        }
        let mut funcs: Vec<aluka_bytecode::FuncTemplate> = module.functions.to_vec();
        let debug_rewrite = std::env::var("ALUKA_REQ_DEBUG").is_ok();
        for f in funcs.iter_mut() {
            for instr in f.code.iter_mut() {
                match instr.op {
                    aluka_bytecode::Op::MakeClosure => {
                        if debug_rewrite && instr.operand >= fn_base {
                            eprintln!(
                                "[req-dbg] rewrite double? module {} op {} already >= fn_base {}",
                                resolved.display(),
                                instr.operand,
                                fn_base
                            );
                        }
                        instr.operand += fn_base;
                    }
                    aluka_bytecode::Op::MakeClass => instr.operand += class_base,
                    _ => {}
                }
            }
        }
        let mut classes = module.classes.clone();
        for c in classes.iter_mut() {
            c.constructor_index += fn_base;
            for m in c.methods.iter_mut() {
                m.func_index += fn_base;
            }
        }
        // header_extras 与函数表平行对齐：先补默认到 fn_base，追加后不足
        // 再补默认（无 extras 的函数等价「无 arguments」默认头）
        while self.module_header_extras.len() < fn_base as usize {
            self.module_header_extras
                .push(aluka_bytecode::FuncHeaderExtras {
                    arguments_slot: -1,
                    no_arguments_object: true,
                    new_target_slot: -1,
                    inlinable: false,
                });
        }
        self.module_header_extras
            .extend(module.header_extras.iter().cloned());
        while self.module_header_extras.len() < fn_base as usize + funcs.len() {
            self.module_header_extras
                .push(aluka_bytecode::FuncHeaderExtras {
                    arguments_slot: -1,
                    no_arguments_object: true,
                    new_target_slot: -1,
                    inlinable: false,
                });
        }
        self.module_functions
            .extend(funcs.iter().cloned().map(std::rc::Rc::new));
        self.module_constants.extend(
            module
                .functions
                .iter()
                .map(|f| std::rc::Rc::new(f.constants.clone())),
        );
        self.module_classes.extend(classes);
        // 模块作用域登记：fn_base 区间归属本模块（函数 → 模块查表依据）
        self.module_scopes.push(crate::modules::ModuleScopeRecord {
            fn_start: fn_base,
            fn_count: module.functions.len() as u32,
            vars: std::collections::HashMap::new(),
        });

        // 模块级全局绑定 + 基准目录压栈。
        // CJS 模块存在两种编译形态，加载器都要支持：
        // - 内联形态（`alukac build` 的 CommonJs 产物）：模块体在 <main>
        //   内直接执行，`require/module/exports/__filename/__dirname` 经
        //   全局解析——必须在 run_func **之前**注入全局并压栈；
        // - 包装形态（ESM wrapper 等）：<main> 返回 7 参闭包，闭包调用
        //   期间同样依赖压栈后的基准目录。
        let module_dir = resolved
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let filename = Value::Object(self.alloc_string(resolved.display().to_string()));
        let dirname = Value::Object(self.alloc_string(module_dir.display().to_string()));
        // 模块专属 require 实例：闭包捕获语义（Node）——模块内函数延迟调用
        // require 时仍解析到本模块目录（解释器 Op::Call 经 require_bases 查表）
        let require_fn = self.alloc_native_fn("require");
        self.require_bases.insert(require_fn, module_dir.clone());
        let saved_globals = ["exports", "module", "__filename", "__dirname", "require"]
            .map(|k| (k, self.globals.get(k).copied()));
        // 注入值双写：模块作用域（模块内函数命中）+ 共享全局（入口/兜底）
        if let Some(scope) = self.module_scopes.last_mut() {
            scope.vars.insert("exports".to_owned(), exports);
            scope.vars.insert("module".to_owned(), module_obj);
            scope.vars.insert("__filename".to_owned(), filename);
            scope.vars.insert("__dirname".to_owned(), dirname);
            scope
                .vars
                .insert("require".to_owned(), Value::Object(require_fn));
        }
        self.set_global("exports", exports);
        self.set_global("module", module_obj);
        self.set_global("__filename", filename);
        self.set_global("__dirname", dirname);
        self.set_global("require", Value::Object(require_fn));
        self.require_base_stack.push(module_dir);

        let invoke_result = (|| -> Result<Value, VmError> {
            let main_idx = fn_base as usize;
            // 操作数栈隔离：嵌套模块体在共享栈上执行，收尾截断回基线，
            // 防止模块体完成值/残留泄漏污染外层调用帧的栈序（真实包
            // `module.exports = <表达式>` 为末语句时必现）
            let stack_base = self.stack.len();
            // main 帧函数归属：注入名（LOAD_GLOBAL/STORE_GLOBAL）按
            // current_func_idx 路由模块作用域，模块 main 必须以自身
            // 函数索引执行（run_func 不换该字段）
            let saved_cf = self.current_func_idx;
            self.current_func_idx = main_idx as i64;
            let closure = self.run_func(&self.module_functions[main_idx].clone());
            self.current_func_idx = saved_cf;
            self.stack.truncate(stack_base);
            let closure = closure?;

            let (func_idx, upvalues) = match closure {
                Value::Object(r) => match self.heap.get(r.0 as usize) {
                    Some(HeapObject::Closure {
                        func_idx, upvalues, ..
                    }) => (*func_idx, upvalues.clone()),
                    // 内联形态：模块体已在 <main> 执行完毕，读回 exports
                    _ => return self.get_property(module_obj, "exports"),
                },
                _ => return self.get_property(module_obj, "exports"),
            };

            // 包装形态：7 参 CJS 签名调用模块闭包（import.meta 按本模块
            // 的 filename/dirname 物化，M2.3）
            let import_meta = self.build_import_meta(
                resolved.display().to_string(),
                resolved
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from(".")),
            );
            let wrapper_ret = self.invoke_function(
                func_idx,
                Value::Undefined,
                &[
                    Value::Object(require_fn),
                    module_obj,
                    exports,
                    filename,
                    dirname,
                    Value::Undefined, // __import
                    Value::Object(import_meta),
                ],
                upvalues,
            )?;
            // 异步 wrapper（TLA / await import）：记录未完成 Promise，
            // 供 `__aluka_import__` 挂接依赖完成链（M2.2）
            self.last_entry_async_promise = if matches!(
                wrapper_ret,
                Value::Object(r)
                    if matches!(
                        self.heap.get(r.0 as usize),
                        Some(HeapObject::Promise { pending: true, .. })
                    )
            ) {
                Some(wrapper_ret)
            } else {
                None
            };
            self.get_property(module_obj, "exports")
        })();

        self.require_base_stack.pop();
        for (k, saved) in saved_globals {
            match saved {
                Some(v) => {
                    self.globals.insert(k.to_owned(), v);
                }
                None => {
                    self.globals.remove(k);
                }
            }
        }

        if let Err(err) = &invoke_result {
            if std::env::var("ALUKA_REQ_DEBUG").is_ok() {
                let detail = match err {
                    VmError::Thrown(exc) => {
                        let raw = self.format_value(*exc);
                        let name = self
                            .get_property(*exc, "name")
                            .ok()
                            .map(|v| self.format_value(v))
                            .unwrap_or_default();
                        let msg = self
                            .get_property(*exc, "message")
                            .ok()
                            .map(|v| self.format_value(v))
                            .unwrap_or_default();
                        format!("{name}: {msg} (raw={raw} last_pc={})", self.last_pc)
                    }
                    other => format!("{other:?}"),
                };
                eprintln!("[req-debug] module {resolved:?} 执行失败: {detail}");
            }
        }
        invoke_result?;
        if std::env::var("ALUKA_REQ_DEBUG").is_ok() {
            eprintln!(
                "[req-debug] module done fn_base={} name={:?}",
                fn_base, resolved
            );
        }
        self.unpin_module(&module_obj);
        let final_exports = self.get_property(module_obj, "exports")?;
        if std::env::var("ALUKA_REQ_DEBUG").is_ok() {
            let obj_handle = match module_obj {
                Value::Object(r) => Some(r.0),
                _ => None,
            };
            let is_free = obj_handle
                .map(|h| matches!(self.heap.get(h as usize), Some(HeapObject::Free)))
                .unwrap_or(false);
            let is_ord = obj_handle
                .map(|h| matches!(self.heap.get(h as usize), Some(HeapObject::Ordinary { .. })))
                .unwrap_or(false);
            eprintln!(
                "[req-debug] spec={spec:?} module_obj={module_obj:?} is_free={is_free} is_ord={is_ord} final={final_exports:?}"
            );
        }
        self.module_exports.insert(key.clone(), final_exports);
        // 同步缓存重绑定后的 module.exports（module.exports 重赋值语义）
        let _ = key;
        Ok(final_exports)
    }

    /// 解除 module 对象钉扎（call_require 收尾）。
    fn unpin_module(&mut self, module_obj: &Value) {
        if let Value::Object(r) = module_obj {
            self.gc_pinned.retain(|&h| h != r.0);
        }
    }

    /// 解析 `specifier` 为字节码文件路径。
    ///
    /// - 相对（`./` `../` `/`）：相对**当前模块目录**（require 基准栈顶，
    ///   空栈回退入口 base_dir）；
    /// - 裸包名（`express` / `express/lib/utils`）：从当前目录逐级向上找
    ///   `node_modules/`（Node 解析语义）；
    /// - 候选形态：`X.bc`、`X/index.bc`、`X/main.bc`；`.json` 按原文保留
    ///   （build 镜像原样拷贝，require 返回解析后的对象）。
    fn resolve_module(&self, specifier: &str) -> Option<PathBuf> {
        let base = self
            .require_base_stack
            .last()
            .cloned()
            .or_else(|| self.base_dir.clone())
            .unwrap_or_else(|| PathBuf::from("."));
        self.resolve_specifier_from(&base, specifier)
    }

    /// `__aluka_import__(source)` 加载器入口：同步完成返回 exports；
    /// 依赖模块为异步完成（TLA）时返回其完成 Promise（M2.2）。
    pub(crate) fn import_module_entry(&mut self, spec_val: Value) -> Result<Value, VmError> {
        self.last_entry_async_promise = None;
        let exports = self.call_require(spec_val)?;
        match self.last_entry_async_promise.take() {
            None => Ok(exports),
            Some(promise) => Ok(promise),
        }
    }

    /// `import.meta.resolve(specifier)` 的路径计算入口（纯解析，不改状态）。
    pub(crate) fn resolve_module_for_meta(&self, base: &str, specifier: &str) -> Option<String> {
        self.resolve_specifier_from(Path::new(base), specifier)
            .map(|p| p.with_extension("").to_string_lossy().to_string())
    }

    fn resolve_specifier_from(&self, base: &Path, specifier: &str) -> Option<PathBuf> {
        let is_relative = specifier.starts_with("./")
            || specifier.starts_with("../")
            || specifier.starts_with('/');
        if is_relative {
            let joined = normalize_path(&base.join(specifier));
            module_candidates(&joined)
        } else if specifier.starts_with('#') {
            // `#alias`：`imports` 内部子路径别名 —— 自当前包根向上找最近
            // 的 package.json，经 `imports` 条件映射解析（M2.1）
            for dir in base.ancestors() {
                let pkg = dir.join("package.json");
                if !pkg.is_file() {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&pkg) {
                    if let Some(parsed) = aluka_module::parse_json(&text) {
                        if let Some(imports) = parsed.get("imports") {
                            if let Some(target) = aluka_module::resolve_imports(
                                imports,
                                specifier,
                                aluka_module::ConditionKind::Require,
                            ) {
                                let joined = normalize_path(&dir.join(target));
                                if let Some(p) = module_candidates(&joined) {
                                    return Some(p);
                                }
                            }
                        }
                    }
                }
            }
            None
        } else {
            // 裸说明符：拆分（包名, 子路径）→ 包根定位 → `exports` 条件
            // 映射优先（M2.1），无 exports 时回退 main/index 既有链路
            let (name, subpath) = aluka_module::split_package_specifier(specifier);
            for dir in base.ancestors() {
                let node_modules = dir.join("node_modules");
                let pkg_root = node_modules.join(&name);
                if !pkg_root.is_dir() {
                    continue;
                }
                let pkg_json = pkg_root.join("package.json");
                if pkg_json.is_file() {
                    if let Ok(text) = std::fs::read_to_string(&pkg_json) {
                        if let Some(parsed) = aluka_module::parse_json(&text) {
                            if let Some(exports) = parsed.get("exports") {
                                if let Some(target) = aluka_module::resolve_exports(
                                    exports,
                                    &subpath,
                                    aluka_module::ConditionKind::Require,
                                ) {
                                    let joined = normalize_path(&pkg_root.join(target));
                                    if let Some(p) = module_candidates(&joined) {
                                        return Some(p);
                                    }
                                    // exports 明确拒绝或目标缺失：不回退
                                    // 旧链路（Node 语义：exports 是唯一入口面）
                                    continue;
                                }
                                // exports 存在但子路径被拒绝：跳过旧链路
                                if subpath != "." {
                                    continue;
                                }
                            }
                        }
                    }
                }
                let pkg_dir = if subpath == "." {
                    pkg_root.clone()
                } else {
                    normalize_path(&pkg_root.join(subpath.strip_prefix("./").unwrap_or(&subpath)))
                };
                if let Some(p) = module_candidates(&normalize_path(&pkg_dir)) {
                    return Some(p);
                }
            }
            None
        }
    }

    /// 物化 `import.meta` 对象：`url`（file:// 形态）/ `filename` /
    /// `dirname` / `resolve(specifier)`。
    pub(crate) fn build_import_meta(
        &mut self,
        filename: String,
        dir: std::path::PathBuf,
    ) -> ObjectRef {
        let meta = self.alloc_ordinary();
        let fwd = filename.replace('\\', "/");
        let url = self.alloc_string(format!("file:///{fwd}"));
        let _ = self.set_property(Value::Object(meta), "url", Value::Object(url));
        let fname = self.alloc_string(filename);
        let _ = self.set_property(Value::Object(meta), "filename", Value::Object(fname));
        let dirname = self.alloc_string(dir.to_string_lossy().to_string());
        let _ = self.set_property(Value::Object(meta), "dirname", Value::Object(dirname));
        // 命名空间标记：try_dispatch 形态二据此反查 importMeta.resolve
        let ns = self.alloc_string("importMeta".to_owned());
        let _ = self.set_property(Value::Object(meta), "_builtinNs", Value::Object(ns));
        let resolve_fn = self.alloc_native_fn("importMeta.resolve");
        let _ = self.set_property(Value::Object(meta), "resolve", Value::Object(resolve_fn));
        let dir_str = self.alloc_string(dir.to_string_lossy().to_string());
        self.set_native_fn_property(resolve_fn, "_metaDir", Value::Object(dir_str));
        meta
    }

    fn module_not_found(&mut self, spec: &str) -> VmError {
        let msg = self.alloc_string(format!("Cannot find module '{spec}'"));
        VmError::Thrown(Value::Object(msg))
    }
}

/// `require` 目标的字节码候选：`.json` 原样；`X.js/cjs/mjs` → `X.bc`；
/// 裸路径 → `X.bc` → `X/index.bc` → `X/main.bc`。
fn module_candidates(p: &Path) -> Option<PathBuf> {
    // 包目录（目录名可能含点，如 ipaddr.js/）：extension 分支会误把目录名
    // 当文件扩展名处理——目录一律走 index/main/package.json 解析
    if p.is_dir() {
        let index = p.join("index.bc");
        if index.is_file() {
            return Some(index);
        }
        let main = p.join("main.bc");
        if main.is_file() {
            return Some(main);
        }
        let pkg = p.join("package.json");
        if pkg.is_file() {
            if let Ok(text) = std::fs::read_to_string(&pkg) {
                if let Some(main_field) = extract_json_string_field(&text, "main") {
                    let main_path = normalize_path(&p.join(main_field.trim()));
                    return module_candidates(&main_path);
                }
            }
        }
        return None;
    }
    match p.extension().and_then(|e| e.to_str()) {
        Some("json") => p.is_file().then(|| p.to_path_buf()),
        Some("js" | "cjs" | "mjs" | "mts" | "ts") => {
            let bc = p.with_extension("bc");
            bc.is_file().then_some(bc)
        }
        _ => {
            // 1) 原样路径追加 `.bc`：spec 无扩展名但文件名含点（如
            // `require('./util.inspect')` —— `with_extension` 会把
            // "inspect" 误当扩展名替换成 util.bc）→ util.inspect.bc
            let direct_bc = PathBuf::from(format!("{}.bc", p.display()));
            if direct_bc.is_file() {
                return Some(direct_bc);
            }
            let bc = p.with_extension("bc");
            if bc.is_file() {
                return Some(bc);
            }
            let index = p.join("index.bc");
            if index.is_file() {
                return Some(index);
            }
            let main = p.join("main.bc");
            if main.is_file() {
                return Some(main);
            }
            // 包根：package.json "main" → 对应 .bc（如 debug → src/index.bc）
            let pkg = p.join("package.json");
            if pkg.is_file() {
                if let Ok(text) = std::fs::read_to_string(&pkg) {
                    if let Some(main_field) = extract_json_string_field(&text, "main") {
                        let main_path = normalize_path(&p.join(main_field.trim()));
                        let mut main_bc = main_path.clone();
                        if main_bc.extension().is_some() {
                            main_bc.set_extension("bc");
                        } else {
                            main_bc = main_path.join("index.bc");
                        }
                        if main_bc.is_file() {
                            return Some(main_bc);
                        }
                    }
                }
            }
            None
        }
    }
}

/// 从 JSON 文本提取顶层字符串字段（轻量扫描；与 alukac build 侧同款）。
fn extract_json_string_field(text: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    let quote = after.chars().next()?;
    if quote != '"' {
        return None;
    }
    let end = after[1..].find('"')?;
    Some(after[1..1 + end].to_owned())
}

/// 词法规范化路径（折叠 `.`/`..`，不触碰文件系统；Windows 分隔符保留）。
fn normalize_path(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
