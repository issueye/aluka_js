//! 对象属性读写、访问器触发、原型链遍历与 Instanceof 语义。

use crate::heap::{HeapObject, OrdinaryProps};
use crate::interpreter::{Vm, VmError};
use crate::jit_helpers::{from_vm_value, to_vm_value};
use crate::ops::to_number;
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;

/// flags 规范序（JS canonical order 子集）。
fn canonical_regexp_flags(flags: &str) -> String {
    const ORDER: [char; 7] = ['d', 'g', 'i', 'm', 's', 'u', 'y'];
    ORDER.iter().filter(|c| flags.contains(**c)).collect()
}

impl Vm {
    /// 读取 Ordinary 对象自有属性值（未删除；快速/字典两模式统一入口）。
    ///
    /// 快速模式：隐藏类命中 → O(1) 槽位直读；字典模式：HashMap 查值。
    /// 删除集先于存储判定（删除不改 shape）。
    pub(crate) fn own_value(&self, idx: usize, key: &str) -> Option<Value> {
        let HeapObject::Ordinary { props, deleted, .. } = self.heap.get(idx)? else {
            return None;
        };
        if deleted.contains(key) {
            return None;
        }
        match props {
            OrdinaryProps::Shape { shape, slots } => {
                let slot = self.shape_table.shape(*shape)?.lookup(key)?;
                slots.get(slot).map(|&b| to_vm_value(b))
            }
            OrdinaryProps::Dict { properties, index } => index
                .get(key)
                .and_then(|&s| properties.get(s))
                .filter(|(k, _)| k == key)
                .map(|(_, v)| *v),
        }
    }

    /// Ordinary 对象是否含自有属性（未删除）。
    pub(crate) fn has_own_slot(&self, idx: usize, key: &str) -> bool {
        self.own_value(idx, key).is_some()
    }

    /// Ordinary 对象属性表内忽略 ASCII 大小写扫描，返回实际键名。
    ///
    /// Windows `process.env` 语义专用：Node 22 在 Windows 上对 env 键的
    /// 查找/写入均大小写不敏感，但键保持环境块的原始大小写形态
    /// （`Object.keys(process.env)` 仍返回 `Path` 等原始键）。
    fn env_find_key(&self, idx: usize, key: &str) -> Option<String> {
        match self.heap.get(idx) {
            Some(HeapObject::Ordinary { props, .. }) => match props {
                OrdinaryProps::Dict { properties, .. } => properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(k, _)| k.clone()),
                OrdinaryProps::Shape { shape, .. } => self
                    .shape_table
                    .shape(*shape)?
                    .names()
                    .find(|n| n.eq_ignore_ascii_case(key))
                    .map(str::to_owned),
            },
            _ => None,
        }
    }

    /// 枚举 Ordinary 对象自有属性（键 + 值，快速/字典两模式均保插入序；
    /// 跳过删除项；访问器键并入，值取访问器函数）。
    pub(crate) fn own_entries(&self, idx: usize) -> Vec<(String, Value)> {
        let Some(HeapObject::Ordinary {
            props,
            deleted,
            non_enum,
            getters,
            setters,
            ..
        }) = self.heap.get(idx)
        else {
            return Vec::new();
        };
        let mut out = match props {
            OrdinaryProps::Shape { shape, slots } => {
                let Some(s) = self.shape_table.shape(*shape) else {
                    return Vec::new();
                };
                let mut out = Vec::with_capacity(s.len());
                for (i, name) in s.names().enumerate() {
                    if deleted.contains(name) || non_enum.contains(name) {
                        continue;
                    }
                    out.push((
                        name.to_owned(),
                        slots
                            .get(i)
                            .map(|&b| to_vm_value(b))
                            .unwrap_or(Value::Undefined),
                    ));
                }
                out
            }
            OrdinaryProps::Dict { properties, .. } => properties
                .iter()
                .filter(|(k, _)| !deleted.contains(k) && !non_enum.contains(k))
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
        };
        // 访问器键并入（Object.keys/entries 应包含可枚举访问器属性；
        // 值取访问器函数值——parser 类惰性 getter 的求值结果即该函数）
        for (k, g) in getters.iter() {
            if !non_enum.contains(k) && !out.iter().any(|(k2, _)| k2 == k) {
                out.push((k.clone(), *g));
            }
        }
        // 纯 setter 键亦并入（值以 undefined 占位；getter 键已由上一循环并入）
        for k in setters.keys() {
            if !non_enum.contains(k) && !out.iter().any(|(k2, _)| k2 == k) {
                out.push((k.clone(), Value::Undefined));
            }
        }
        out
    }
    /// 克隆专用自有属性枚举：`(键, Some(数据值))` = 数据属性；`(键, None)`
    /// = 访问器（或纯 setter）键——调用方须按 **`Get`** 求值（触发 getter）。
    ///
    /// 与 [`Vm::own_entries`] 的差异**仅在访问器键的取值面**：`own_entries`
    /// 返回访问器函数值（服务于 `Object.keys` / `JSON.stringify` 等场景，
    /// 那些场景**绝不能**触发 getter），故其行为保持不变。键序沿用
    /// `own_entries`（数据属性按存储序在前，访问器键随后按键名升序——访问器
    /// 表为 HashMap，无插入序可依，升序至少保证跨进程确定性）。
    pub(crate) fn own_clone_entries(&self, idx: usize) -> Vec<(String, Option<Value>)> {
        let mut out: Vec<(String, Option<Value>)> = self
            .own_entries(idx)
            .into_iter()
            .map(|(k, v)| (k, Some(v)))
            .collect();
        let Some(HeapObject::Ordinary {
            getters,
            setters,
            non_enum,
            ..
        }) = self.heap.get(idx)
        else {
            return out;
        };
        // 访问器键改判（访问器优先于同名数据槽：`defineProperty` 覆盖数据属性
        // 为访问器时数据槽仍在 `props` 里，读取面由 getters 表优先）
        let mut keys: Vec<&String> = getters
            .keys()
            .chain(setters.keys())
            .filter(|k| !non_enum.contains(k.as_str()))
            .collect();
        keys.sort();
        keys.dedup();
        for k in keys {
            match out.iter_mut().find(|(k2, _)| k2 == k) {
                Some(e) => e.1 = None,
                None => out.push((k.clone(), None)),
            }
        }
        out
    }

    /// 把自有属性登记为**不可枚举**（只影响 `Object.keys` / `JSON.stringify`
    /// 等枚举面）。
    ///
    /// 克隆重建 Error 实例专用：Node 的克隆体 `Object.keys` / `JSON.stringify`
    /// 面为空集，而本运行时 Error 的 `message`/`name` 由
    /// [`Vm::alloc_error_instance`] 经普通数据路径建立（默认可枚举）。键不
    /// 存在时无副作用（调用方保证先建属性）。
    pub(crate) fn mark_non_enumerable(&mut self, obj: Value, key: &str) {
        let ValueCase::Object(r) = obj.case() else {
            return;
        };
        if let Some(HeapObject::Ordinary { non_enum, .. }) = self.heap.get_mut(r.0 as usize) {
            non_enum.insert(key.to_owned());
        }
    }

    /// 删除 Ordinary 对象的自有属性。
    ///
    /// **V8 键序语义**：`delete` 使快速属性对象**慢化为字典模式**（与 V8 一致
    /// ——快属性删除会让隐藏类失效），保证「删除后重加」的键落在键序**末尾**
    /// （`Object.keys` / `JSON.stringify` 顺序对齐 Node 22）。字典模式直接
    /// 移除实体。非 Ordinary 或无该属性时为无操作。Proxy 经 trap 派发。
    pub(crate) fn delete_property(&mut self, obj: Value, key: &str) {
        // Proxy 对象：经 deleteProperty trap 派发（假值抛 TypeError 由 trap 层处理）
        if let Some(r) = obj.as_object() {
            if self.proxy_parts(r).is_some() {
                let _ = self.proxy_delete(r, key);
                return;
            }
        }
        // 数组对象：索引键 → 元素置 undefined（本表示无空洞，读面与 Node
        // 一致——length 不变、读到 undefined；`idx in arr` 恒真为已知近似）；
        // 非索引键 → 删自有属性表
        if let Some(r) = obj.as_object() {
            if let Some(HeapObject::Array {
                elements,
                properties,
                ..
            }) = self.heap.get_mut(r.0 as usize)
            {
                match key.parse::<usize>() {
                    Ok(i) if i < elements.len() => {
                        elements[i] = Value::Undefined;
                    }
                    _ => {
                        properties.remove(key);
                    }
                }
                return;
            }
        }
        if let Some(r) = obj.as_object() {
            if let Some(HeapObject::Ordinary {
                props,
                deleted,
                deleted_gen,
                ..
            }) = self.heap.get_mut(r.0 as usize)
            {
                match props {
                    OrdinaryProps::Shape { shape, slots } => {
                        let hit = self
                            .shape_table
                            .shape(*shape)
                            .and_then(|s| s.lookup(key))
                            .is_some_and(|slot| slot < slots.len());
                        if hit {
                            // 命中 → 整体迁移字典模式（跳过本键与既有删除键；
                            // 槽位值保插入序；重加键将由 set_property append）
                            let slot_vals: Vec<Value> =
                                slots.iter().map(|&b| to_vm_value(b)).collect();
                            let names: Vec<String> = self
                                .shape_table
                                .shape(*shape)
                                .map(|s| s.names().map(str::to_owned).collect())
                                .unwrap_or_default();
                            let mut properties: Vec<(String, Value)> =
                                Vec::with_capacity(names.len());
                            for (i, name) in names.iter().enumerate() {
                                if name == key || deleted.contains(name) {
                                    continue;
                                }
                                properties.push((
                                    name.clone(),
                                    slot_vals.get(i).copied().unwrap_or(Value::Undefined),
                                ));
                            }
                            let index = properties
                                .iter()
                                .enumerate()
                                .map(|(i, (k, _))| (k.clone(), i))
                                .collect();
                            *props = OrdinaryProps::Dict { properties, index };
                        }
                    }
                    OrdinaryProps::Dict { properties, index } => {
                        properties.retain(|(k, _)| k != key);
                        // retain 后槽位整体前移：重建键 → 槽位索引（删除稀少，
                        // O(n) 重建可接受；保持与列表严格同步）。
                        index.clear();
                        for (i, (k, _)) in properties.iter().enumerate() {
                            index.insert(k.clone(), i);
                        }
                    }
                }
                deleted.insert(key.to_owned());
                *deleted_gen += 1;
            }
        }
    }

    /// 获取常量池字符串。
    pub fn get_const_string(&self, idx: usize) -> Result<String, VmError> {
        Ok(self
            .const_string(idx)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{idx}")))
    }

    /// 借用当前常量池中的字符串，供解释循环热路径使用。
    ///
    /// 与公共兼容 API `get_const_string` 不同，这个方法不复制字符串；返回的
    /// 借用只应在下一次可变 VM 操作前使用。非法/非字符串常量返回 `None`，
    /// 由调用点按原有格式化回退处理。
    pub(crate) fn const_string(&self, idx: usize) -> Option<&str> {
        match self.current_constants.get(idx) {
            Some(aluka_bytecode::Constant::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// 将任意值转换为属性键。
    pub fn to_property_key(&self, val: Value) -> String {
        match val.case() {
            ValueCase::Number(n) => {
                if n.fract() == 0.0 {
                    format!("{}", n as i64)
                } else {
                    format!("{n}")
                }
            }
            ValueCase::Boolean(b) => format!("{b}"),
            ValueCase::Null => "null".to_owned(),
            ValueCase::Undefined => "undefined".to_owned(),
            ValueCase::Object(r) => {
                let idx = r.0 as usize;
                if idx < self.heap.len() {
                    match &self.heap[idx] {
                        HeapObject::String(s) => return s.clone(),
                        HeapObject::BigInt(s) => return s.clone(),
                        HeapObject::Symbol { .. } => return crate::symbol::mangled_key(r),
                        _ => {}
                    }
                }
                if let Some(aluka_bytecode::Constant::String(s)) = self.current_constants.get(idx) {
                    s.clone()
                } else {
                    format!("[Object {:?}]", r)
                }
            }
        }
    }

    /// 读取属性（含原型链查找、getter 触发与数组元素读取）。
    /// 属性读取（调试包装：ALUKA_GETPROP_DEBUG 时打印 key 与结果）。
    pub fn get_property(&mut self, obj: Value, key: &str) -> Result<Value, VmError> {
        let __dbg = std::env::var("ALUKA_GETPROP_DEBUG").is_ok();
        let r = self.get_property_inner(obj, key);
        if __dbg {
            let v = match &r {
                Ok(v) => format!("{v:?}"),
                Err(e) => format!("Err {e:?}"),
            };
            eprintln!("[getprop] key={key} -> {v}");
        }
        r
    }

    pub(crate) fn get_property_inner(&mut self, obj: Value, key: &str) -> Result<Value, VmError> {
        // Proxy 对象：经 get trap 派发（含 revoked 校验与 target 回退）
        if let Some(r) = obj.as_object() {
            if self.proxy_parts(r).is_some() {
                return self.proxy_get(r, key, obj);
            }
        }
        // globalThis：属性读取直通全局变量表与内建全局
        if let Some(r) = obj.as_object() {
            if self.has_own_slot(r.0 as usize, "_isGlobalThis") {
                return Ok(self.resolve_global(key).unwrap_or(Value::Undefined));
            }
        }
        // 流实例计算属性（writableLength/writableNeedDrain/destroyed 等；
        // 状态存 STREAM_STORE，M3.1 背压状态机）
        if matches!(
            key,
            "writableLength"
                | "writableNeedDrain"
                | "writableHighWaterMark"
                | "readableLength"
                | "readableHighWaterMark"
                | "destroyed"
                | "errored"
                | "flowing"
        ) && let ValueCase::Object(r) = obj.case()
            && self.has_own_slot(r.0 as usize, "_isStream")
            && let Some(v) = crate::builtins::stream::stream_computed_prop(r.0, key)
        {
            return Ok(v);
        }
        // 内置对象的方法按需物化（process.nextTick 等属性访问先于调用）
        if key == "env" && self.process_object.is_some_and(|p| obj == Value::Object(p)) {
            // process.env：对象单例缓存（Node 语义：process.env === process.env 恒等）
            if let Some(env_id) = self.env_object {
                return Ok(Value::Object(env_id));
            }
            let env_obj = self.alloc_ordinary();
            for (k, v) in std::env::vars() {
                let s_ref = self.alloc_string(v);
                let _ = self.set_property(Value::Object(env_obj), &k, Value::Object(s_ref));
            }
            self.env_object = Some(env_obj);
            return Ok(Value::Object(env_obj));
        }
        if key == "nextTick" && self.process_object.is_some_and(|p| obj == Value::Object(p)) {
            return Ok(Value::Object(self.alloc_native_fn("nextTick")));
        }
        if key == "exit" && self.process_object.is_some_and(|p| obj == Value::Object(p)) {
            // process.exit(code)：立即终止（Node 语义；handler 抛 VmError::Exit）
            return Ok(Value::Object(self.alloc_native_fn("process.exit")));
        }
        // process.stderr/stdout：流面（isTTY 假 + write 落 stderr/stdout；
        // depd 的 log 读 isTTY 决定彩色、write 输出弃用消息）
        if matches!(key, "stderr" | "stdout")
            && self.process_object.is_some_and(|p| obj == Value::Object(p))
        {
            let stream = self.alloc_ordinary();
            let _ = self.set_property(Value::Object(stream), "isTTY", Value::Boolean(false));
            let _ = self.set_property(Value::Object(stream), "isatty", Value::Boolean(false));
            let write_fn = if key == "stderr" {
                self.alloc_native_fn("process.stderr.write")
            } else {
                self.alloc_native_fn("process.stdout.write")
            };
            let _ = self.set_property(Value::Object(stream), "write", Value::Object(write_fn));
            return Ok(Value::Object(stream));
        }
        // Symbol 构造器的知名符号物化（Symbol.iterator 等属性读取；
        // 构造器为 NativeCtor 单例——NativeFn 与 NativeCtor 名都认）
        if crate::symbol::WELL_KNOWN_NAMES.contains(&key) {
            let is_symbol_ctor = matches!(obj.case(), ValueCase::Object(r)
                    if matches!(
                        self.heap.get(r.0 as usize),
                        Some(HeapObject::NativeFn { name, .. })
                            | Some(HeapObject::NativeCtor { name, .. })
                            if name == "Symbol"
                    )
            );
            if is_symbol_ctor {
                return Ok(self.well_known_symbol(key));
            }
        }
        // 闭包函数：`name` / `length` 读模板元数据（Go 前端编译产物携带函数名）。
        // 先判断键再取模板：普通属性（尤其热路径中的 `prototype`/自定义键）
        // 不需要复制函数名 String。
        if let Some(r) = obj.as_object() {
            let func_idx = match self.heap.get(r.0 as usize) {
                Some(HeapObject::Closure { func_idx, .. }) => Some(*func_idx),
                _ => None,
            };
            if key == "length" {
                if let Some(num_params) = func_idx
                    .and_then(|idx| self.module_functions.get(idx))
                    .map(|t| t.num_params)
                {
                    return Ok(Value::Number(num_params as f64));
                }
            } else if key == "name" {
                if let Some(name) = func_idx
                    .and_then(|idx| self.module_functions.get(idx))
                    .map(|t| t.name.clone())
                {
                    return Ok(Value::Object(self.alloc_string(name)));
                }
            }
        }
        // 字符串接收者：`length` 与数字下标访问（原型方法由 CALL_METHOD 链求值）
        if let Some(r) = obj.as_object() {
            let str_len = match self.heap.get(r.0 as usize) {
                Some(HeapObject::String(text)) => Some(text.chars().count()),
                _ => None,
            };
            if let Some(len) = str_len {
                if key == "length" {
                    return Ok(Value::Number(len as f64));
                }
                if let Ok(i) = key.parse::<usize>() {
                    if i < len {
                        if let Some(HeapObject::String(text)) = self.heap.get(r.0 as usize) {
                            let ch = text
                                .chars()
                                .nth(i)
                                .map(|c| c.to_string())
                                .unwrap_or_default();
                            return Ok(Value::Object(self.alloc_string(ch)));
                        }
                    }
                    return Ok(Value::Undefined);
                }
                // 知名符号键（如 Symbol.iterator）→ 转发字符串原型面同键属性
                if crate::symbol::is_symbol_key(key) {
                    let str_proto = crate::builtins::surface::str_proto(self);
                    if let Some(v) = self.own_value(str_proto.0 as usize, key) {
                        return Ok(v);
                    }
                }
            }
        }
        // process.env 单例：Windows 下键查找大小写不敏感（Node 22 实测对齐——
        // `process.env.PATH`/`Path`/`path` 等效命中；键保持原始大小写形态）
        if let Some(env_id) = self.env_object {
            if obj == Value::Object(env_id) && self.own_value(env_id.0 as usize, key).is_none() {
                if let Some(actual) = self.env_find_key(env_id.0 as usize, key) {
                    // 递归一次：以实际键名走精确路径（字面不同必然精确命中）
                    return self.get_property(obj, &actual);
                }
            }
        }
        let mut cur = obj;
        let mut depth = 0;
        while let ValueCase::Object(r) = cur.case() {
            if depth > 100 {
                break;
            }
            depth += 1;
            let idx = r.0 as usize;
            if idx >= self.heap.len() {
                break;
            }
            match &self.heap[idx] {
                HeapObject::Ordinary { getters, proto, .. } => {
                    if let Some(g_val) = getters.get(key) {
                        return self.invoke_accessor(*g_val, obj, &[]);
                    }
                    if let Some(v) = self.own_value(idx, key) {
                        return Ok(v);
                    }
                    if let Some(parent) = *proto {
                        cur = Value::Object(parent);
                    } else {
                        break;
                    }
                }
                HeapObject::Closure {
                    properties,
                    getters,
                    proto,
                    ..
                } => {
                    if let Some(g_val) = getters.get(key) {
                        return self.invoke_accessor(*g_val, obj, &[]);
                    }
                    if let Some(v) = properties.get(key) {
                        return Ok(*v);
                    }
                    if let Some(parent) = *proto {
                        cur = Value::Object(parent);
                    } else if matches!(key, "bind" | "call" | "apply" | "toString") {
                        // 函数方法面按需物化（闭包以 None 原型登记；属性读取面
                        // 对齐 JS 的 Function.prototype——真实包 `fn.bind` /
                        // `fn.call` 属性读取依赖此合成，调用经注册表 handler）
                        let f = self.alloc_native_fn(match key {
                            "bind" => "Function.prototype.bind",
                            "call" => "Function.prototype.call",
                            "apply" => "Function.prototype.apply",
                            _ => "Function.prototype.toString",
                        });
                        return Ok(Value::Object(f));
                    } else {
                        break;
                    }
                }
                HeapObject::NativeCtor { properties, .. } => {
                    if let Some(v) = properties.get(key) {
                        return Ok(*v);
                    }
                    break;
                }
                HeapObject::NativeFn { properties, .. } => {
                    if let Some(v) = properties.get(key) {
                        return Ok(*v);
                    }
                    break;
                }
                HeapObject::Array {
                    elements,
                    properties,
                    proto,
                } => {
                    if key == "length" {
                        return Ok(Value::Number(elements.len() as f64));
                    }
                    if let Ok(i) = key.parse::<usize>() {
                        if let Some(v) = elements.get(i) {
                            return Ok(*v);
                        }
                        // 界外索引键：落入自有属性表（写路径把超大下标存此处）
                    }
                    if let Some(v) = properties.get(key) {
                        return Ok(*v);
                    }
                    if let Some(parent) = *proto {
                        cur = Value::Object(parent);
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        // RegExp 实例表面（source/flags 系/lastIndex/constructor）：堆对象
        // 不携带属性表，按需合成（`lastIndex` 存线程局部状态表）
        if let Some(r) = obj.as_object() {
            if let Some(HeapObject::RegExp { pattern, flags }) = self.heap.get(r.0 as usize) {
                let (pattern, flags) = (pattern.clone(), flags.clone());
                let ctor = self.regexp_ctor;
                let synthesized = match key {
                    "source" => Some(Value::Object(self.alloc_string(if pattern.is_empty() {
                        "(?:)".to_owned()
                    } else {
                        pattern
                    }))),
                    "flags" => Some(Value::Object(
                        self.alloc_string(canonical_regexp_flags(&flags)),
                    )),
                    "global" => Some(Value::Boolean(flags.contains('g'))),
                    "ignoreCase" => Some(Value::Boolean(flags.contains('i'))),
                    "multiline" => Some(Value::Boolean(flags.contains('m'))),
                    "dotAll" => Some(Value::Boolean(flags.contains('s'))),
                    "sticky" => Some(Value::Boolean(flags.contains('y'))),
                    "lastIndex" => Some(Value::Number(
                        crate::interpreter::regex_last_index(r.0) as f64
                    )),
                    "constructor" => ctor.map(Value::Object),
                    _ => None,
                };
                if let Some(v) = synthesized {
                    return Ok(v);
                }
                // 未知键沿 RegExp.prototype 原型链（方法面：test/exec/toString/
                // compile——真实包 `re.test` 属性读取与 `RegExp.prototype.test`
                // 存槽依赖原型链）
                let mut chain: Option<ObjectRef> = self.regexp_prototype;
                for _ in 0..8 {
                    let Some(pr) = chain.take() else { break };
                    let (names, slots, proto_next): (
                        Option<Vec<String>>,
                        Option<Vec<Value>>,
                        Option<ObjectRef>,
                    ) = match self.heap.get(pr.0 as usize) {
                        Some(HeapObject::Ordinary {
                            props: OrdinaryProps::Shape { shape, slots },
                            proto,
                            ..
                        }) => {
                            let names = self
                                .shape_table
                                .shape(*shape)
                                .map(|sh| sh.names().map(str::to_owned).collect::<Vec<_>>());
                            let vals = slots.iter().map(|&b| to_vm_value(b)).collect::<Vec<_>>();
                            (names, Some(vals), *proto)
                        }
                        Some(HeapObject::Ordinary {
                            props: OrdinaryProps::Dict { properties, .. },
                            proto,
                            ..
                        }) => {
                            let names = Some(
                                properties
                                    .iter()
                                    .map(|(k, _)| k.clone())
                                    .collect::<Vec<_>>(),
                            );
                            let vals =
                                Some(properties.iter().map(|(_, v)| *v).collect::<Vec<Value>>());
                            (names, vals, *proto)
                        }
                        _ => (None, None, None),
                    };
                    if let (Some(ns), Some(sl)) = (names, slots) {
                        if let Some(pos) = ns.iter().position(|n| *n == key) {
                            if let Some(v) = sl.get(pos) {
                                return Ok(*v);
                            }
                        }
                    }
                    chain = proto_next;
                }
            }
        }
        // Map/Set 实例的合成属性面：size（entries 数）+ 知名符号属性
        // （如 Symbol.iterator——方法挂 container_proto 面，Map 变体无原型
        // 链字段，属性读取在此按需转发；普通方法 keys/values 等由
        // CALL_METHOD 特判处理，不依赖属性存在）
        if let Some(r) = obj.as_object() {
            if let Some(HeapObject::Map { entries }) = self.heap.get(r.0 as usize) {
                if key == "size" {
                    return Ok(Value::Number(entries.len() as f64));
                }
                // `constructor`：Map/Set 共用 container_proto，无法在原型上区分，
                // 故按实例登记在此合成（Node：`new Map().constructor === Map`）
                if key == "constructor" {
                    let is_set = self.is_set_instance(obj);
                    let ctor = if is_set { self.set_ctor } else { self.map_ctor };
                    if let Some(c) = ctor {
                        return Ok(Value::Object(c));
                    }
                }
                // 知名符号键 → 读容器原型面的同键属性
                if crate::symbol::is_symbol_key(key) {
                    let cont_proto = crate::builtins::surface::container_proto(self);
                    if let Some(v) = self.own_value(cont_proto.0 as usize, key) {
                        return Ok(v);
                    }
                }
            }
        }
        // TypedArray / DataView / ArrayBuffer 实例表面（length/buffer 等按需合成）
        if let Some(r) = obj.as_object() {
            // 先快照堆字段（避免可变借用与堆读取冲突）
            let ta_info = match self.heap.get(r.0 as usize) {
                Some(HeapObject::TypedArray {
                    kind,
                    buffer,
                    byte_offset,
                    length,
                }) => Some((*kind, *buffer, *byte_offset, *length)),
                _ => None,
            };
            if let Some((kind, buffer, byte_offset, length)) = ta_info {
                // 数值下标 → 元素读取（越界 undefined）
                if let Ok(i) = key.parse::<usize>() {
                    if i < length {
                        let off = byte_offset + i * kind.elem_size();
                        self.check_detached(buffer)?;
                        let Some(HeapObject::ArrayBuffer { data, .. }) =
                            self.heap.get(buffer.0 as usize)
                        else {
                            return Ok(Value::Undefined);
                        };
                        let elem = kind.read_le(data, off);
                        return Ok(self.decode_element(kind, elem));
                    }
                    return Ok(Value::Undefined);
                }
                let synthesized = match key {
                    "length" => Some(Value::Number(length as f64)),
                    "byteLength" => Some(Value::Number((length * kind.elem_size()) as f64)),
                    "byteOffset" => Some(Value::Number(byte_offset as f64)),
                    "buffer" => Some(Value::Object(buffer)),
                    _ => None,
                };
                if let Some(v) = synthesized {
                    return Ok(v);
                }
            }
            let dv_info = match self.heap.get(r.0 as usize) {
                Some(HeapObject::DataView {
                    buffer,
                    byte_offset,
                    byte_length,
                }) => Some((*buffer, *byte_offset, *byte_length)),
                _ => None,
            };
            if let Some((buffer, byte_offset, byte_length)) = dv_info {
                // Node：detached 后 DataView 的合成属性读取抛 TypeError
                // （与 TypedArray 元素访问同路径；transfer detach 场景）。
                self.check_detached(buffer)?;
                let synthesized = match key {
                    "byteLength" => Some(Value::Number(byte_length as f64)),
                    "byteOffset" => Some(Value::Number(byte_offset as f64)),
                    "buffer" => Some(Value::Object(buffer)),
                    _ => None,
                };
                if let Some(v) = synthesized {
                    return Ok(v);
                }
            }
            let ab_info = match self.heap.get(r.0 as usize) {
                Some(HeapObject::ArrayBuffer {
                    data,
                    resizable,
                    max_byte_length,
                    detached,
                    ..
                }) => Some((data.len(), *resizable, *max_byte_length, *detached)),
                _ => None,
            };
            if let Some((len, resizable, max_len, detached)) = ab_info {
                let synthesized = match key {
                    "byteLength" => Some(Value::Number(len as f64)),
                    "detached" => Some(Value::Boolean(detached)),
                    "resizable" => Some(Value::Boolean(resizable)),
                    "maxByteLength" => Some(Value::Number(max_len.max(len) as f64)),
                    _ => None,
                };
                if let Some(v) = synthesized {
                    return Ok(v);
                }
            }
        }
        // 函数对象的 `name`：从堆变体字段合成。
        //
        // `HeapObject::NativeFn`/`NativeCtor` 自带 `name` 字段，但属性读路径此前
        // 只查自有属性表 → `Array.name` 落回 fn_proto 的占位 → 得 `[function Function]`
        // （Node 为 "Array"）。此处优先合成，先于下方 fn_proto 兜底。
        if key == "name" {
            if let Some(r) = obj.as_object() {
                let name = match self.heap.get(r.0 as usize) {
                    Some(HeapObject::NativeFn { name, .. }) => Some(name.clone()),
                    Some(HeapObject::NativeCtor { name, .. }) => Some(name.clone()),
                    _ => None,
                };
                if let Some(n) = name {
                    return Ok(Value::Object(self.alloc_string(n)));
                }
            }
        }
        // Object.prototype.hasOwnProperty：不落于原型对象（保持零自有
        // 属性，for-in 口径对齐 Node.js 22 LTS 标准），属性链查不到时在此合成
        if key == "hasOwnProperty" {
            if let Some(h) = self.objproto_has_own {
                return Ok(Value::Object(h));
            }
        }
        // Symbol 接收者的 `description`：从堆字段合成（Node：`Symbol("d").description`
        // 为 "d"、`Symbol().description` 为 undefined）。此前落到 symbol_proto 的
        // 占位 NativeFn，typeof 非 undefined 但取值错。
        if key == "description" {
            if let Some(r) = obj.as_object() {
                if let Some(HeapObject::Symbol { description, .. }) = self.heap.get(r.0 as usize) {
                    if description.is_empty() {
                        return Ok(Value::Undefined);
                    }
                    let s = description.clone();
                    return Ok(Value::Object(self.alloc_string(s)));
                }
            }
        }
        // 原始值接收者的 `constructor`：Node 语义 `(1).constructor === Number`、
        // `"a".constructor === String`（包装构造器从全局解析）。原型方法表里没有
        // `constructor` 条目，故在此合成。
        if key == "constructor" {
            let ctor_name = match obj.case() {
                ValueCase::Number(_) => Some("Number"),
                ValueCase::Boolean(_) => Some("Boolean"),
                ValueCase::Object(r) => match self.heap.get(r.0 as usize) {
                    Some(HeapObject::String(_)) => Some("String"),
                    Some(HeapObject::Symbol { .. }) => Some("Symbol"),
                    _ => None,
                },
                _ => None,
            };
            if let Some(name) = ctor_name {
                if let Some(c) = self.resolve_global(name) {
                    return Ok(c);
                }
            }
        }
        // 原始值 / 无链内建实例的原型面兜底。
        //
        // 这些接收者无法参与上方通用原型链遍历：`Value::Number`/`Boolean` 不是
        // 堆对象；`HeapObject::String`/`Symbol`/`Closure`/`NativeFn`/`NativeCtor`
        // 没有 `[[Prototype]]` 字段（`get_prototype` 恒 None）。而 surface 已把
        // 原型方法以 `define_proto_method` 挂成 str_proto/num_proto/bool_proto/
        // symbol_proto/fn_proto 上的**真实属性**，故此处按接收者类别查一次——
        // 修复 `typeof "abc".toUpperCase` 为 undefined（按名调用一直可用，属性读
        // 拿不到函数值）与 `(1).toFixed` 同类缺口。
        if let Some(proto) = self.builtin_proto_of(obj) {
            if let Some(v) = self.own_value(proto.0 as usize, key) {
                return Ok(v);
            }
        }
        // null/undefined 上读属性：JS 语义抛 TypeError（Node 22 消息形态）
        if matches!(obj, Value::Undefined | Value::Null) {
            let kind = if obj == Value::Null {
                "null"
            } else {
                "undefined"
            };
            let err = self.alloc_error_instance(&format!(
                "Cannot read properties of {kind} (reading '{key}')"
            ));
            let name = self.alloc_string("TypeError".to_owned());
            let _ = self.set_property(Value::Object(err), "name", Value::Object(name));
            return Err(VmError::Thrown(Value::Object(err)));
        }
        Ok(Value::Undefined)
    }

    /// 设置属性（含数组下标写入、闭包对象属性写入与 Setter 访问器触发）。
    pub fn set_property(&mut self, obj: Value, key: &str, val: Value) -> Result<(), VmError> {
        // Proxy 对象：经 set trap 派发（假值返回抛 TypeError）
        if let Some(r) = obj.as_object() {
            if self.proxy_parts(r).is_some() {
                return self.proxy_set(r, key, val, obj);
            }
        }
        // process.env：Windows 下键写入大小写不敏感——命中既有键的原始大小写
        // 形态时以其实际键名重定向（Node 22 实测：`env.PATH = v` 更新 `Path` 键）。
        // 递归一次即收敛：实际键名字面不同，必然走精确命中路径。
        if let Some(env_id) = self.env_object {
            if obj == Value::Object(env_id) && self.own_value(env_id.0 as usize, key).is_none() {
                if let Some(actual) = self.env_find_key(env_id.0 as usize, key) {
                    return self.set_property(obj, &actual, val);
                }
            }
        }
        // globalThis：属性写入直通全局变量表
        if let Some(r) = obj.as_object() {
            if self.has_own_slot(r.0 as usize, "_isGlobalThis") {
                self.globals.insert(key.to_owned(), val);
                return Ok(());
            }
        }
        // RegExp 实例的 lastIndex：写线程局部状态表（堆对象无可变属性）
        if key == "lastIndex" {
            if let Some(r) = obj.as_object() {
                if matches!(self.heap.get(r.0 as usize), Some(HeapObject::RegExp { .. })) {
                    crate::interpreter::set_regex_last_index(r.0, to_number(val).max(0.0) as usize);
                    return Ok(());
                }
            }
        }
        // TypedArray 数值下标写入：按元素类型收窄/钳制后落盘（越界忽略）
        if let Some(r) = obj.as_object() {
            if let Ok(i) = key.parse::<usize>() {
                if matches!(
                    self.heap.get(r.0 as usize),
                    Some(HeapObject::TypedArray { .. })
                ) {
                    return self.ta_set(r, i, val);
                }
            }
        }
        if let Some(r) = obj.as_object() {
            let idx = r.0 as usize;
            if idx < self.heap.len() {
                // Setter 访问器优先：命中则调用（不写数据属性，对齐 JS [[Set]] 语义）
                let setter = match &self.heap[idx] {
                    HeapObject::Ordinary { setters, .. } => setters.get(key).copied(),
                    _ => None,
                };
                if let Some(s_val) = setter {
                    self.invoke_accessor(s_val, obj, &[val])?;
                    return Ok(());
                }
                match &mut self.heap[idx] {
                    HeapObject::Ordinary { props, deleted, .. } => {
                        // 曾删除的 key：清除删除标记（删除本身已把 shape 慢化为
                        // dict 模式——见 delete_property；此处 dict 分支原位更新/追加）
                        deleted.remove(key);
                        let next: Option<OrdinaryProps> = match props {
                            OrdinaryProps::Shape { shape, slots } => {
                                // 未删除过的新属性/命中：原位写槽或派生追加
                                if let Some(slot) = self
                                    .shape_table
                                    .shape(*shape)
                                    .and_then(|s| s.lookup(key))
                                    .filter(|&slot| slot < slots.len())
                                {
                                    slots[slot] = from_vm_value(val);
                                    None
                                } else if slots.len() >= crate::heap::DICT_THRESHOLD {
                                    // 达字典阈值：整体转字典模式（避免 shape 树 O(n²)
                                    // 克隆）；已删除键不迁入（重加按 append 语义）
                                    let mut properties: Vec<(String, Value)> =
                                        Vec::with_capacity(slots.len() + 1);
                                    if let Some(s) = self.shape_table.shape(*shape) {
                                        for (i, name) in s.names().enumerate() {
                                            if deleted.contains(name) {
                                                continue;
                                            }
                                            // 盒 → VM Value 物化进字典（保形状序 = 插入序）
                                            properties.push((
                                                name.to_owned(),
                                                slots
                                                    .get(i)
                                                    .map(|&b| to_vm_value(b))
                                                    .unwrap_or(Value::Undefined),
                                            ));
                                        }
                                    }
                                    properties.push((key.to_owned(), val));
                                    let index = properties
                                        .iter()
                                        .enumerate()
                                        .map(|(i, (k, _))| (k.clone(), i))
                                        .collect();
                                    Some(OrdinaryProps::Dict { properties, index })
                                } else {
                                    // 沿 shape transition 派生子隐藏类，追加槽位
                                    let new_shape = self.shape_table.transition(*shape, key);
                                    slots.push(from_vm_value(val));
                                    *shape = new_shape;
                                    None
                                }
                            }
                            OrdinaryProps::Dict { properties, index } => {
                                // 有序语义：既有键原位更新（保插入位置），否则追加。
                                // 键 → 槽位索引 O(1) 命中（顺序新键追加免全表扫描——
                                // 否则海量键对象如 Buffer 数值下标退化为 O(N²)）。
                                let hit = index
                                    .get(key)
                                    .copied()
                                    .filter(|&s| properties.get(s).is_some_and(|(k, _)| k == key));
                                match hit {
                                    Some(s) => properties[s].1 = val,
                                    None => {
                                        let s = properties.len();
                                        properties.push((key.to_owned(), val));
                                        index.insert(key.to_owned(), s);
                                    }
                                }
                                None
                            }
                        };
                        if let Some(next) = next {
                            *props = next;
                        }
                    }
                    HeapObject::Closure { properties, .. } => {
                        properties.insert(key.to_owned(), val);
                    }
                    HeapObject::NativeCtor { properties, .. } => {
                        properties.insert(key.to_owned(), val);
                    }
                    HeapObject::NativeFn { properties, .. } => {
                        properties.insert(key.to_owned(), val);
                    }
                    HeapObject::Array {
                        elements,
                        properties,
                        ..
                    } => {
                        if let Ok(i) = key.parse::<usize>() {
                            // 规范口径：数组索引 < 2^32-1；且本表示为密集
                            // Vec——超限或超大下标一律落入自有属性表
                            //（稀疏位不分配；M7.2 修复：`a[4294967295]=…`
                            // 曾触发 2^32×8 字节分配直接 OOM）
                            const DENSE_CAP: usize = 10_000_000;
                            if i < DENSE_CAP.min(4294967295) {
                                if i >= elements.len() {
                                    elements.resize(i + 1, Value::Undefined);
                                }
                                elements[i] = val;
                            } else {
                                properties.insert(key.to_owned(), val);
                            }
                        } else if key != "length" {
                            properties.insert(key.to_owned(), val);
                        }
                    }
                    _ => {}
                }
                // 写屏障：老容器写入年轻引用 → 记忆集（minor GC 次级根）
                self.gc_write_barrier(r, val);
            }
        }
        Ok(())
    }

    /// 判断属性（自有或沿原型链）是否存在于对象上（`in` 运算符语义）。
    pub fn has_property(&mut self, obj: Value, key: &str) -> bool {
        // Proxy 对象：经 has trap 派发
        if let Some(r) = obj.as_object() {
            if self.proxy_parts(r).is_some() {
                return self.proxy_has(r, key).unwrap_or(false);
            }
        }
        let mut cur = obj;
        let mut depth = 0;
        while let ValueCase::Object(r) = cur.case() {
            if depth > 100 {
                break;
            }
            depth += 1;
            let idx = r.0 as usize;
            if idx >= self.heap.len() {
                break;
            }
            match &self.heap[idx] {
                HeapObject::Ordinary {
                    getters,
                    setters,
                    proto,
                    ..
                } => {
                    if getters.contains_key(key) || setters.contains_key(key) {
                        return true;
                    }
                    if self.own_value(idx, key).is_some() {
                        return true;
                    }
                    cur = match proto {
                        Some(p) => Value::Object(*p),
                        None => break,
                    };
                }
                HeapObject::Closure { properties, .. }
                | HeapObject::NativeCtor { properties, .. } => {
                    if properties.contains_key(key) {
                        return true;
                    }
                    break;
                }
                HeapObject::Array { elements, .. } => {
                    if key == "length" {
                        return true;
                    }
                    if let Ok(i) = key.parse::<usize>() {
                        return i < elements.len();
                    }
                    break;
                }
                _ => break,
            }
        }
        false
    }

    /// 枚举对象自有属性（键 + 值），供 `{ ...src }` 展开使用。
    ///
    /// 普通对象取属性字典；数组产出索引键与 `length`；Proxy 经 ownKeys +
    /// get trap 派发。其余类型为空集。
    pub(crate) fn own_properties(&mut self, obj: Value) -> Vec<(String, Value)> {
        // Proxy 对象：ownKeys trap 列键、get trap 取值（规范 [[OwnPropertyKeys]]）
        if let Some(r) = obj.as_object() {
            if self.proxy_parts(r).is_some() {
                let keys = self.proxy_own_keys(r).unwrap_or_default();
                return keys
                    .into_iter()
                    .map(|k| {
                        let v = self.get_property(obj, &k).unwrap_or(Value::Undefined);
                        (k, v)
                    })
                    .collect();
            }
        }
        if let Some(r) = obj.as_object() {
            let idx = r.0 as usize;
            if idx < self.heap.len() {
                match &self.heap[idx] {
                    HeapObject::Ordinary { .. } => {
                        return self.own_entries(idx);
                    }
                    HeapObject::Closure {
                        properties,
                        getters,
                        non_enum,
                        ..
                    } => {
                        // 函数对象的自有面（Object.keys(require('body-parser'))
                        // 等：prototype + defineProperty 挂载的访问器键）
                        let mut out: Vec<(String, Value)> = properties
                            .iter()
                            .filter(|(k, _)| !non_enum.contains(*k))
                            .map(|(k, v)| (k.clone(), *v))
                            .collect();
                        for (k, g) in getters.iter() {
                            if !non_enum.contains(k) && !out.iter().any(|(k2, _)| k2 == k) {
                                out.push((k.clone(), *g));
                            }
                        }
                        return out;
                    }
                    HeapObject::Array { elements, .. } => {
                        let mut out = Vec::with_capacity(elements.len() + 1);
                        for (i, v) in elements.iter().enumerate() {
                            out.push((i.to_string(), *v));
                        }
                        out.push(("length".to_owned(), Value::Number(elements.len() as f64)));
                        return out;
                    }
                    _ => {}
                }
            }
        }
        Vec::new()
    }

    /// `for-in` 键枚举（对齐 Go 版 `EnumerateForInKeys`）。
    ///
    /// 沿原型链（≤128 层）收集自有键并去重（先到先得，自有键优先）；
    /// 字符串产出索引键（按 UTF-16 code unit 计）；原始值为空集。
    pub(crate) fn enumerate_for_in_keys(&self, val: Value) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cur = Some(val);
        for _ in 0..128 {
            let Some(v) = cur.take() else { break };
            let ValueCase::Object(r) = v.case() else {
                break;
            };
            let idx = r.0 as usize;
            let Some(h) = self.heap.get(idx) else { break };
            let (keys, proto) = match h {
                HeapObject::Ordinary {
                    props,
                    deleted,
                    non_enum,
                    proto,
                    ..
                } => {
                    let ks = match props {
                        OrdinaryProps::Shape { shape, .. } => self
                            .shape_table
                            .shape(*shape)
                            .map(|s| {
                                s.names()
                                    .filter(|n| !deleted.contains(*n) && !non_enum.contains(*n))
                                    .map(str::to_owned)
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default(),
                        OrdinaryProps::Dict { properties, .. } => properties
                            .iter()
                            .filter(|(k, _)| !deleted.contains(k) && !non_enum.contains(k))
                            .map(|(k, _)| k.clone())
                            .collect(),
                    };
                    (ks, *proto)
                }
                HeapObject::Array {
                    elements,
                    properties,
                    proto,
                } => {
                    let mut ks = (0..elements.len())
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>();
                    ks.extend(properties.keys().cloned());
                    (ks, *proto)
                }
                HeapObject::Closure { properties, .. } => {
                    (properties.keys().cloned().collect::<Vec<_>>(), None)
                }
                HeapObject::NativeCtor { properties, .. } => {
                    (properties.keys().cloned().collect::<Vec<_>>(), None)
                }
                HeapObject::NativeFn { properties, .. } => {
                    (properties.keys().cloned().collect::<Vec<_>>(), None)
                }
                HeapObject::String(s) => {
                    // 索引键按 UTF-16 code unit 计（星面字符占 2 个）
                    let units: usize = s.chars().map(|c| if c > '\u{FFFF}' { 2 } else { 1 }).sum();
                    let ks = (0..units).map(|i| i.to_string()).collect::<Vec<_>>();
                    (ks, None)
                }
                _ => (Vec::new(), None),
            };
            for k in keys {
                if seen.insert(k.clone()) {
                    out.push(k);
                }
            }
            cur = proto.map(Value::Object);
        }
        out
    }

    /// 读取对象的内部原型 [[Prototype]]。
    pub fn get_prototype(&self, val: Value) -> Option<ObjectRef> {
        if let Some(r) = val.as_object() {
            let idx = r.0 as usize;
            if let Some(obj) = self.heap.get(idx) {
                match obj {
                    HeapObject::Ordinary { proto, .. } => *proto,
                    HeapObject::Closure { proto, .. } => *proto,
                    HeapObject::Array { proto, .. } => *proto,
                    _ => None,
                }
            } else {
                None
            }
        } else {
            None
        }
    }

    /// 写入对象的内部原型 [[Prototype]]（`Object.setPrototypeOf` 底层语义；
    /// Proxy 由 [`crate::proxy`] 的 trap 路径先行拦截，此处仅处理普通容器）。
    /// 非 Ordinary/Array/Closure 对象为无操作。
    pub(crate) fn set_prototype_of(&mut self, obj: Value, proto: Option<ObjectRef>) {
        if let Some(r) = obj.as_object() {
            if let Some(
                HeapObject::Ordinary { proto: p, .. }
                | HeapObject::Closure { proto: p, .. }
                | HeapObject::Array { proto: p, .. },
            ) = self.heap.get_mut(r.0 as usize)
            {
                *p = proto;
            }
        }
    }

    /// 调用访问器函数值（Getter/Setter 共用）。
    ///
    /// 访问器表存闭包对象值：闭包须携带 upvalue 捕获（延迟调用时闭包引用
    /// 的模块/外层变量仍可解析，如 body-parser 的 getter → loadParser）；
    /// 其余可调用形态（原生函数等）走通用调用协议。
    fn invoke_accessor(
        &mut self,
        val: Value,
        this: Value,
        args: &[Value],
    ) -> Result<Value, VmError> {
        if let Some(r) = val.as_object() {
            if let Some(HeapObject::Closure {
                func_idx, upvalues, ..
            }) = self.heap.get(r.0 as usize)
            {
                return self.invoke_function(*func_idx, this, args, upvalues.clone());
            }
        }
        self.invoke_callable(val, this, args)
    }

    /// 合成对象属性的特性描述对象（`Object.getOwnPropertyDescriptor` 底层）。
    ///
    /// 本运行时数据属性恒为可写/可枚举/可配置（无属性位存储）；访问器经
    /// Ordinary 的 getter/setter 表判定。属性不存在时返回 undefined。
    pub(crate) fn ordinary_property_descriptor(
        &mut self,
        obj: Value,
        key: &str,
    ) -> Result<Value, VmError> {
        if !self.has_property(obj, key) {
            return Ok(Value::Undefined);
        }
        // 访问器描述优先（getter/setter 表命中即访问器属性）。表存访问器
        // 函数值（闭包/原生），描述面直接暴露该函数（对齐 JS 语义）。
        if let Some(r) = obj.as_object() {
            if let Some(HeapObject::Ordinary {
                getters, setters, ..
            }) = self.heap.get(r.0 as usize)
            {
                let g = getters.get(key).copied();
                let s = setters.get(key).copied();
                if g.is_some() || s.is_some() {
                    let desc = self.alloc_ordinary();
                    if let Some(gf) = g {
                        let _ = self.set_property(Value::Object(desc), "get", gf);
                    }
                    if let Some(sf) = s {
                        let _ = self.set_property(Value::Object(desc), "set", sf);
                    }
                    let _ =
                        self.set_property(Value::Object(desc), "enumerable", Value::Boolean(true));
                    let _ = self.set_property(
                        Value::Object(desc),
                        "configurable",
                        Value::Boolean(true),
                    );
                    return Ok(Value::Object(desc));
                }
            }
        }
        let value = self.get_property(obj, key)?;
        let desc = self.alloc_ordinary();
        let _ = self.set_property(Value::Object(desc), "value", value);
        let _ = self.set_property(Value::Object(desc), "writable", Value::Boolean(true));
        let _ = self.set_property(Value::Object(desc), "enumerable", Value::Boolean(true));
        let _ = self.set_property(Value::Object(desc), "configurable", Value::Boolean(true));
        Ok(Value::Object(desc))
    }

    /// 按描述对象定义属性（`Object.defineProperty` 底层语义）。
    ///
    /// 支持 value/writable（数据属性）与 get/set（访问器属性）；enumerable/
    /// configurable 位无存储（忽略）。目标为 Proxy 时经 trap 派发由调用方处理。
    pub(crate) fn ordinary_define_property(
        &mut self,
        obj: Value,
        key: &str,
        desc: Value,
    ) -> Result<(), VmError> {
        let get_v = |vm: &mut Vm, k: &str| -> Result<Value, VmError> { vm.get_property(desc, k) };
        // enumerable 位：缺省 false（JS 规范 defineProperty 语义）；
        // 本运行时记录到 non_enum 集合，供 Object.keys/entries 过滤
        let enumerable = get_v(self, "enumerable")
            .map(|v| self.truthy(v))
            .unwrap_or(false);
        let remember_enumerable = |non_enum: &mut std::collections::HashSet<String>, key: &str| {
            if !enumerable {
                non_enum.insert(key.to_owned());
            }
        };
        let has_get = self.has_property(desc, "get") && {
            let g = get_v(self, "get")?;
            !matches!(g, Value::Undefined)
        };
        let has_set = self.has_property(desc, "set") && {
            let s = get_v(self, "set")?;
            !matches!(s, Value::Undefined)
        };
        if has_get || has_set {
            // 访问器属性：注册 getter/setter 到访问器表（存访问器函数值，
            // 保留闭包 upvalue 捕获——延迟调用语义）
            let ValueCase::Object(r) = obj.case() else {
                return Ok(());
            };
            let idx = r.0 as usize;
            let g_val = if has_get {
                let g = get_v(self, "get")?;
                Some(g)
            } else {
                None
            };
            let s_val = if has_set {
                let s = get_v(self, "set")?;
                Some(s)
            } else {
                None
            };
            if let Some(HeapObject::Ordinary {
                getters,
                setters,
                has_accessors,
                non_enum,
                ..
            }) = self.heap.get_mut(idx)
            {
                if let Some(g) = g_val {
                    getters.insert(key.to_owned(), g);
                }
                if let Some(s) = s_val {
                    setters.insert(key.to_owned(), s);
                }
                *has_accessors = 1;
                remember_enumerable(non_enum, key);
            } else if let Some(HeapObject::Closure {
                getters, non_enum, ..
            }) = self.heap.get_mut(idx)
            {
                // 闭包（如 body-parser 的 `exports = module.exports = fn`）静态面
                // 访问器注册；Closure 无 setter 表，set 语义走 set_property 数据路径
                if let Some(g) = g_val {
                    getters.insert(key.to_owned(), g);
                }
                remember_enumerable(non_enum, key);
            }
            // 写屏障：容器注册访问器函数值（g/s 可为年轻闭包）
            if let Some(g) = g_val {
                self.gc_write_barrier(r, g);
            }
            if let Some(s) = s_val {
                self.gc_write_barrier(r, s);
            }
            return Ok(());
        }
        let value = get_v(self, "value")?;
        if !enumerable {
            if let Some(r) = obj.as_object() {
                if let Some(HeapObject::Ordinary {
                    non_enum, props, ..
                }) = self.heap.get_mut(r.0 as usize)
                {
                    // 数据属性不可枚举：记 non_enum（自身存储仍走数据路径）
                    non_enum.insert(key.to_owned());
                    let _ = props;
                } else if let Some(HeapObject::Closure { non_enum, .. }) =
                    self.heap.get_mut(r.0 as usize)
                {
                    non_enum.insert(key.to_owned());
                }
            }
        }
        self.set_property(obj, key, value)
    }

    /// 内建构造器名 ↔ 实例堆变体判定（`instanceof` 兜底）。
    ///
    /// 返回 `None` 表示「该构造器名不参与兜底」，交回通用原型链遍历。
    /// 只处理**无 `[[Prototype]]` 字段**的内建实例（有链的 Array/Object/Error
    /// 等仍走链遍历，语义不变）。
    fn builtin_instance_of(&self, lr: aluka_core::ObjectRef, ctor_name: &str) -> Option<bool> {
        let obj = self.heap.get(lr.0 as usize)?;
        let verdict = match ctor_name {
            // Map/Set 共用 `HeapObject::Map` 变体，靠 Set 实例登记区分
            "Map" => {
                matches!(obj, HeapObject::Map { .. }) && !self.is_set_instance(Value::Object(lr))
            }
            "Set" => {
                matches!(obj, HeapObject::Map { .. }) && self.is_set_instance(Value::Object(lr))
            }
            "WeakMap" | "WeakSet" | "WeakRef" => false,
            "Promise" => matches!(obj, HeapObject::Promise { .. }),
            "Function" => matches!(
                obj,
                HeapObject::Closure { .. }
                    | HeapObject::NativeFn { .. }
                    | HeapObject::NativeCtor { .. }
            ),
            "Date" => self.has_own_slot(lr.0 as usize, "_isDate"),
            "ArrayBuffer" => matches!(obj, HeapObject::ArrayBuffer { shared: false, .. }),
            "SharedArrayBuffer" => matches!(obj, HeapObject::ArrayBuffer { shared: true, .. }),
            "DataView" => matches!(obj, HeapObject::DataView { .. }),
            // 11 种 TypedArray 构造器：按 TypedArray 变体的 kind 名匹配
            name if crate::typed_array::TypedKind::all()
                .iter()
                .any(|k| k.ctor_name() == name) =>
            {
                matches!(obj, HeapObject::TypedArray { kind, .. } if kind.ctor_name() == name)
            }
            _ => return None,
        };
        Some(verdict)
    }

    /// 原始值与**无 `[[Prototype]]` 字段**的内建实例所对应的原型对象。
    ///
    /// 仅用于属性读兜底（见 [`Vm::get_property`] 收尾）：这些接收者不参与通用
    /// 原型链遍历（`get_prototype` 对它们恒返回 `None`），但 surface 已把原型方法
    /// 挂成真实属性，故按类别直接给出对应的原型单例。
    fn builtin_proto_of(&mut self, obj: Value) -> Option<aluka_core::ObjectRef> {
        use crate::builtins::surface as s;
        match obj.case() {
            ValueCase::Number(_) => Some(s::num_proto(self)),
            ValueCase::Boolean(_) => Some(s::bool_proto(self)),
            ValueCase::Object(r) => match self.heap.get(r.0 as usize) {
                Some(HeapObject::String(_)) => Some(s::str_proto(self)),
                Some(HeapObject::Symbol { .. }) => Some(s::symbol_proto(self)),
                Some(
                    HeapObject::Closure { .. }
                    | HeapObject::NativeFn { .. }
                    | HeapObject::NativeCtor { .. },
                ) => Some(s::fn_proto(self)),
                _ => None,
            },
            _ => None,
        }
    }

    /// 检查 l instanceof r（沿着 l 的原型链查找 r.prototype）。
    pub fn check_instanceof(&mut self, l: Value, r: Value) -> bool {
        // RegExp 实例（无原型链字段的堆形态）对 RegExp 构造器特判
        if let (ValueCase::Object(lr), ValueCase::Object(rr)) = (l.case(), r.case()) {
            if self.regexp_ctor == Some(rr)
                && matches!(
                    self.heap.get(lr.0 as usize),
                    Some(HeapObject::RegExp { .. })
                )
            {
                return true;
            }
        }
        // 内建实例兜底：Map/Set/Promise/Date/TypedArray/ArrayBuffer/DataView/函数
        // 等实例用**无 `[[Prototype]]` 字段**的堆变体表示（`get_prototype` 恒 None），
        // 无法参与下方通用链遍历，故按「构造器 ↔ 堆变体」判定。
        // 仅在 `r` 是 VM 自建构造器（NativeCtor，且名字为内建名）时生效——
        // 用户自定义 class 是 Closure，不会误命中。
        if let (ValueCase::Object(lr), ValueCase::Object(rr)) = (l.case(), r.case()) {
            if let Some(ctor_name) = match self.heap.get(rr.0 as usize) {
                Some(HeapObject::NativeCtor { name, .. }) => Some(name.clone()),
                _ => None,
            } {
                if let Some(v) = self.builtin_instance_of(lr, &ctor_name) {
                    return v;
                }
            }
        }
        let target_proto = match self.get_property(r, "prototype").map(|v| v.case()) {
            Ok(ValueCase::Object(p)) => p,
            _ => return false,
        };
        // 原型链遍历对 Proxy 感知：链上 Proxy 经 getPrototypeOf trap 解析
        let mut cur = match l.case() {
            ValueCase::Object(lr) if self.proxy_parts(lr).is_some() => {
                match self.proxy_get_prototype_of(lr).map(|v| v.case()) {
                    Ok(ValueCase::Object(p)) => Some(p),
                    _ => None,
                }
            }
            other => self.get_prototype(Value::from(other)),
        };
        let mut depth = 0;
        while let Some(proto_ref) = cur {
            if depth > 100 {
                break;
            }
            depth += 1;
            if proto_ref == target_proto {
                return true;
            }
            cur = self.get_prototype(Value::Object(proto_ref));
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::HeapObject;

    fn vm() -> Vm {
        Vm::new(0)
    }

    fn obj(vm: &mut Vm) -> Value {
        Value::Object(vm.alloc_ordinary())
    }

    /// 小对象（< 字典阈值）保持在快速 shape 模式，且结构相同的对象共享同一 shape。
    #[test]
    fn small_objects_stay_in_fast_shape_mode_and_share_shape() {
        let mut vm = vm();
        let a = obj(&mut vm);
        let b = obj(&mut vm);
        let _ = vm.set_property(a, "x", Value::Number(1.0));
        let _ = vm.set_property(a, "y", Value::Number(2.0));
        let _ = vm.set_property(b, "x", Value::Number(3.0));
        let _ = vm.set_property(b, "y", Value::Number(4.0));
        let (Some(ra), Some(rb)) = (a.as_object(), b.as_object()) else {
            unreachable!()
        };
        let (HeapObject::Ordinary { props: pa, .. }, HeapObject::Ordinary { props: pb, .. }) =
            (&vm.heap[ra.0 as usize], &vm.heap[rb.0 as usize])
        else {
            unreachable!()
        };
        let (OrdinaryProps::Shape { shape: sa, .. }, OrdinaryProps::Shape { shape: sb, .. }) =
            (pa, pb)
        else {
            panic!("小对象必须保持在快速 shape 模式");
        };
        assert_eq!(sa, sb, "结构相同的对象必须共享同一 shape（PIC 命中前提）");
        // 读写经 shape 路径正确
        assert!(
            vm.get_property(a, "y")
                .is_ok_and(|v| v.as_number() == Some(2.0))
        );
        assert!(
            vm.get_property(b, "y")
                .is_ok_and(|v| v.as_number() == Some(4.0))
        );
    }

    /// 属性数达字典阈值后整体转字典模式：海量键对象（Buffer 数值下标等）
    /// 不得沿 shape transition 树逐键克隆前缀（O(n²) 内存爆炸回归防护）。
    #[test]
    fn oversized_objects_switch_to_dict_mode_bounded_memory() {
        let mut vm = vm();
        let o = obj(&mut vm);
        const N: usize = 256; // 远超市面字典阈值 32
        // 引导期（Vm::new 预建原型链）已产生基线 shape，只断言增量
        let shapes_before = vm.shape_table.len();
        for i in 0..N {
            let _ = vm.set_property(o, &i.to_string(), Value::Number(i as f64));
        }
        let Some(r) = o.as_object() else {
            unreachable!()
        };
        let HeapObject::Ordinary { props, .. } = &vm.heap[r.0 as usize] else {
            unreachable!()
        };
        assert!(
            matches!(props, OrdinaryProps::Dict { .. }),
            "超阈值对象必须转字典模式"
        );
        // shape 树只应新增阈值内的过渡（≪ N），证明未逐键克隆前缀
        let shape_growth = vm.shape_table.len() - shapes_before;
        assert!(
            shape_growth < 64,
            "shape 树不应随字典模式对象增长（新增 {shape_growth}）"
        );
        // 全部属性可读、可枚举、可删
        for i in 0..N {
            assert!(
                vm.get_property(o, &i.to_string())
                    .is_ok_and(|v| v.as_number() == Some(i as f64))
            );
        }
        assert_eq!(vm.own_entries(r.0 as usize).len(), N);
        vm.delete_property(o, "128");
        assert!(matches!(vm.get_property(o, "128"), Ok(Value::Undefined)));
        assert_eq!(vm.own_entries(r.0 as usize).len(), N - 1);
        // 删除后重写恢复
        let _ = vm.set_property(o, "128", Value::Number(128.0));
        assert!(
            vm.get_property(o, "128")
                .is_ok_and(|v| v.as_number() == Some(128.0))
        );
    }
}
