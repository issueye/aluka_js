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

    // Number：转换调用 + 数值静态面
    let number = vm.alloc_native_ctor("Number", vm.object_prototype);
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

    // Boolean：转换调用
    let boolean = vm.alloc_native_ctor("Boolean", vm.object_prototype);
    vm.globals
        .insert("Boolean".to_owned(), Value::Object(boolean));

    // String：静态方法 fromCharCode/fromCodePoint（iconv-lite 等真实包依赖）
    let string = vm.alloc_native_ctor("String", vm.object_prototype);
    for (method, handler) in [
        ("fromCharCode", string_from_char_code as BuiltinHandler),
        ("fromCodePoint", string_from_code_point as BuiltinHandler),
    ] {
        let f = vm.alloc_native_fn(&format!("String.{method}"));
        let _ = vm.set_property(Value::Object(string), method, Value::Object(f));
        register_handler(registry, "String", method, handler);
    }
    vm.globals.insert("String".to_owned(), Value::Object(string));

    // Date：now 静态 + 实例最小面（getTime/toISOString/valueOf/toString）
    let date = vm.alloc_native_ctor("Date", vm.object_prototype);
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
    Ok(Value::Object(
        vm.alloc_string(match String::from_utf8(out) {
            Ok(s) => s,
            Err(_) => {
                let err = vm.alloc_string("URI malformed".to_owned());
                let name = vm.alloc_string("URIError".to_owned());
                let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
                return Err(VmError::Thrown(Value::Object(err)));
            }
        }),
    ))
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
