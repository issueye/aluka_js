//! ECMAScript/Web 全局函数与全局构造器拆分单元。
//!
//! 拆分为以下子模块（各模块按单一职责独立演进）：
//!
//! | 模块 | 职责 |
//! |---|---|
//! | `core_fn` | isNaN / isFinite / parseFloat / parseInt |
//! | `uri` | encodeURIComponent / decodeURIComponent / encodeURI / decodeURI |
//! | `number` | Number 静态方法（isInteger / isSafeInteger / ...） |
//! | `string_fns` | String.fromCharCode / fromCodePoint |
//! | `date` | Date 构造器 + now/parse + 实例方法 |
//! | `object` | Object 静态方法 |
//! | `error` | Error.captureStackTrace + callsite 方法 |
//! | `fetch` | fetch / Response / Request + HTTP |
//! | `headers` | Headers 全局构造器与实例方法 |
//! | `abort` | AbortController / AbortSignal |
//! | `event` | EventTarget / Event / CustomEvent |
//! | `form_data` | FormData + multipart 编解码 |
//! | `web` | URLSearchParams / TextEncoder / TextDecoder / QueuingStrategy / Blob |

pub mod abort;
pub mod core_fn;
pub mod date;
pub mod error;
pub mod event;
pub mod fetch;
pub mod form_data;
pub mod headers;
pub mod number;
pub mod object;
pub mod string_fns;
pub mod uri;
pub mod web;

use crate::builtins::{BuiltinHandler, BuiltinRegistry, ModuleDef, register_handler};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// 全局函数模块（在 `register_all` 中装配）。
pub const MODULE: ModuleDef = ModuleDef {
    name: "globals",
    build,
};

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    // ---- 核心全局函数 ----
    let fns: &[(&str, &str, BuiltinHandler)] = &[
        ("isNaN", "global.isNaN", core_fn::global_is_nan),
        ("isFinite", "global.isFinite", core_fn::global_is_finite),
        ("parseFloat", "global.parseFloat", core_fn::global_parse_float),
        ("parseInt", "global.parse_int", core_fn::global_parse_int),
        ("encodeURIComponent", "global.encodeURIComponent", uri::encode_uri_component),
        ("decodeURIComponent", "global.decodeURIComponent", uri::decode_uri_component),
        ("encodeURI", "global.encodeURI", uri::encode_uri),
        ("decodeURI", "global.decodeURI", uri::decode_uri),
    ];
    for (name, _key, handler) in fns {
        let f = vm.alloc_native_fn(&format!("global.{name}"));
        vm.globals.insert(name.to_string(), Value::Object(f));
        register_handler(registry, "global", name, *handler);
    }

    // ---- Number ----
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
    for method in ["isInteger", "isSafeInteger", "isFinite", "isNaN", "parseInt", "parseFloat"] {
        let f = vm.alloc_native_fn(&format!("Number.{method}"));
        let _ = vm.set_property(Value::Object(number), method, Value::Object(f));
        register_handler(registry, "Number", method, number::number_static);
    }
    vm.globals.insert("Number".to_owned(), Value::Object(number));

    // ---- Boolean ----
    let bool_p = crate::builtins::surface::bool_proto(vm);
    let boolean = vm.alloc_native_ctor("Boolean", Some(bool_p));
    vm.globals.insert("Boolean".to_owned(), Value::Object(boolean));

    // ---- String ----
    let str_p = crate::builtins::surface::str_proto(vm);
    let string = vm.alloc_native_ctor("String", Some(str_p));
    for (method, handler) in [
        ("fromCharCode", string_fns::string_from_char_code as BuiltinHandler),
        ("fromCodePoint", string_fns::string_from_code_point as BuiltinHandler),
    ] {
        let f = vm.alloc_native_fn(&format!("String.{method}"));
        let _ = vm.set_property(Value::Object(string), method, Value::Object(f));
        register_handler(registry, "String", method, handler);
    }
    vm.globals.insert("String".to_owned(), Value::Object(string));

    // ---- Date ----
    let date_proto = vm.alloc_ordinary_with_proto(vm.object_prototype);
    let date = vm.alloc_native_ctor("Date", Some(date_proto));
    let now = vm.alloc_native_fn("Date.now");
    let _ = vm.set_property(Value::Object(date), "now", Value::Object(now));
    register_handler(registry, "Date", "now", date::date_now);
    let parse = vm.alloc_native_fn("Date.parse");
    let _ = vm.set_property(Value::Object(date), "parse", Value::Object(parse));
    register_handler(registry, "Date", "parse", date::date_parse);
    for method in ["getTime", "valueOf", "toISOString", "toString", "getTimezoneOffset"] {
        let date_fn = vm.alloc_native_fn(&format!("Date.{method}"));
        let _ = vm.set_property(Value::Object(date), method, Value::Object(date_fn));
        register_handler(registry, "Date", method, date::date_instance_method);
    }
    vm.globals.insert("Date".to_owned(), Value::Object(date));

    // ---- M4: Fetch API + AbortController ----
    let fetch_fn = vm.alloc_native_fn("fetch");
    vm.globals.insert("fetch".to_owned(), Value::Object(fetch_fn));
    registry.dispatch.insert("fetch".to_owned(), fetch::global_fetch);

    // AbortController
    let abort_ctor = vm.alloc_native_ctor("AbortController", None);
    vm.globals.insert("AbortController".to_owned(), Value::Object(abort_ctor));
    registry.dispatch.insert("AbortController".to_owned(), abort::abort_controller_ctor_impl);
    register_handler(registry, "AbortController", "abort", abort::controller_abort_impl);
    register_handler(registry, "AbortSignal", "abort", abort::abort_signal_abort_dispatch);
    register_handler(registry, "AbortSignal", "throwIfAborted", abort::signal_throw_if_aborted);

    // AbortSignal 全局
    let asig_ctor = vm.alloc_native_ctor("AbortSignal", None);
    let asig_proto = vm.alloc_ordinary();
    for method in ["addEventListener", "removeEventListener", "abort", "throwIfAborted"] {
        let f = vm.alloc_native_fn(&format!("AbortSignal.{method}"));
        let _ = vm.set_property(Value::Object(asig_proto), method, Value::Object(f));
    }
    let _ = vm.set_property(Value::Object(asig_ctor), "prototype", Value::Object(asig_proto));
    let asig_abort = vm.alloc_native_fn("AbortSignal.abort");
    let _ = vm.set_property(Value::Object(asig_ctor), "abort", Value::Object(asig_abort));
    vm.globals.insert("AbortSignal".to_owned(), Value::Object(asig_ctor));
    registry.dispatch.insert("AbortSignal".to_owned(), abort::abort_signal_ctor_impl);
    register_handler(registry, "AbortSignal", "addEventListener", abort::signal_add_event_listener);
    register_handler(registry, "AbortSignal", "removeEventListener", abort::signal_remove_event_listener);

    // Headers
    let headers_ctor = vm.alloc_native_ctor("Headers", None);
    vm.globals.insert("Headers".to_owned(), Value::Object(headers_ctor));
    registry.dispatch.insert("Headers".to_owned(), headers::headers_ctor_impl);
    register_handler(registry, "Headers", "get", headers::headers_get_impl);
    register_handler(registry, "Headers", "has", headers::headers_has_impl);
    for method in ["append", "set", "delete", "forEach"] {
        register_handler(registry, "Headers", method, headers::headers_method);
    }

    // Response 实例方法
    register_handler(registry, "Response", "text", fetch::response_text_handler);
    register_handler(registry, "Response", "json", fetch::response_json_dispatch);
    register_handler(registry, "Response", "arrayBuffer", fetch::response_array_buffer_handler);
    register_handler(registry, "Response", "formData", form_data::response_form_data_handler);
    register_handler(registry, "Response", "clone", fetch::response_clone);

    // Response 全局构造器
    let resp_ctor = vm.alloc_native_ctor("Response", None);
    let resp_proto = vm.alloc_ordinary();
    for method in ["text", "json", "arrayBuffer", "formData", "clone"] {
        let f = vm.alloc_native_fn(&format!("Response.{method}"));
        let _ = vm.set_property(Value::Object(resp_proto), method, Value::Object(f));
    }
    let _ = vm.set_property(Value::Object(resp_ctor), "prototype", Value::Object(resp_proto));
    for st in ["redirect", "error", "json"] {
        let f = vm.alloc_native_fn(&format!("Response.{st}"));
        let _ = vm.set_property(Value::Object(resp_ctor), st, Value::Object(f));
    }
    vm.globals.insert("Response".to_owned(), Value::Object(resp_ctor));
    registry.dispatch.insert("Response".to_owned(), fetch::response_ctor_impl);
    register_handler(registry, "Response", "redirect", fetch::response_static_redirect);
    register_handler(registry, "Response", "error", fetch::response_static_error);

    // Request 全局构造器
    let request_ctor = vm.alloc_native_ctor("Request", None);
    vm.globals.insert("Request".to_owned(), Value::Object(request_ctor));
    registry.dispatch.insert("Request".to_owned(), fetch::request_ctor_impl);
    register_handler(registry, "Request", "clone", fetch::request_clone);
    register_handler(registry, "Request", "text", fetch::response_text_handler);

    // ---- M4.1: URLSearchParams / TextEncoder / TextDecoder / QueuingStrategy / Blob ----
    let usp_ctor = vm.alloc_native_ctor("URLSearchParams", None);
    vm.globals.insert("URLSearchParams".to_owned(), Value::Object(usp_ctor));
    registry.dispatch.insert("URLSearchParams".to_owned(), web::url_search_params_ctor);
    for method in ["append", "get", "getAll", "has", "set", "delete", "toString"] {
        register_handler(registry, "URLSearchParams", method, web::url_search_params_method);
    }

    let te_ctor = vm.alloc_native_ctor("TextEncoder", None);
    vm.globals.insert("TextEncoder".to_owned(), Value::Object(te_ctor));
    registry.dispatch.insert("TextEncoder".to_owned(), web::text_encoder_ctor);
    register_handler(registry, "TextEncoder", "encode", web::text_encoder_encode);

    let td_ctor = vm.alloc_native_ctor("TextDecoder", None);
    vm.globals.insert("TextDecoder".to_owned(), Value::Object(td_ctor));
    registry.dispatch.insert("TextDecoder".to_owned(), web::text_decoder_ctor);
    register_handler(registry, "TextDecoder", "decode", web::text_decoder_decode);

    let bq_ctor = vm.alloc_native_ctor("ByteLengthQueuingStrategy", None);
    vm.globals.insert("ByteLengthQueuingStrategy".to_owned(), Value::Object(bq_ctor));
    registry.dispatch.insert("ByteLengthQueuingStrategy".to_owned(), web::queuing_strategy_ctor);
    let cq_ctor = vm.alloc_native_ctor("CountQueuingStrategy", None);
    vm.globals.insert("CountQueuingStrategy".to_owned(), Value::Object(cq_ctor));
    registry.dispatch.insert("CountQueuingStrategy".to_owned(), web::queuing_strategy_ctor);
    register_handler(registry, "ByteLengthQueuingStrategy", "size", web::queuing_strategy_size);
    register_handler(registry, "CountQueuingStrategy", "size", web::queuing_strategy_size);

    // Blob
    let blob_ctor = vm.alloc_native_ctor("Blob", None);
    vm.globals.insert("Blob".to_owned(), Value::Object(blob_ctor));
    registry.dispatch.insert("Blob".to_owned(), web::blob_ctor_impl);
    register_handler(registry, "Blob", "text", web::blob_text);
    register_handler(registry, "Blob", "arrayBuffer", web::blob_array_buffer);

    // ---- M4.4: EventTarget / CustomEvent / FormData ----
    let et_ctor = vm.alloc_native_ctor("EventTarget", None);
    vm.globals.insert("EventTarget".to_owned(), Value::Object(et_ctor));
    registry.dispatch.insert("EventTarget".to_owned(), event::event_target_ctor_impl);
    for method in ["addEventListener", "removeEventListener", "dispatchEvent"] {
        register_handler(registry, "EventTarget", method, event::event_target_dispatch);
    }

    let ev_ctor = vm.alloc_native_ctor("Event", None);
    vm.globals.insert("Event".to_owned(), Value::Object(ev_ctor));
    registry.dispatch.insert("Event".to_owned(), event::event_ctor_impl);
    register_handler(registry, "Event", "preventDefault", event::event_prevent_default);

    let ce_ctor = vm.alloc_native_ctor("CustomEvent", None);
    vm.globals.insert("CustomEvent".to_owned(), Value::Object(ce_ctor));
    registry.dispatch.insert("CustomEvent".to_owned(), event::custom_event_ctor_impl);

    let fd_ctor = vm.alloc_native_ctor("FormData", None);
    vm.globals.insert("FormData".to_owned(), Value::Object(fd_ctor));
    registry.dispatch.insert("FormData".to_owned(), form_data::form_data_ctor_impl);
    for method in ["append", "set", "get", "getAll", "has", "delete", "entries", "keys", "values", "forEach"] {
        register_handler(registry, "FormData", method, form_data::form_data_method);
    }

    // ---- M4.2: Web Streams 全局 ----
    for (name, handler) in [
        ("ReadableStream", crate::builtins::stream_web::readable_stream_ctor as BuiltinHandler),
        ("WritableStream", crate::builtins::stream_web::writable_stream_ctor as BuiltinHandler),
        ("TransformStream", crate::builtins::stream_web::transform_stream_ctor as BuiltinHandler),
    ] {
        let ctor = vm.alloc_native_ctor(name, None);
        vm.globals.insert(name.to_owned(), Value::Object(ctor));
        registry.dispatch.insert(name.to_owned(), handler);
    }

    // ---- Object 静态方法面 ----
    if let Some(octor) = vm.object_ctor {
        vm.builtin_registry.register_module_object("Object", octor);
        for method in [
            "defineProperty", "defineProperties", "getOwnPropertyDescriptor",
            "getOwnPropertyNames", "setPrototypeOf", "getPrototypeOf",
            "assign", "freeze", "seal", "isFrozen", "isSealed",
            "values", "entries", "fromEntries",
        ] {
            let f = vm.alloc_native_fn(&format!("Object.{method}"));
            let _ = vm.set_property(Value::Object(octor), method, Value::Object(f));
            register_handler(registry, "Object", method, object::object_static);
        }
    }

    // ---- Error 静态面 ----
    if let Some(ector) = vm.error_ctor {
        vm.builtin_registry.register_module_object("Error", ector);
        let cap = vm.alloc_native_fn("Error.captureStackTrace");
        let _ = vm.set_property(Value::Object(ector), "captureStackTrace", Value::Object(cap));
        register_handler(registry, "Error", "captureStackTrace", error::error_capture_stack_trace);
        let _ = vm.set_property(Value::Object(ector), "stackTraceLimit", Value::Number(10.0));
    }

    // ---- callsite 对象方法 ----
    for method in [
        "getFileName", "getLineNumber", "getColumnNumber", "toString",
        "isNative", "isEval", "isConstructor", "getFunctionName", "getTypeName",
    ] {
        register_handler(registry, "callsite", method, error::callsite_method);
    }

    // ---- globalThis ----
    let this_obj = vm.alloc_ordinary();
    let marker = vm.alloc_string("_isGlobalThis".to_owned());
    let _ = vm.set_property(Value::Object(this_obj), "_isGlobalThis", Value::Object(marker));
    vm.globals.insert("globalThis".to_owned(), Value::Object(this_obj));

    Ok(vm.alloc_ordinary())
}

/// 全局函数注册表处理器统一形态（handler 签名同名别名）。
#[allow(non_upper_case_globals)]
const _: () = ();