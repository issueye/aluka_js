//! ECMAScript 全局函数与全局对象（Node.js 22 LTS 规范面）。
//!
//! 覆盖真实 npm 包的硬需求面：`isNaN`/`isFinite`/`parseFloat`/`parseInt`、
//! `Number`（转换调用 + 数值静态面）、`Boolean`、`encodeURIComponent`/
//! `decodeURIComponent`/`encodeURI`/`decodeURI`、`Date`（now/解析/格式化
//! 最小面）、`globalThis`（属性读写直通全局变量表）。

use crate::builtins::{
    BuiltinHandler, BuiltinRegistry, ModuleDef, current_receiver, register_handler,
};
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::ops::to_number;
use crate::value::Value;
use aluka_core::ObjectRef;

/// 全局函数模块（在 `register_all` 中装配）。
pub const MODULE: ModuleDef = ModuleDef {
    name: "globals",
    build,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    // 直接调用型全局函数：单例 NativeFn + 注册表分派
    let fns: &[(&str, &str, BuiltinHandler)] = &[
        ("isNaN", "global.isNaN", global_is_nan),
        ("isFinite", "global.isFinite", global_is_finite),
        ("parseFloat", "global.parseFloat", global_parse_float),
        ("parseInt", "global.parse_int", global_parse_int),
        (
            "encodeURIComponent",
            "global.encodeURIComponent",
            encode_uri_component,
        ),
        (
            "decodeURIComponent",
            "global.decodeURIComponent",
            decode_uri_component,
        ),
        ("encodeURI", "global.encodeURI", encode_uri),
        ("decodeURI", "global.decodeURI", decode_uri),
    ];
    for (name, _key, handler) in fns {
        let f = vm.alloc_native_fn(&format!("global.{name}"));
        vm.globals.insert(name.to_string(), Value::Object(f));
        register_handler(registry, "global", name, *handler);
    }

    // Number：转换调用 + 数值静态面（prototype 指向方法面单例——真实包
    // `Number.prototype.toString` 存槽后 `.call` 调用依赖方法属性存在）
    let num_p = crate::builtins::surface::num_proto(vm);
    let number = vm.alloc_native_ctor("Number", Some(num_p));
    let statics: &[(&str, Value)] = &[
        ("MAX_SAFE_INTEGER", Value::Number(9007199254740991.0)),
        ("MIN_SAFE_INTEGER", Value::Number(-9007199254740991.0)),
        ("EPSILON", Value::Number(f64::EPSILON)),
        ("MAX_VALUE", Value::Number(f64::MAX)),
        ("MIN_VALUE", Value::Number(f64::MIN_POSITIVE)),
        ("POSITIVE_INFINITY", Value::Number(f64::INFINITY)),
        ("NEGATIVE_INFINITY", Value::Number(f64::NEG_INFINITY)),
        ("NaN", Value::Number(f64::NAN)),
    ];
    for (key, value) in statics {
        let _ = vm.set_property(Value::Object(number), key, *value);
    }
    for method in [
        "isInteger",
        "isSafeInteger",
        "isFinite",
        "isNaN",
        "parseInt",
        "parseFloat",
    ] {
        let f = vm.alloc_native_fn(&format!("Number.{method}"));
        let _ = vm.set_property(Value::Object(number), method, Value::Object(f));
        register_handler(registry, "Number", method, number_static);
    }
    vm.globals
        .insert("Number".to_owned(), Value::Object(number));

    // Boolean：转换调用（prototype 指向方法面单例）
    let bool_p = crate::builtins::surface::bool_proto(vm);
    let boolean = vm.alloc_native_ctor("Boolean", Some(bool_p));
    vm.globals
        .insert("Boolean".to_owned(), Value::Object(boolean));

    // String：静态方法 fromCharCode/fromCodePoint（iconv-lite 等真实包依赖）；
    // prototype 指向方法面单例——express 依赖树（get-intrinsic/call-bound 等）
    // 顶层存 `String.prototype.slice/indexOf/...` 槽位再 `.call` 调用，取属性
    // 必须是真实方法值（曾指向 object_prototype 空对象导致全链 undefined）
    let str_p = crate::builtins::surface::str_proto(vm);
    let string = vm.alloc_native_ctor("String", Some(str_p));
    for (method, handler) in [
        ("fromCharCode", string_from_char_code as BuiltinHandler),
        ("fromCodePoint", string_from_code_point as BuiltinHandler),
    ] {
        let f = vm.alloc_native_fn(&format!("String.{method}"));
        let _ = vm.set_property(Value::Object(string), method, Value::Object(f));
        register_handler(registry, "String", method, handler);
    }
    vm.globals
        .insert("String".to_owned(), Value::Object(string));

    // Date：now 静态 + 实例最小面（getTime/toISOString/valueOf/toString）。
    // prototype 为独立普通对象（曾与 Object.prototype 共享同一空对象，
    // 构造器原型互相污染）
    let date_proto = vm.alloc_ordinary_with_proto(vm.object_prototype);
    let date = vm.alloc_native_ctor("Date", Some(date_proto));
    let now = vm.alloc_native_fn("Date.now");
    let _ = vm.set_property(Value::Object(date), "now", Value::Object(now));
    register_handler(registry, "Date", "now", date_now);
    let parse = vm.alloc_native_fn("Date.parse");
    let _ = vm.set_property(Value::Object(date), "parse", Value::Object(parse));
    register_handler(registry, "Date", "parse", date_parse);
    for method in [
        "getTime",
        "valueOf",
        "toISOString",
        "toString",
        "getTimezoneOffset",
    ] {
        let date_fn = vm.alloc_native_fn(&format!("Date.{method}"));
        let _ = vm.set_property(Value::Object(date), method, Value::Object(date_fn));
        register_handler(registry, "Date", method, date_instance_method);
    }
    vm.globals.insert("Date".to_owned(), Value::Object(date));

    // ===== M4: Fetch API + AbortController =====
    let fetch_fn = vm.alloc_native_fn("fetch");
    vm.globals
        .insert("fetch".to_owned(), Value::Object(fetch_fn));
    // 裸调用经 NativeFn 名查表：键必须恰为 "fetch"（"fetch.fetch" 永不命中）
    registry.dispatch.insert("fetch".to_owned(), global_fetch);

    let abort_ctor = vm.alloc_native_ctor("AbortController", None);
    vm.globals
        .insert("AbortController".to_owned(), Value::Object(abort_ctor));
    // 构造器体经 do_construct 以裸构造器名查表（对齐 ReadableStream 注册法）
    registry
        .dispatch
        .insert("AbortController".to_owned(), abort_controller_ctor_impl);
    // M4：abort 方法与 signal 监听器分派键（与 NativeFn 名严格对齐）
    register_handler(registry, "AbortController", "abort", controller_abort_impl);
    register_handler(registry, "AbortSignal", "abort", signal_abort_impl);
    register_handler(
        registry,
        "AbortSignal",
        "addEventListener",
        signal_add_event_listener,
    );
    register_handler(
        registry,
        "AbortSignal",
        "removeEventListener",
        signal_remove_event_listener,
    );

    let headers_ctor = vm.alloc_native_ctor("Headers", None);
    vm.globals
        .insert("Headers".to_owned(), Value::Object(headers_ctor));
    registry
        .dispatch
        .insert("Headers".to_owned(), headers_ctor_impl);
    register_handler(registry, "Headers", "get", headers_get_impl);
    register_handler(registry, "Headers", "has", headers_has_impl);

    // Response.text / Response.json handler（fetch 返回的 Response 对象方法）
    register_handler(registry, "Response", "text", response_text_handler);
    register_handler(registry, "Response", "json", response_json_handler);
    register_handler(
        registry,
        "Response",
        "arrayBuffer",
        response_array_buffer_handler,
    );
    register_handler(registry, "Response", "formData", response_form_data_handler);

    // ===== M4.1 Request 全局构造器（fetch(request) 直传形态）=====
    let request_ctor = vm.alloc_native_ctor("Request", None);
    vm.globals
        .insert("Request".to_owned(), Value::Object(request_ctor));
    registry
        .dispatch
        .insert("Request".to_owned(), request_ctor_impl);
    register_handler(registry, "Request", "clone", request_noop_self);
    register_handler(registry, "Request", "text", response_text_handler);

    // ===== M4.4 EventTarget / CustomEvent / FormData 全局构造器 =====
    let et_ctor = vm.alloc_native_ctor("EventTarget", None);
    vm.globals
        .insert("EventTarget".to_owned(), Value::Object(et_ctor));
    registry
        .dispatch
        .insert("EventTarget".to_owned(), event_target_ctor_impl);
    for method in ["addEventListener", "removeEventListener", "dispatchEvent"] {
        register_handler(registry, "EventTarget", method, event_target_dispatch);
    }

    let ce_ctor = vm.alloc_native_ctor("CustomEvent", None);
    vm.globals
        .insert("CustomEvent".to_owned(), Value::Object(ce_ctor));
    registry
        .dispatch
        .insert("CustomEvent".to_owned(), custom_event_ctor_impl);

    let fd_ctor = vm.alloc_native_ctor("FormData", None);
    vm.globals
        .insert("FormData".to_owned(), Value::Object(fd_ctor));
    registry
        .dispatch
        .insert("FormData".to_owned(), form_data_ctor_impl);
    for method in [
        "append", "set", "get", "getAll", "has", "delete", "entries", "keys", "values", "forEach",
    ] {
        register_handler(registry, "FormData", method, form_data_method);
    }

    // ===== Object 静态方法面（真实包硬需求）=====
    if let Some(octor) = vm.object_ctor {
        vm.builtin_registry.register_module_object("Object", octor);
        for method in [
            "defineProperty",
            "defineProperties",
            "getOwnPropertyDescriptor",
            "getOwnPropertyNames",
            "setPrototypeOf",
            "getPrototypeOf",
            "assign",
            "freeze",
            "seal",
            "isFrozen",
            "isSealed",
            "values",
            "entries",
            "fromEntries",
        ] {
            let f = vm.alloc_native_fn(&format!("Object.{method}"));
            let _ = vm.set_property(Value::Object(octor), method, Value::Object(f));
            register_handler(registry, "Object", method, object_static);
        }
    }

    // ===== Error 静态面：captureStackTrace / stackTraceLimit =====
    if let Some(ector) = vm.error_ctor {
        vm.builtin_registry.register_module_object("Error", ector);
        let cap = vm.alloc_native_fn("Error.captureStackTrace");
        let _ = vm.set_property(
            Value::Object(ector),
            "captureStackTrace",
            Value::Object(cap),
        );
        register_handler(
            registry,
            "Error",
            "captureStackTrace",
            error_capture_stack_trace,
        );
        let _ = vm.set_property(Value::Object(ector), "stackTraceLimit", Value::Number(10.0));
    }

    // 调用点对象方法（Error.captureStackTrace 生成的 stack 元素）
    for method in [
        "getFileName",
        "getLineNumber",
        "getColumnNumber",
        "toString",
        "isNative",
        "isEval",
        "isConstructor",
        "getFunctionName",
        "getTypeName",
    ] {
        register_handler(registry, "callsite", method, callsite_method);
    }

    // globalThis：属性读写直通全局变量表
    let this_obj = vm.alloc_ordinary();
    let marker = vm.alloc_string("_isGlobalThis".to_owned());
    let _ = vm.set_property(
        Value::Object(this_obj),
        "_isGlobalThis",
        Value::Object(marker),
    );
    vm.globals
        .insert("globalThis".to_owned(), Value::Object(this_obj));

    Ok(vm.alloc_ordinary())
}

/// 全局函数注册表处理器统一形态（handler 签名同名别名）。
#[allow(non_upper_case_globals)]
const _: () = ();

fn arg_number(vm: &mut Vm, args: &[Value]) -> f64 {
    // JS ToNumber：堆字符串对象须解析（isNaN('23') → false 等真实包依赖）
    args.first()
        .map(|v| vm.to_number_value(*v))
        .unwrap_or(f64::NAN)
}

fn global_is_nan(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Boolean(arg_number(vm, args).is_nan()))
}

fn global_is_finite(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let n = arg_number(vm, args);
    Ok(Value::Boolean(n.is_finite()))
}

fn global_parse_float(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let text = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let t = text.trim_start();
    // 最长前缀数值解析（规范 ToNumber 语法的字符串子集）
    let mut end = 0usize;
    let bytes: Vec<char> = t.chars().collect();
    let mut seen_dot = false;
    let mut seen_exp = false;
    let mut prev_digit = false;
    for (i, c) in bytes.iter().enumerate() {
        if c.is_ascii_digit() {
            prev_digit = true;
            end = i + 1;
            continue;
        }
        if (*c == '+' || *c == '-') && (i == 0 || (seen_exp && !prev_digit)) {
            end = i + 1;
            continue;
        }
        if *c == '.' && !seen_dot && !seen_exp {
            seen_dot = true;
            end = i + 1;
            continue;
        }
        if (*c == 'e' || *c == 'E') && prev_digit && !seen_exp {
            seen_exp = true;
            prev_digit = false;
            end = i + 1;
            continue;
        }
        break;
    }
    let prefix: String = bytes[..end].iter().collect();
    let n = prefix.parse::<f64>().unwrap_or(f64::NAN);
    Ok(Value::Number(n))
}

fn global_parse_int(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let text = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let radix = args.get(1).map(|v| to_number(*v)).unwrap_or(0.0);
    let t = text.trim_start();
    let (t, radix) = if radix == 0.0 || radix.is_nan() {
        if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            (hex, 16.0)
        } else {
            (t, 10.0)
        }
    } else {
        (t, radix)
    };
    let mut end = 0usize;
    let chars: Vec<char> = t.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        let is_digit = c.is_ascii_digit() || c.is_ascii_alphabetic();
        if !is_digit {
            break;
        }
        let v = c.to_digit(36).unwrap_or(36) as f64;
        if v >= radix {
            break;
        }
        end = i + 1;
    }
    if end == 0 {
        return Ok(Value::Number(f64::NAN));
    }
    let prefix: String = chars[..end].iter().collect();
    match i64::from_str_radix(&prefix, radix as u32) {
        Ok(v) => Ok(Value::Number(v as f64)),
        Err(_) => Ok(Value::Number(f64::NAN)),
    }
}

fn number_static(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = match current_receiver() {
        Value::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(HeapObject::NativeFn { name, .. }) => name.clone(),
            _ => String::new(),
        },
        _ => String::new(),
    };
    let method = name.split('.').next_back().unwrap_or("");
    let v = args.first().copied().unwrap_or(Value::Undefined);
    // 数字静态面/全局 isNaN 族的数值化：字符串对象须解析（JS ToNumber
    // 语义；`isNaN('23')` → false、`Number.isSafeInteger('5')` → false）。
    // to_number_value 处理堆字符串；原始值经 to_number 兜底。
    let to_num = |vm: &mut Vm, v: Value| -> f64 { vm.to_number_value(v) };
    match method {
        "isInteger" => {
            let n = to_num(vm, v);
            let is_int = matches!(v, Value::Number(_)) && n.fract() == 0.0 && n.is_finite();
            Ok(Value::Boolean(is_int))
        }
        "isSafeInteger" => {
            let n = to_num(vm, v);
            let ok = matches!(v, Value::Number(_))
                && n.fract() == 0.0
                && n.is_finite()
                && n.abs() <= 9007199254740991.0;
            Ok(Value::Boolean(ok))
        }
        "isFinite" => {
            let n = to_num(vm, v);
            Ok(Value::Boolean(
                matches!(v, Value::Number(_)) && n.is_finite(),
            ))
        }
        "isNaN" => Ok(Value::Boolean(to_num(vm, v).is_nan())),
        "parseInt" => global_parse_int(vm, args),
        "parseFloat" => global_parse_float(vm, args),
        _ => Ok(Value::Undefined),
    }
}

/// `String.fromCharCode(...codes)`：将数值码点序列转为字符串（UTF-16 码元）。
fn string_from_char_code(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut s = String::with_capacity(args.len());
    for arg in args {
        let code = to_number(*arg) as u32;
        // UTF-16 代理对：0x10000+ 拆为高低代理
        if code < 0x10000 {
            s.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
        } else if code < 0x110000 {
            let hi = 0xD800 + ((code - 0x10000) >> 10);
            let lo = 0xDC00 + ((code - 0x10000) & 0x3FF);
            s.push(char::from_u32(hi).unwrap_or('\u{FFFD}'));
            s.push(char::from_u32(lo).unwrap_or('\u{FFFD}'));
        } else {
            s.push('\u{FFFD}');
        }
    }
    Ok(Value::Object(vm.alloc_string(s)))
}

/// `String.fromCodePoint(...codes)`：将 Unicode 码点序列转为字符串。
fn string_from_code_point(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let mut s = String::with_capacity(args.len());
    for arg in args {
        let code = to_number(*arg) as u32;
        s.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
    }
    Ok(Value::Object(vm.alloc_string(s)))
}

fn date_now(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0);
    Ok(Value::Number(now))
}

fn date_parse(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let text = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    // ISO 8601 子集：YYYY-MM-DD[THH:MM:SS[.mmm][Z]]
    let parsed = parse_iso_date(&text).unwrap_or(f64::NAN);
    Ok(Value::Number(parsed))
}

fn parse_iso_date(text: &str) -> Option<f64> {
    fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }
    let t = text.trim();
    let (date_part, time_part) = match t.find('T').or_else(|| t.find(' ')) {
        Some(i) => (&t[..i], Some(&t[i + 1..])),
        None => (t, None),
    };
    let mut dp = date_part.split('-');
    let y: i64 = dp.next()?.parse().ok()?;
    let m: i64 = dp.next()?.parse().ok()?;
    let d: i64 = dp.next()?.parse().ok()?;
    let (mut hh, mut mm, mut ss) = (0i64, 0i64, 0i64);
    let mut tz_offset_ms = 0f64;
    if let Some(tp) = time_part {
        let tp = tp.trim_end_matches('Z');
        let parts: Vec<&str> = tp.split(':').collect();
        hh = parts.first()?.parse().ok()?;
        mm = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let sec_part = parts.get(2).copied().unwrap_or("0");
        if let Some(dot) = sec_part.find('.') {
            ss = sec_part[..dot].parse().ok()?;
        } else {
            ss = sec_part.parse().ok()?;
        }
        if t.ends_with('Z') {
            tz_offset_ms = 0.0;
        }
    }
    let days = days_from_civil(y, m, d) as f64;
    Some(days * 86_400_000.0 + (hh * 3600 + mm * 60 + ss) as f64 * 1000.0 - tz_offset_ms)
}

fn date_time_value(vm: &Vm, v: Value) -> f64 {
    match v {
        Value::Object(r) => match vm.own_value(r.0 as usize, "_timeValue") {
            Some(Value::Number(n)) => n,
            _ => f64::NAN,
        },
        _ => f64::NAN,
    }
}

// ===== URI 编解码 =====

fn is_uri_unreserved(c: u8) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
        )
}

fn encode_component(vm: &mut Vm, args: &[Value], extra_safe: &[u8]) -> Result<Value, VmError> {
    let text = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let mut out = String::new();
    for byte in text.as_bytes() {
        if is_uri_unreserved(*byte) || extra_safe.contains(byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    Ok(Value::Object(vm.alloc_string(out)))
}

fn decode_component_impl(
    vm: &mut Vm,
    args: &[Value],
    plus_as_space: bool,
) -> Result<Value, VmError> {
    let text = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' if plus_as_space => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Ok(Value::Object(vm.alloc_string(
        match String::from_utf8(out) {
            Ok(s) => s,
            Err(_) => {
                let err = vm.alloc_string("URI malformed".to_owned());
                let name = vm.alloc_string("URIError".to_owned());
                let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
                return Err(VmError::Thrown(Value::Object(err)));
            }
        },
    )))
}

fn encode_uri_component(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    encode_component(vm, args, &[])
}

fn decode_uri_component(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    decode_component_impl(vm, args, false)
}

fn encode_uri(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    encode_component(vm, args, b";/?:@&=+$,#")
}

fn decode_uri(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    decode_component_impl(vm, args, false)
}

impl Vm {
    /// `new Date([value])`：时间值 / ISO 字符串 / 当前时间。
    pub(crate) fn construct_date(&mut self, args: &[Value]) -> Result<Value, VmError> {
        let time = match args.first() {
            None | Some(Value::Undefined) => date_now_ms(),
            Some(v) => match v {
                Value::Number(n) => *n,
                other => {
                    let text = self.format_value(*other);
                    parse_iso_date(&text).unwrap_or(f64::NAN)
                }
            },
        };
        let inst = self.alloc_ordinary();
        let ns = self.alloc_string("Date".to_owned());
        let _ = self.set_property(Value::Object(inst), "_builtinNs", Value::Object(ns));
        let _ = self.set_property(Value::Object(inst), "_isDate", Value::Boolean(true));
        let _ = self.set_property(Value::Object(inst), "_timeValue", Value::Number(time));
        Ok(Value::Object(inst))
    }
}

fn date_now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

/// Date 实例方法（经 `_builtinNs: "Date"` 命名空间分派）。
fn date_instance_method(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let method = match receiver {
        Value::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(HeapObject::NativeFn { name, .. }) => {
                name.clone().split('.').next_back().unwrap_or("").to_owned()
            }
            _ => String::new(),
        },
        _ => String::new(),
    };
    let t = date_time_value(vm, receiver);
    match method.as_str() {
        "getTime" | "valueOf" => Ok(Value::Number(t)),
        "getTimezoneOffset" => Ok(Value::Number(0.0)),
        "toString" => Ok(Value::Object(vm.alloc_string(date_to_iso_string(t, false)))),
        "toISOString" => Ok(Value::Object(vm.alloc_string(date_to_iso_string(t, true)))),
        _ => Ok(Value::Undefined),
    }
}

/// 毫秒时间值 → ISO 8601（UTC）；`ms_precision=false` 时截断到秒。
fn date_to_iso_string(t: f64, ms_precision: bool) -> String {
    if t.is_nan() {
        return "Invalid Date".to_owned();
    }
    let secs_total = (t / 1000.0).floor() as i64;
    let millis = (t - secs_total as f64 * 1000.0).round() as i64;
    let days = secs_total.div_euclid(86400);
    let secs_of_day = secs_total.rem_euclid(86400);
    let (h, m, sec) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    if ms_precision {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            y, mth, d, h, m, sec, millis
        )
    } else {
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mth, d, h, m, sec)
    }
}

/// Object 静态方法统一分派。
fn object_static(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    // 方法名优先取 pending_native_name（try_dispatch/注册表分派均登记）；
    // 回退 receiver 本体 NativeFn 名（`.call` 形态）
    let method = super::pending_native_name()
        .split('.')
        .next_back()
        .unwrap_or("")
        .to_owned();

    let _ = current_receiver();
    let target = args.first().copied().unwrap_or(Value::Undefined);
    match method.as_str() {
        "defineProperty" => {
            let key = args
                .get(1)
                .map(|v| vm.to_property_key(*v))
                .unwrap_or_default();
            let desc = args.get(2).copied().unwrap_or(Value::Undefined);
            if let Value::Object(r) = target {
                if vm.proxy_parts(r).is_some() {
                    let ok = vm.proxy_define_property(r, &key, desc)?;
                    return Ok(Value::Boolean(ok));
                }
            }
            vm.ordinary_define_property(target, &key, desc)?;
            Ok(target)
        }
        "defineProperties" => {
            // 逐描述符定义（描述符对象的自有键 → defineProperty）
            if let Some(props) = args.get(1).copied() {
                let items = vm.own_properties(props);
                for (k, desc) in items {
                    vm.ordinary_define_property(target, &k, desc)?;
                }
            }
            Ok(target)
        }
        "getOwnPropertyDescriptor" => {
            let key = args
                .get(1)
                .map(|v| vm.to_property_key(*v))
                .unwrap_or_default();
            vm.ordinary_property_descriptor(target, &key)
        }
        "getOwnPropertyNames" | "keys" => {
            let items: Vec<Value> = vm
                .own_properties(target)
                .into_iter()
                .map(|(k, _)| Value::Object(vm.alloc_string(k)))
                .collect();
            Ok(Value::Object(vm.alloc_array(items)))
        }
        "setPrototypeOf" => {
            let proto = args.get(1).copied().unwrap_or(Value::Undefined);
            let p = match proto {
                Value::Object(pr) => Some(pr),
                _ => None,
            };
            vm.set_prototype_of(target, p);
            Ok(target)
        }
        "getPrototypeOf" => Ok(vm
            .get_prototype(target)
            .map(Value::Object)
            .unwrap_or(Value::Null)),
        "assign" => {
            let out = target;
            for src in args.get(1..).unwrap_or(&[]) {
                for (k, v) in vm.own_properties(*src) {
                    vm.set_property(out, &k, v)?;
                }
            }
            Ok(out)
        }
        "freeze" | "seal" => Ok(target),
        "isFrozen" | "isSealed" => Ok(Value::Boolean(false)),
        "values" | "entries" => {
            let mut items = vm.own_properties(target);
            items.sort_by(|a, b| a.0.cmp(&b.0));
            let out = match method.as_str() {
                "values" => items.into_iter().map(|(_, v)| v).collect(),
                _ => items
                    .into_iter()
                    .map(|(k, v)| {
                        let key_str = vm.alloc_string(k);
                        Value::Object(vm.alloc_array(vec![Value::Object(key_str), v]))
                    })
                    .collect(),
            };
            Ok(Value::Object(vm.alloc_array(out)))
        }
        "fromEntries" => {
            let list = args
                .first()
                .copied()
                .map(|v| vm.to_array_values(v))
                .unwrap_or_default();
            let out = vm.alloc_ordinary();
            for pair in list {
                let vals = vm.to_array_values(pair);
                if vals.len() >= 2 {
                    let key = vm.to_property_key(vals[0]);
                    vm.set_property(Value::Object(out), &key, vals[1])?;
                }
            }
            Ok(Value::Object(out))
        }
        _ => Ok(Value::Undefined),
    }
}

/// `Error.captureStackTrace(obj[, ctorOpt])`：以通用调用点数组填充
/// `obj.stack`（文件名取入口文件，行号为已登记的降级 0）。
fn error_capture_stack_trace(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let Some(target) = args.first().copied() else {
        return Ok(Value::Undefined);
    };
    let mut frames = Vec::with_capacity(CALLSITE_FRAMES);
    for _ in 0..CALLSITE_FRAMES {
        let site = vm.alloc_ordinary();
        let ns = vm.alloc_string("callsite".to_owned());
        let _ = vm.set_property(Value::Object(site), "_builtinNs", Value::Object(ns));
        let file = vm.alloc_string(vm.entry_file.clone());
        let _ = vm.set_property(Value::Object(site), "_file", Value::Object(file));
        frames.push(Value::Object(site));
    }
    let stack_arr = Value::Object(vm.alloc_array(frames));
    vm.set_property(target, "stack", stack_arr)?;
    Ok(Value::Undefined)
}

/// 单帧数量（getStack().slice(1) 后仍需 stack[1] 有效）。
const CALLSITE_FRAMES: usize = 12;

/// 调用点对象方法（getFileName/getLineNumber/getColumnNumber/toString）。
fn callsite_method(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let method = match receiver {
        Value::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(HeapObject::NativeFn { name, .. }) => {
                name.clone().split('.').next_back().unwrap_or("").to_owned()
            }
            _ => String::new(),
        },
        _ => String::new(),
    };
    let file = match receiver {
        Value::Object(r) => vm
            .own_value(r.0 as usize, "_file")
            .map(|v| vm.format_value(v))
            .unwrap_or_default(),
        _ => String::new(),
    };
    match method.as_str() {
        "getFileName" => Ok(Value::Object(vm.alloc_string(file))),
        "getLineNumber" | "getColumnNumber" => Ok(Value::Number(0.0)),
        "isNative" | "isEval" | "isConstructor" => Ok(Value::Boolean(false)),
        "getFunctionName" | "getTypeName" => Ok(Value::Undefined),
        "toString" => Ok(Value::Object(
            vm.alloc_string(format!("at <anonymous> ({file})")),
        )),
        _ => Ok(Value::Undefined),
    }
}

// ---- M4: Fetch API + AbortController ----

/// `fetch(url[, options]) -> Promise<Response>`：同步 HTTP 请求后
/// 构造 Response 对象并以 Promise 包装返回。
/// `Response.text()` handler：返回 _bodyText 属性的文本内容。
fn response_text_handler(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    vm.get_property(receiver, "_bodyText")
}

/// `Response.json()` handler：解析 _bodyText 为 JSON 并返回解析结果。
fn response_json_handler(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let body = vm.get_property(receiver, "_bodyText")?;
    let text = vm.format_value(body);
    let str_ref = vm.alloc_string(text);
    vm.json_parse(&[Value::Object(str_ref)])
}

/// `Response.arrayBuffer()` handler：返回 _bodyText 的字节数组。
#[allow(dead_code)]
fn response_array_buffer_handler(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let body = vm.get_property(receiver, "_bodyText")?;
    let text = vm.format_value(body);
    let bytes = text.as_bytes().to_vec();
    let nums: Vec<Value> = bytes.iter().map(|&b| Value::Number(b as f64)).collect();
    Ok(Value::Object(vm.alloc_array(nums)))
}

fn global_fetch(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let url_val = args.first().copied().unwrap_or(Value::Undefined);
    let opts = args.get(1).copied().unwrap_or(Value::Undefined);

    // M4.1：Request 对象形态 + init 归并
    let input = parse_fetch_input(vm, url_val, opts)?;
    let FetchInput {
        url,
        method,
        headers,
        body,
        signal,
        redirect,
    } = input;

    // AbortSignal 前置检查
    if let Value::Object(sig_ref) = signal {
        if let Ok(Value::Boolean(true)) = vm.get_property(Value::Object(sig_ref), "aborted") {
            let reason = match vm.get_property(Value::Object(sig_ref), "reason") {
                Ok(r) if !matches!(r, Value::Undefined) => r,
                _ => default_abort_error(vm),
            };
            let promise = vm.alloc_rejected_promise(reason);
            return Ok(Value::Object(promise));
        }
    }

    // M4.1：https:// 显式拒绝（无 TLS 栈——rejected TypeError 兜底，
    // 不再静默走明文 TcpStream）
    if url.starts_with("https://") {
        let err = vm.alloc_error_instance("fetch: https is not supported yet");
        let name = vm.alloc_string("TypeError".to_owned());
        let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
        let promise = vm.alloc_rejected_promise(Value::Object(err));
        return Ok(Value::Object(promise));
    }

    // 同步 HTTP 请求（follow 模式下 3xx Location 跟随 ≤5 跳）
    let mut current_url = url;
    let mut result = None;
    let mut followed_redirect = false;
    for _ in 0..5 {
        let attempt = do_sync_http_request(vm, &current_url, &method, &headers, body.as_ref());
        match attempt {
            Err(message) => {
                let err = vm.alloc_error_instance(&message);
                let name = vm.alloc_string("TypeError".to_owned());
                let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
                let promise = vm.alloc_rejected_promise(Value::Object(err));
                return Ok(Value::Object(promise));
            }
            Ok((status, headers_text, body_text)) => {
                // 解析 Location（跟随重定向判定；展示头在响应构造处统一解析）
                let (_, location) = parse_response_headers(&headers_text);
                let is_redirect = (300..400).contains(&status) && location.is_some();
                if is_redirect {
                    match redirect.as_str() {
                        "manual" => {
                            result = Some((status, headers_text, body_text));
                            break;
                        }
                        "error" => {
                            let err = vm.alloc_error_instance(&format!(
                                "fetch: redirect mode 'error' blocked redirect to {location:?}"
                            ));
                            let name = vm.alloc_string("TypeError".to_owned());
                            let _ =
                                vm.set_property(Value::Object(err), "name", Value::Object(name));
                            let promise = vm.alloc_rejected_promise(Value::Object(err));
                            return Ok(Value::Object(promise));
                        }
                        _ => {
                            // follow：拼接 Location（相对路径基于当前 URL）
                            if let Some(loc) = &location {
                                current_url = resolve_redirect_url(&current_url, loc);
                                followed_redirect = true;
                                continue;
                            }
                        }
                    }
                }
                result = Some((status, headers_text, body_text));
                break;
            }
        }
    }
    let Some((status, headers_text, body_text)) = result else {
        let err = vm.alloc_error_instance("fetch: too many redirects");
        let name = vm.alloc_string("TypeError".to_owned());
        let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
        let promise = vm.alloc_rejected_promise(Value::Object(err));
        return Ok(Value::Object(promise));
    };
    // 展示头对象（小写规范键）
    let (hdr_pairs, _) = parse_response_headers(&headers_text);

    // AbortSignal 后置检查：signal 在请求发起后（前序宏任务中）被 abort →
    // 兑现为携带 reason（缺省 AbortError）的 rejected promise，不返回 Response
    if let Value::Object(sig_ref) = signal {
        if let Ok(Value::Boolean(true)) = vm.get_property(Value::Object(sig_ref), "aborted") {
            let reason = match vm.get_property(Value::Object(sig_ref), "reason") {
                Ok(r) if !matches!(r, Value::Undefined) => r,
                _ => default_abort_error(vm),
            };
            let promise = vm.alloc_rejected_promise(reason);
            return Ok(Value::Object(promise));
        }
    }

    // 构造 Response 对象
    let response = vm.alloc_ordinary();
    let _ = vm.set_property(
        Value::Object(response),
        "status",
        Value::Number(status as f64),
    );
    let _ = vm.set_property(
        Value::Object(response),
        "ok",
        Value::Boolean((200..300).contains(&status)),
    );
    let body_ref = vm.alloc_string(body_text.clone());
    let _ = vm.set_property(
        Value::Object(response),
        "_bodyText",
        Value::Object(body_ref),
    );
    let _ = vm.set_property(Value::Object(response), "_isResponse", Value::Boolean(true));

    // M4.1：headers 展示面（小写键头对象 + headers.get/has 分派面）
    let headers_obj = vm.alloc_ordinary();
    for (k, v) in &hdr_pairs {
        let v_ref = vm.alloc_string(v.clone());
        let _ = vm.set_property(
            Value::Object(headers_obj),
            &k.to_ascii_lowercase(),
            Value::Object(v_ref),
        );
    }
    let _ = vm.set_property(
        Value::Object(headers_obj),
        "_isHeaders",
        Value::Boolean(true),
    );
    let ns_val = vm.alloc_string("Headers".to_owned());
    let _ = vm.set_property(
        Value::Object(headers_obj),
        "_builtinNs",
        Value::Object(ns_val),
    );
    let get_fn = vm.alloc_native_fn("Headers.get");
    let _ = vm.set_property(Value::Object(headers_obj), "get", Value::Object(get_fn));
    let has_fn = vm.alloc_native_fn("Headers.has");
    let _ = vm.set_property(Value::Object(headers_obj), "has", Value::Object(has_fn));
    let _ = vm.set_property(
        Value::Object(response),
        "headers",
        Value::Object(headers_obj),
    );
    // 原始 content-type 备份（formData 解析用）
    let ctype = hdr_pairs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let ctype_ref = vm.alloc_string(ctype);
    let _ = vm.set_property(
        Value::Object(response),
        "_contentType",
        Value::Object(ctype_ref),
    );
    // 重定向标记（Node 语义：请求途中实际跟随过 3xx 跳转才 true；
    // manual 模式返回的 3xx 响应本身未跟随 → false）
    let _ = vm.set_property(
        Value::Object(response),
        "redirected",
        Value::Boolean(followed_redirect),
    );

    // .text() 方法：返回 body 文本
    let text_fn = vm.alloc_native_fn("Response.text");
    let _ = vm.set_property(Value::Object(response), "text", Value::Object(text_fn));
    // .json() 方法
    let json_fn = vm.alloc_native_fn("Response.json");
    let _ = vm.set_property(Value::Object(response), "json", Value::Object(json_fn));
    // .arrayBuffer() / .formData()
    let ab_fn = vm.alloc_native_fn("Response.arrayBuffer");
    let _ = vm.set_property(Value::Object(response), "arrayBuffer", Value::Object(ab_fn));
    let fd_fn = vm.alloc_native_fn("Response.formData");
    let _ = vm.set_property(Value::Object(response), "formData", Value::Object(fd_fn));

    // Promise<Response>
    let promise = vm.alloc_fulfilled_promise(Value::Object(response));
    Ok(Value::Object(promise))
}

/// 解析响应头块（CRLF 行）→ (键值对, Location)。
fn parse_response_headers(header_block: &str) -> (Vec<(String, String)>, Option<String>) {
    let mut pairs = Vec::new();
    let mut location = None;
    for line in header_block.split("\r\n").skip(1) {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim();
            pairs.push((k.trim().to_owned(), v.to_owned()));
            if k.trim().eq_ignore_ascii_case("location") {
                location = Some(v.to_owned());
            }
        }
    }
    (pairs, location)
}

/// 重定向 URL 拼接（绝对 URL 直用；相对路径基于当前 URL 归并）。
fn resolve_redirect_url(current: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_owned();
    }
    // 剥 scheme + host
    let rest = current
        .strip_prefix("http://")
        .or_else(|| current.strip_prefix("https://"))
        .unwrap_or(current);
    let (host_port, cur_path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let base = if location.starts_with('/') {
        format!("http://{host_port}")
    } else {
        // 目录相对：取当前路径的目录前缀
        let dir = match cur_path.rfind('/') {
            Some(i) => &cur_path[..i + 1],
            None => "/",
        };
        format!("http://{host_port}{dir}")
    };
    let location_trimmed = location.trim_start_matches('/');
    format!("{base}/{location_trimmed}")
}

/// 同步 HTTP 请求 → (status, 原始头块, body_text)
fn do_sync_http_request(
    vm: &mut Vm,
    url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Option<&Value>,
) -> Result<(u16, String, String), String> {
    let (host, port, path) = parse_http_url(url);

    use std::io::{Read as _, Write as _};
    use std::net::TcpStream;
    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect(&addr).map_err(|e| format!("fetch: connect: {e}"))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .ok();
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(10)))
        .ok();

    // HTTP/1.1 报文行必须 CRLF 结尾（RFC 9112；LF-only 会被 Node/严格
    // 解析器以 400 拒绝，自研 http server 同样按 CRLF 分帧）
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (k, v) in headers {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        let bs = vm.format_value(*b);
        request.push_str(&format!("Content-Length: {}\r\n", bs.len()));
    }
    request.push_str("\r\n");
    if let Some(b) = body {
        request.push_str(&vm.format_value(*b));
    }
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("fetch: write: {e}"))?;

    let mut response_bytes = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => response_bytes.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }

    let text = String::from_utf8_lossy(&response_bytes).to_string();
    // 头/体分隔与状态行均按 CRLF 分帧（RFC 9112）；兼容 LF-only 响应
    let (header_block, raw_body) = match text.find("\r\n\r\n") {
        Some(i) => (text[..i].to_owned(), text[i + 4..].to_owned()),
        None => match text.find("\n\n") {
            Some(i) => (text[..i].to_owned(), text[i + 2..].to_owned()),
            None => (text.clone(), String::new()),
        },
    };
    let status_line = header_block.split("\r\n").next().unwrap_or(&header_block);
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // M4.1：chunked 传输编码解码（Transfer-Encoding: chunked——Node http
    // server 的默认分帧；未解码时 body 混入 chunk 尺寸行与尾帧）
    let body_text = if header_block
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        decode_chunked_body(&raw_body)
    } else {
        raw_body
    };

    Ok((status, header_block, body_text))
}

/// 解码 chunked 报文体：按 `{hex-size}\r\n{data}\r\n` 帧序列拼接，遇
/// `0\r\n` 终止（尾帧扩展块与 trailer 一并忽略）。
fn decode_chunked_body(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(line_end) = rest.find("\r\n") {
        // 帧头行：hex 尺寸（可带扩展；分号后忽略）
        let size_line = &rest[..line_end];
        let size_token = size_line.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_token, 16) else {
            break;
        };
        if size == 0 {
            break;
        }
        let data_start = line_end + 2;
        let data_end = (data_start + size).min(rest.len());
        let chunk = &rest[data_start..data_end];
        let truncated = chunk.len() < size;
        out.push_str(chunk);
        // 跳过帧尾 CRLF；防御性截断（截断帧按已收内容处理）
        let next = (data_end + 2).min(rest.len());
        rest = &rest[next..];
        if truncated {
            break;
        }
    }
    out
}

/// 解析 HTTP(S) URL → (host, port, path)
fn parse_http_url(url: &str) -> (String, u16, String) {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let (host_port, path) = match rest.find('/') {
        Some(i) => (rest[..i].to_owned(), rest[i..].to_owned()),
        None => (rest.to_owned(), "/".to_owned()),
    };
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (h.to_owned(), p.parse().unwrap_or(80)),
        None => (host_port, 80),
    };
    (host, port, path)
}

/// `AbortController` 构造器：创建 { signal: { aborted, reason, _abortId }, abort() }
/// 对象；signal 携带 addEventListener / removeEventListener（M4 abort 联动面）。
static ABORT_ID_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

fn next_abort_id() -> u32 {
    ABORT_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

fn abort_controller_ctor_impl(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let abort_id = crate::builtins::global_fns::next_abort_id();
    let controller = vm.alloc_ordinary();
    let signal = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(signal), "aborted", Value::Boolean(false));
    let _ = vm.set_property(
        Value::Object(signal),
        "_abortId",
        Value::Number(abort_id as f64),
    );
    let _ = vm.set_property(
        Value::Object(signal),
        "_isAbortSignal",
        Value::Boolean(true),
    );
    let listeners = vm.alloc_array(Vec::new());
    let _ = vm.set_property(
        Value::Object(signal),
        "_listeners",
        Value::Object(listeners),
    );
    for method in ["addEventListener", "removeEventListener"] {
        let fn_ref = vm.alloc_native_fn(&format!("AbortSignal.{method}"));
        let _ = vm.set_property(Value::Object(signal), method, Value::Object(fn_ref));
    }
    let abort_fn = vm.alloc_native_fn("AbortSignal.abort");
    let _ = vm.set_property(Value::Object(signal), "abort", Value::Object(abort_fn));
    let _ = vm.set_property(Value::Object(controller), "signal", Value::Object(signal));
    let abort_method = vm.alloc_native_fn("AbortController.abort");
    let _ = vm.set_property(
        Value::Object(controller),
        "abort",
        Value::Object(abort_method),
    );
    Ok(Value::Object(controller))
}

/// 构造缺省 AbortError（`name: "AbortError"`）。
fn default_abort_error(vm: &mut Vm) -> Value {
    let err = vm.alloc_error_instance("This operation was aborted");
    let name = vm.alloc_string("AbortError".to_owned());
    let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
    Value::Object(err)
}

/// abort 统一路径：幂等置位 `aborted`、写 `reason`、同步触发 'abort' 监听器
/// （事件对象 `{ type: "abort" }`，Node 22 LTS EventTarget 语义对齐面）。
fn apply_abort(vm: &mut Vm, signal: Value, reason: Value) -> Result<(), VmError> {
    if let Ok(Value::Boolean(true)) = vm.get_property(signal, "aborted") {
        return Ok(());
    }
    let _ = vm.set_property(signal, "aborted", Value::Boolean(true));
    let reason = if matches!(reason, Value::Undefined) {
        default_abort_error(vm)
    } else {
        reason
    };
    let _ = vm.set_property(signal, "reason", reason);
    // 触发已登记监听器（_listeners 数组；堆数组直读）
    if let Ok(Value::Object(arr)) = vm.get_property(signal, "_listeners") {
        let elements: Vec<Value> = match vm.heap.get(arr.0 as usize) {
            Some(crate::heap::HeapObject::Array { elements, .. }) => elements.clone(),
            _ => Vec::new(),
        };
        if !elements.is_empty() {
            let event = vm.alloc_ordinary();
            let type_str = vm.alloc_string("abort".to_owned());
            let _ = vm.set_property(Value::Object(event), "type", Value::Object(type_str));
            let event_val = Value::Object(event);
            for cb in elements {
                let _ = vm.invoke_callable(cb, Value::Undefined, &[event_val]);
            }
        }
    }
    Ok(())
}

/// `signal.addEventListener(type, cb)`：登记 'abort' 监听器（仅 type="abort" 生效）。
fn signal_add_event_listener(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this = current_receiver();
    let ty = args.first().copied().unwrap_or(Value::Undefined);
    if vm.format_value(ty) != "abort" {
        return Ok(Value::Undefined);
    }
    let Some(cb) = args.get(1).copied() else {
        return Ok(Value::Undefined);
    };
    let arr = match vm.get_property(this, "_listeners") {
        Ok(Value::Object(r)) => r,
        _ => vm.alloc_array(Vec::new()),
    };
    if let Some(crate::heap::HeapObject::Array { elements, .. }) = vm.heap.get_mut(arr.0 as usize) {
        elements.push(cb);
    }
    Ok(Value::Undefined)
}

/// `signal.removeEventListener(type, cb)`：移除首个匹配监听器。
fn signal_remove_event_listener(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this = current_receiver();
    let ty = args.first().copied().unwrap_or(Value::Undefined);
    if vm.format_value(ty) != "abort" {
        return Ok(Value::Undefined);
    }
    let Some(cb) = args.get(1).copied() else {
        return Ok(Value::Undefined);
    };
    if let Ok(Value::Object(arr)) = vm.get_property(this, "_listeners") {
        if let Some(crate::heap::HeapObject::Array { elements, .. }) =
            vm.heap.get_mut(arr.0 as usize)
        {
            if let Some(pos) = elements.iter().position(|e| e == &cb) {
                elements.remove(pos);
            }
        }
    }
    Ok(Value::Undefined)
}

/// `signal.abort(reason)` handler：接收者即 signal。
fn signal_abort_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let signal = current_receiver();
    let reason = args.first().copied().unwrap_or(Value::Undefined);
    apply_abort(vm, signal, reason)?;
    Ok(Value::Undefined)
}

/// `controller.abort(reason)` handler：接收者是 controller，委托其 signal。
fn controller_abort_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let controller = current_receiver();
    let signal = vm.get_property(controller, "signal")?;
    let reason = args.first().copied().unwrap_or(Value::Undefined);
    apply_abort(vm, signal, reason)?;
    Ok(Value::Undefined)
}

/// `Headers` 构造器：创建空 Headers 对象。
fn headers_ctor_impl(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let headers = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(headers), "_isHeaders", Value::Boolean(true));
    // get/has 方法面（大小写不敏感查值；Node Headers 规范）
    let get_fn = vm.alloc_native_fn("Headers.get");
    let _ = vm.set_property(Value::Object(headers), "get", Value::Object(get_fn));
    let has_fn = vm.alloc_native_fn("Headers.has");
    let _ = vm.set_property(Value::Object(headers), "has", Value::Object(has_fn));
    Ok(Value::Object(headers))
}

/// `Headers.get(name)`：大小写不敏感取头值（无则 null）。
fn headers_get_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Some(name) = args.first().map(|v| vm.format_value(*v)) else {
        return Ok(Value::Null);
    };
    if let Value::Object(r) = receiver {
        for (k, v) in vm.own_entries(r.index()) {
            if k.eq_ignore_ascii_case(&name) && !k.starts_with('_') {
                let text = vm.format_value(v);
                let s = vm.alloc_string(text);
                return Ok(Value::Object(s));
            }
        }
    }
    Ok(Value::Null)
}

/// `Headers.has(name)`：大小写不敏感存在性。
fn headers_has_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let Some(name) = args.first().map(|v| vm.format_value(*v)) else {
        return Ok(Value::Boolean(false));
    };
    if let Value::Object(r) = receiver {
        for (k, _) in vm.own_entries(r.index()) {
            if k.eq_ignore_ascii_case(&name) && !k.starts_with('_') {
                return Ok(Value::Boolean(true));
            }
        }
    }
    Ok(Value::Boolean(false))
}

// ---------------------------------------------------------------------------
// M4.1 Request / 重定向 / Response.headers
// ---------------------------------------------------------------------------

/// `new Request(input[, options])`：URL 字符串或既有 Request + init 形态。
/// options: { method, headers, body, signal, redirect }——统一落到实例属性，
/// `fetch(request)` 直传时按属性读回。
fn request_ctor_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let input = args.first().copied().unwrap_or(Value::Undefined);
    // input 为既有 Request → 继承其属性（init 覆盖）
    let (url, inherited) = if let Value::Object(_) = input {
        if let Ok(Value::Boolean(true)) = vm.get_property(input, "_isRequest") {
            let u = vm.get_property(input, "url")?;
            (u, Some(input))
        } else {
            (input, None)
        }
    } else {
        (input, None)
    };

    let req = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(req), "_isRequest", Value::Boolean(true));
    let _ = vm.set_property(Value::Object(req), "url", url);

    let opts = args.get(1).copied().unwrap_or(Value::Undefined);

    // method（继承 → init）
    let method = vm
        .get_property(opts, "method")
        .ok()
        .filter(|v| !matches!(v, Value::Undefined))
        .or_else(|| {
            inherited
                .and_then(|i| vm.get_property(i, "method").ok())
                .filter(|v| !matches!(v, Value::Undefined))
        })
        .unwrap_or(Value::Undefined);
    let _ = vm.set_property(Value::Object(req), "method", method);

    // headers（继承 → init）
    let headers = vm
        .get_property(opts, "headers")
        .ok()
        .filter(|v| !matches!(v, Value::Undefined))
        .or_else(|| {
            inherited
                .and_then(|i| vm.get_property(i, "headers").ok())
                .filter(|v| !matches!(v, Value::Undefined))
        })
        .unwrap_or(Value::Undefined);
    let _ = vm.set_property(Value::Object(req), "headers", headers);

    // body（继承 → init）
    let body = vm
        .get_property(opts, "body")
        .ok()
        .filter(|v| !matches!(v, Value::Undefined | Value::Null))
        .or_else(|| {
            inherited
                .and_then(|i| vm.get_property(i, "body").ok())
                .filter(|v| !matches!(v, Value::Undefined | Value::Null))
        })
        .unwrap_or(Value::Undefined);
    let _ = vm.set_property(Value::Object(req), "body", body);

    // signal（不继承——Request 规范：signal 属 init 专属）
    let signal = vm
        .get_property(opts, "signal")
        .ok()
        .filter(|v| !matches!(v, Value::Undefined))
        .unwrap_or(Value::Undefined);
    let _ = vm.set_property(Value::Object(req), "signal", signal);

    // redirect 模式（默认 follow）
    let redirect = vm
        .get_property(opts, "redirect")
        .ok()
        .map(|v| vm.format_value(v))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "follow".to_owned());
    let redirect_val = vm.alloc_string(redirect);
    let _ = vm.set_property(Value::Object(req), "redirect", Value::Object(redirect_val));

    // GET/HEAD 携带 body → TypeError（fetch 规范）
    let method_text = vm
        .get_property(Value::Object(req), "method")
        .map(|v| vm.format_value(v))
        .unwrap_or_default()
        .to_uppercase();
    if matches!(method_text.as_str(), "GET" | "HEAD")
        && !matches!(body, Value::Undefined | Value::Null)
    {
        return Err(VmError::Thrown(Value::Object(vm.alloc_error_instance(
            "Request constructor: GET/HEAD request cannot have a body",
        ))));
    }

    Ok(Value::Object(req))
}

/// `request.clone()`：返回自身（简化——同步请求模型无共享状态问题）。
fn request_noop_self(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(current_receiver())
}

/// 解析 fetch 输入形态：URL 字符串 / Request 对象 → (url, method, headers, body, signal, redirect)
struct FetchInput {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<Value>,
    signal: Value,
    redirect: String,
}

/// 从 fetch 第一参 + options 归并请求参数（Request 属性为底、init 覆盖）。
fn parse_fetch_input(vm: &mut Vm, first: Value, opts: Value) -> Result<FetchInput, VmError> {
    // Request 对象形态：init 可覆盖其属性
    let is_request = matches!(first, Value::Object(_))
        && matches!(
            vm.get_property(first, "_isRequest"),
            Ok(Value::Boolean(true))
        );

    let url = if is_request {
        vm.get_property(first, "url")
            .map(|v| vm.format_value(v))
            .unwrap_or_default()
    } else {
        vm.format_value(first)
    };

    let mut method = if is_request {
        vm.get_property(first, "method")
            .map(|v| vm.format_value(v))
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "GET".to_owned())
    } else {
        "GET".to_owned()
    };
    if let Ok(m) = vm.get_property(opts, "method") {
        let ms = vm.format_value(m);
        if !ms.is_empty() {
            method = ms.to_uppercase();
        }
    }

    // headers：Request 属性 → init 追加覆盖
    let mut headers: Vec<(String, String)> = Vec::new();
    if is_request {
        if let Ok(Value::Object(ho)) = vm.get_property(first, "headers") {
            for (k, v) in vm.own_entries(ho.0 as usize) {
                headers.push((k, vm.format_value(v)));
            }
        }
    }
    if let Ok(Value::Object(hdr_obj)) = vm.get_property(opts, "headers") {
        for (k, v) in vm.own_entries(hdr_obj.0 as usize) {
            if let Some(pos) = headers
                .iter()
                .position(|(hk, _)| hk.eq_ignore_ascii_case(&k))
            {
                headers[pos].1 = vm.format_value(v);
            } else {
                headers.push((k, vm.format_value(v)));
            }
        }
    }

    let mut body = if is_request {
        vm.get_property(first, "body")
            .ok()
            .filter(|v| !matches!(v, Value::Undefined | Value::Null))
    } else {
        None
    };
    if let Ok(b) = vm.get_property(opts, "body") {
        if !matches!(b, Value::Undefined | Value::Null) {
            body = Some(b);
        }
    }

    let mut signal = if is_request {
        vm.get_property(first, "signal").unwrap_or(Value::Undefined)
    } else {
        Value::Undefined
    };
    if let Ok(s) = vm.get_property(opts, "signal") {
        if !matches!(s, Value::Undefined) {
            signal = s;
        }
    }

    let mut redirect = if is_request {
        vm.get_property(first, "redirect")
            .map(|v| vm.format_value(v))
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "follow".to_owned())
    } else {
        "follow".to_owned()
    };
    if let Ok(r) = vm.get_property(opts, "redirect") {
        let rs = vm.format_value(r);
        if !rs.is_empty() {
            redirect = rs;
        }
    }

    Ok(FetchInput {
        url,
        method,
        headers,
        body,
        signal,
        redirect,
    })
}

/// `Response.formData()` handler：解析 multipart 或 urlencoded body 为 FormData。
fn response_form_data_handler(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let ctype = vm
        .get_property(receiver, "_contentType")
        .map(|v| vm.format_value(v))
        .unwrap_or_default();
    let body = vm.get_property(receiver, "_bodyText")?;
    let text = vm.format_value(body);
    parse_form_data_body(vm, &ctype, &text)
}

// ---------------------------------------------------------------------------
// M4.4 EventTarget / CustomEvent
// ---------------------------------------------------------------------------

/// `new EventTarget()`：监听器注册表（_etListeners: { type: [cb] }）。
fn event_target_ctor_impl(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let target = vm.alloc_ordinary();
    let _ = vm.set_property(
        Value::Object(target),
        "_isEventTarget",
        Value::Boolean(true),
    );
    // 分派键：_builtinNs → "EventTarget.{method}"（try_dispatch 形态二）
    let ns = vm.alloc_string("EventTarget".to_owned());
    let _ = vm.set_property(Value::Object(target), "_builtinNs", Value::Object(ns));
    let map = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(target), "_etListeners", Value::Object(map));
    // addEventListener / removeEventListener / dispatchEvent 方法面
    for method in ["addEventListener", "removeEventListener", "dispatchEvent"] {
        let f = vm.alloc_native_fn(&format!("EventTarget.{method}"));
        let _ = vm.set_property(Value::Object(target), method, Value::Object(f));
    }
    Ok(Value::Object(target))
}

/// EventTarget 三方法统一分派（addEventListener / removeEventListener / dispatchEvent）。
fn event_target_dispatch(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = crate::builtins::pending_native_name();
    let target = current_receiver();
    // 事件名：addEventListener/removeEventListener 第一参为字符串；
    // dispatchEvent 第一参为事件对象（读其 type 属性）——统一归并为
    // 字符串事件名后操作监听器表。
    let first = args.first().copied().unwrap_or(Value::Undefined);
    let ev = if let Ok(Value::Object(_)) = vm.get_property(first, "type") {
        vm.get_property(first, "type")
            .map(|v| vm.format_value(v))
            .unwrap_or_default()
    } else {
        vm.format_value(first)
    };
    let cb = args.get(1).copied();
    let map = match vm.get_property(target, "_etListeners") {
        Ok(Value::Object(m)) => m,
        _ => return Ok(Value::Undefined),
    };
    let arr = match vm.get_property(Value::Object(map), &ev) {
        Ok(Value::Object(a)) => a,
        _ => {
            // 未命中：addEventListener 创建监听数组；其余无监听器直接返回
            if name.ends_with("addEventListener") {
                let new_arr = vm.alloc_array(Vec::new());
                let _ = vm.set_property(Value::Object(map), &ev, Value::Object(new_arr));
                new_arr
            } else {
                return Ok(match name.as_str() {
                    "EventTarget.dispatchEvent" => Value::Boolean(false),
                    _ => Value::Undefined,
                });
            }
        }
    };
    match name.as_str() {
        "EventTarget.addEventListener" => {
            if let Some(cb) = cb.filter(|v| is_callable(vm, *v)) {
                if let Some(crate::heap::HeapObject::Array { elements, .. }) =
                    vm.heap.get_mut(arr.0 as usize)
                {
                    if !elements.contains(&cb) {
                        elements.push(cb);
                    }
                }
            }
            Ok(Value::Undefined)
        }
        "EventTarget.removeEventListener" => {
            if let Some(cb) = cb {
                if let Some(crate::heap::HeapObject::Array { elements, .. }) =
                    vm.heap.get_mut(arr.0 as usize)
                {
                    elements.retain(
                        |e| !matches!((e, &cb), (Value::Object(a), Value::Object(b)) if a == b),
                    );
                }
            }
            Ok(Value::Undefined)
        }
        _ => {
            // dispatchEvent：派发事件对象（type 命中则调用监听器，返回是否有监听器）
            let callbacks: Vec<Value> = match vm.heap.get(arr.0 as usize) {
                Some(crate::heap::HeapObject::Array { elements, .. }) => elements.clone(),
                _ => Vec::new(),
            };
            let has = !callbacks.is_empty();
            // target 属性注入（Event 标准面）
            if let Value::Object(e) = args.first().copied().unwrap_or(Value::Undefined) {
                let _ = vm.set_property(Value::Object(e), "target", target);
            }
            let event_val = args.first().copied().unwrap_or(Value::Undefined);
            for cb in callbacks {
                let _ = vm.invoke_callable(cb, Value::Undefined, &[event_val]);
            }
            Ok(Value::Boolean(has))
        }
    }
}

/// `new CustomEvent(type[, options])`：{ type, detail, ... }。
fn custom_event_ctor_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let ty = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let event = vm.alloc_ordinary();
    let ty_val = vm.alloc_string(ty);
    let _ = vm.set_property(Value::Object(event), "type", Value::Object(ty_val));
    let opts = args.get(1).copied().unwrap_or(Value::Undefined);
    let detail = vm.get_property(opts, "detail").unwrap_or(Value::Undefined);
    let _ = vm.set_property(Value::Object(event), "detail", detail);
    let _ = vm.set_property(Value::Object(event), "_isCustomEvent", Value::Boolean(true));
    Ok(Value::Object(event))
}

// ---------------------------------------------------------------------------
// M4.4 FormData + multipart/form-data
// ---------------------------------------------------------------------------

/// FormData 实例的内部形态：`_fdEntries: [{ name, value, filename? }]`（堆数组）。
///（字符串键值序对即可覆盖 Node 22 差分面；文件面后续以 filename 扩展）
///
/// `new FormData()`：创建空表单（分派键 `_builtinNs` → `FormData.{method}`）。
fn form_data_ctor_impl(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let fd = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(fd), "_isFormData", Value::Boolean(true));
    // 分派键：_builtinNs → "FormData.{method}"
    let ns = vm.alloc_string("FormData".to_owned());
    let _ = vm.set_property(Value::Object(fd), "_builtinNs", Value::Object(ns));
    let entries = vm.alloc_array(Vec::new());
    let _ = vm.set_property(Value::Object(fd), "_fdEntries", Value::Object(entries));
    for method in [
        "append", "set", "get", "getAll", "has", "delete", "entries", "keys", "values", "forEach",
    ] {
        let f = vm.alloc_native_fn(&format!("FormData.{method}"));
        let _ = vm.set_property(Value::Object(fd), method, Value::Object(f));
    }
    Ok(Value::Object(fd))
}

/// 读取 FormData 条目快照 [(name, value)]。
fn fd_entries(vm: &mut Vm, receiver: Value) -> Vec<(String, String)> {
    let Ok(Value::Object(arr)) = vm.get_property(receiver, "_fdEntries") else {
        return Vec::new();
    };
    let elements: Vec<Value> = match vm.heap.get(arr.0 as usize) {
        Some(crate::heap::HeapObject::Array { elements, .. }) => elements.clone(),
        _ => Vec::new(),
    };
    elements
        .iter()
        .filter_map(|e| {
            let Value::Object(_) = e else {
                return None;
            };
            let name = vm
                .get_property(*e, "name")
                .map(|v| vm.format_value(v))
                .unwrap_or_default();
            let value = vm
                .get_property(*e, "value")
                .map(|v| vm.format_value(v))
                .unwrap_or_default();
            Some((name, value))
        })
        .collect()
}

/// 修改 FormData 条目（重建 _fdEntries 数组）。
fn fd_rewrite(vm: &mut Vm, receiver: Value, entries: Vec<(String, String)>) {
    let vals: Vec<Value> = entries
        .into_iter()
        .map(|(name, value)| {
            let entry = vm.alloc_ordinary();
            let n = vm.alloc_string(name);
            let _ = vm.set_property(Value::Object(entry), "name", Value::Object(n));
            let v = vm.alloc_string(value);
            let _ = vm.set_property(Value::Object(entry), "value", Value::Object(v));
            Value::Object(entry)
        })
        .collect();
    let arr = vm.alloc_array(vals);
    let _ = vm.set_property(receiver, "_fdEntries", Value::Object(arr));
}

/// FormData 全方法统一分派（按 pending_native_name 区分）。
fn form_data_method(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let name = crate::builtins::pending_native_name();
    let receiver = current_receiver();
    let Some(key) = args.first().map(|v| vm.format_value(*v)) else {
        return Ok(Value::Undefined);
    };
    let mut entries = fd_entries(vm, receiver);
    match name.as_str() {
        "FormData.append" => {
            let value = args.get(1).map(|v| vm.format_value(*v)).unwrap_or_default();
            entries.push((key, value));
            fd_rewrite(vm, receiver, entries);
            Ok(Value::Undefined)
        }
        "FormData.set" => {
            let value = args.get(1).map(|v| vm.format_value(*v)).unwrap_or_default();
            // Node 语义：set 替换既有键的原位置（无则追加末尾）
            match entries.iter_mut().find(|(k, _)| k == &key) {
                Some(slot) => slot.1 = value,
                None => entries.push((key, value)),
            }
            fd_rewrite(vm, receiver, entries);
            Ok(Value::Undefined)
        }
        "FormData.get" => Ok(entries
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, v)| {
                let s = vm.alloc_string(v.clone());
                Value::Object(s)
            })
            .unwrap_or(Value::Null)),
        "FormData.getAll" => {
            let vals: Vec<Value> = entries
                .iter()
                .filter(|(k, _)| k == &key)
                .map(|(_, v)| {
                    let s = vm.alloc_string(v.clone());
                    Value::Object(s)
                })
                .collect();
            Ok(Value::Object(vm.alloc_array(vals)))
        }
        "FormData.has" => Ok(Value::Boolean(entries.iter().any(|(k, _)| k == &key))),
        "FormData.delete" => {
            entries.retain(|(k, _)| k != &key);
            fd_rewrite(vm, receiver, entries);
            Ok(Value::Undefined)
        }
        "FormData.entries" => {
            let pairs: Vec<Value> = entries
                .iter()
                .map(|(k, v)| {
                    let ks = vm.alloc_string(k.clone());
                    let vs = vm.alloc_string(v.clone());
                    let pair = vm.alloc_array(vec![Value::Object(ks), Value::Object(vs)]);
                    Value::Object(pair)
                })
                .collect();
            Ok(Value::Object(vm.alloc_array(pairs)))
        }
        "FormData.keys" => {
            let mut keys: Vec<String> = Vec::new();
            for (k, _) in &entries {
                if !keys.contains(k) {
                    keys.push(k.clone());
                }
            }
            let vals: Vec<Value> = keys
                .into_iter()
                .map(|k| {
                    let s = vm.alloc_string(k);
                    Value::Object(s)
                })
                .collect();
            Ok(Value::Object(vm.alloc_array(vals)))
        }
        "FormData.values" => {
            let vals: Vec<Value> = entries
                .iter()
                .map(|(_, v)| {
                    let s = vm.alloc_string(v.clone());
                    Value::Object(s)
                })
                .collect();
            Ok(Value::Object(vm.alloc_array(vals)))
        }
        _ => {
            // FormData.forEach(cb[, thisArg])——首参即回调（无键参）
            let Some(cb) = args.first().copied().filter(|v| is_callable(vm, *v)) else {
                return Ok(Value::Undefined);
            };
            let this_arg = args.get(1).copied().unwrap_or(Value::Undefined);
            for (k, v) in entries {
                let vs = vm.alloc_string(v);
                let ks = vm.alloc_string(k);
                let _ = vm.invoke_callable(
                    cb,
                    this_arg,
                    &[Value::Object(vs), Value::Object(ks), receiver],
                );
            }
            Ok(Value::Undefined)
        }
    }
}

/// 判断值是否可调用（Closure / NativeFn / NativeCtor）——复用 readline 面。
fn is_callable(vm: &Vm, v: Value) -> bool {
    crate::builtins::readline::is_callable_value(vm, v)
}

/// multipart/form-data 边界生成（Node 语义近似：随机 12 段十六进制）。
fn generate_boundary() -> String {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5eed_1234_abcd_5678);
    let mut out = String::new();
    for _ in 0..12 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.push(char::from_digit((seed >> 33) as u32 % 16, 16).unwrap_or('0'));
    }
    out
}

/// FormData → multipart/form-data 请求体（boundary 回填 contentType）。
pub fn encode_form_data_multipart(vm: &mut Vm, fd: Value) -> Result<(String, String), VmError> {
    let entries = fd_entries(vm, fd);
    let boundary = format!("----AlukaFormBoundary{}", generate_boundary());
    let mut body = String::new();
    for (name, value) in entries.iter() {
        body.push_str("--");
        body.push_str(&boundary);
        body.push_str("\r\nContent-Disposition: form-data; name=\"");
        body.push_str(&name.replace('\\', "\\\\").replace('"', "\\\""));
        body.push_str("\"\r\n\r\n");
        body.push_str(value);
        body.push_str("\r\n");
    }
    if !entries.is_empty() {
        body.push_str("--");
        body.push_str(&boundary);
        body.push_str("--\r\n");
    }
    let ctype = format!("multipart/form-data; boundary={boundary}");
    Ok((body, ctype))
}

/// 解析 multipart/urlencoded 响应体为 FormData（Response.formData）。
fn parse_form_data_body(vm: &mut Vm, ctype: &str, body: &str) -> Result<Value, VmError> {
    let fd = form_data_ctor_impl(vm, &[])?;
    if let Some(bi) = ctype.find("boundary=") {
        let boundary = ctype[bi + "boundary=".len()..]
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches('"')
            .to_owned();
        let delim = format!("--{boundary}");
        for part in body.split(&delim) {
            let part = part.trim_start_matches('\r').trim_start_matches('\n');
            if part.is_empty() || part.starts_with("--") {
                continue;
            }
            let Some(hdr_end) = part.find("\r\n\r\n") else {
                continue;
            };
            let headers = &part[..hdr_end];
            let value = part[hdr_end + 4..]
                .trim_end_matches('\r')
                .trim_end_matches('\n');
            let mut name = String::new();
            for line in headers.split("\r\n") {
                if let Some(rest) = line.strip_prefix("Content-Disposition:").map(str::trim) {
                    for seg in rest.split(';') {
                        let seg = seg.trim();
                        if let Some(n) = seg.strip_prefix("name=") {
                            name = n.trim_matches('"').to_owned();
                        }
                    }
                }
            }
            if !name.is_empty() {
                let name_val = vm.alloc_string(name);
                let value_val = vm.alloc_string(value.to_owned());
                let append = vm.alloc_native_fn("FormData.append");
                let _ = vm.invoke_callable(
                    Value::Object(append),
                    fd,
                    &[Value::Object(name_val), Value::Object(value_val)],
                );
            }
        }
    } else if ctype.starts_with("application/x-www-form-urlencoded") {
        for pair in body.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let kd = vm.alloc_string(url_decode(k));
            let vd = vm.alloc_string(url_decode(v));
            let append = vm.alloc_native_fn("FormData.append");
            let _ = vm.invoke_callable(
                Value::Object(append),
                fd,
                &[Value::Object(kd), Value::Object(vd)],
            );
        }
    }
    Ok(fd)
}

/// 轻量百分号解码（urlencoded 表单体）。
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() + 1 && i + 2 <= bytes.len() - 1 + 1 => {
                if i + 2 < bytes.len() {
                    let hex = &s[i + 1..i + 3];
                    if let Ok(b) = u8::from_str_radix(hex, 16) {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}
