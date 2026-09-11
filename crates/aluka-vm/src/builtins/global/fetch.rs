//! Fetch API 全局：fetch / Response / Request + HTTP 实现。

use crate::builtins::current_receiver;
use crate::builtins::http::wire;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};
use aluka_core::ObjectRef;

// ---- 公开 handlers ----

/// `fetch(url[, options]) -> Promise<Response>`。
pub(crate) fn global_fetch(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let url_val = args.first().copied().unwrap_or(Value::Undefined);
    let opts = args.get(1).copied().unwrap_or(Value::Undefined);

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
    if let Some(sig_ref) = signal.as_object() {
        if let Ok(ValueCase::Boolean(true)) = vm
            .get_property(Value::Object(sig_ref), "aborted")
            .map(ValueCase::from)
        {
            let reason = match vm.get_property(Value::Object(sig_ref), "reason") {
                Ok(r) if !matches!(r, Value::Undefined) => r,
                _ => default_abort_error(vm),
            };
            let promise = vm.alloc_rejected_promise(reason);
            return Ok(Value::Object(promise));
        }
    }

    if url.starts_with("https://") {
        let err = vm.alloc_error_instance("fetch failed");
        let name = vm.alloc_string("TypeError".to_owned());
        let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
        let promise = vm.alloc_rejected_promise(Value::Object(err));
        return Ok(Value::Object(promise));
    }

    let method_upper = method.to_uppercase();
    if matches!(method_upper.as_str(), "CONNECT" | "TRACE" | "TRACK") {
        let err = vm.alloc_error_instance("fetch failed");
        let name = vm.alloc_string("TypeError".to_owned());
        let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
        let promise = vm.alloc_rejected_promise(Value::Object(err));
        return Ok(Value::Object(promise));
    }

    let mut current_url = url;
    let mut result = None;
    // Node 22：实际跟随过 ≥1 次重定向的最终响应 `redirected` 为 true
    //（manual/error 模式与无重定向链保持 false）。
    let mut followed_any = false;
    for _ in 0..20 {
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
                            if let Some(loc) = &location {
                                current_url = resolve_redirect_url(&current_url, loc);
                                followed_any = true;
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
    let (hdr_pairs, _) = parse_response_headers(&headers_text);

    if let Some(sig_ref) = signal.as_object() {
        if let Ok(ValueCase::Boolean(true)) = vm
            .get_property(Value::Object(sig_ref), "aborted")
            .map(ValueCase::from)
        {
            let reason = match vm.get_property(Value::Object(sig_ref), "reason") {
                Ok(r) if !matches!(r, Value::Undefined) => r,
                _ => default_abort_error(vm),
            };
            let promise = vm.alloc_rejected_promise(reason);
            return Ok(Value::Object(promise));
        }
    }

    let response = build_response_object(
        vm,
        status,
        &headers_text,
        &hdr_pairs,
        &body_text,
        followed_any,
        &current_url,
    )?;
    let promise = vm.alloc_fulfilled_promise(Value::Object(response));
    Ok(Value::Object(promise))
}

/// 构造 Response 对象（fetch 与 new Response() 共用）。
pub(crate) fn build_response_object(
    vm: &mut Vm,
    status: u16,
    header_block: &str,
    hdr_pairs: &[(String, String)],
    body_text: &str,
    followed_redirect: bool,
    url: &str,
) -> Result<ObjectRef, VmError> {
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
    let status_text = header_block
        .split("\r\n")
        .next()
        .unwrap_or("")
        .split_whitespace()
        .skip(2)
        .collect::<Vec<_>>()
        .join(" ");
    let st_ref = vm.alloc_string(status_text);
    let _ = vm.set_property(Value::Object(response), "statusText", Value::Object(st_ref));
    let ty_ref = vm.alloc_string("basic".to_owned());
    let _ = vm.set_property(Value::Object(response), "type", Value::Object(ty_ref));
    let url_ref = vm.alloc_string(url.to_owned());
    let _ = vm.set_property(Value::Object(response), "url", Value::Object(url_ref));
    let _ = vm.set_property(Value::Object(response), "bodyUsed", Value::Boolean(false));
    let body_ref = vm.alloc_string(body_text.to_owned());
    let _ = vm.set_property(
        Value::Object(response),
        "_bodyText",
        Value::Object(body_ref),
    );
    let _ = vm.set_property(Value::Object(response), "_isResponse", Value::Boolean(true));

    // headers
    let headers_obj = vm.alloc_ordinary();
    for method in HEADERS_METHODS {
        let f = vm.alloc_native_fn(&format!("Headers.{method}"));
        let _ = vm.set_property(Value::Object(headers_obj), method, Value::Object(f));
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
    super::headers::hdr_rewrite(vm, Value::Object(headers_obj), hdr_pairs);
    super::headers::hdr_sync_props(vm, Value::Object(headers_obj), hdr_pairs);
    let _ = vm.set_property(
        Value::Object(response),
        "headers",
        Value::Object(headers_obj),
    );

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
    let _ = vm.set_property(
        Value::Object(response),
        "redirected",
        Value::Boolean(followed_redirect),
    );

    for method in ["text", "json", "arrayBuffer", "formData", "clone"] {
        let f = vm.alloc_native_fn(&format!("Response.{method}"));
        let _ = vm.set_property(Value::Object(response), method, Value::Object(f));
    }
    let body_obj = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(body_obj), "_isBody", Value::Boolean(true));
    let _ = vm.set_property(Value::Object(response), "body", Value::Object(body_obj));
    Ok(response)
}

const HEADERS_METHODS: &[&str] = &[
    "append", "set", "get", "has", "delete", "forEach", "entries", "keys", "values",
];

/// `new Response([body][, init])`。
pub(crate) fn response_ctor_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let body = args.first().copied().unwrap_or(Value::Undefined);
    let init = args.get(1).copied().unwrap_or(Value::Undefined);
    let has_body = !matches!(body, Value::Undefined | Value::Null);
    let body_text = if has_body {
        vm.format_value(body)
    } else {
        String::new()
    };
    let status = match vm.get_property(init, "status").map(|v| v.case()) {
        Ok(ValueCase::Number(n)) => n as u16,
        _ => 200,
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Ok(ValueCase::Object(h)) = vm.get_property(init, "headers").map(ValueCase::from) {
        for (k, v) in vm.own_entries(h.0 as usize) {
            pairs.push((k, vm.format_value(v)));
        }
    }
    if has_body
        && !pairs
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    {
        pairs.push((
            "content-type".to_owned(),
            "text/plain;charset=UTF-8".to_owned(),
        ));
    }
    let response = build_response_object(vm, status, "", &pairs, &body_text, false, "")?;
    let ty = vm.alloc_string("default".to_owned());
    let _ = vm.set_property(Value::Object(response), "type", Value::Object(ty));
    if let Some(st) = opt_str(vm, init, "statusText") {
        let s = vm.alloc_string(st);
        let _ = vm.set_property(Value::Object(response), "statusText", Value::Object(s));
    }
    let _ = vm.set_property(
        Value::Object(response),
        "ok",
        Value::Boolean((200..300).contains(&status)),
    );
    Ok(Value::Object(response))
}

/// `Response.redirect(url[, status])`。
pub(crate) fn response_static_redirect(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let url = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let status = match args.get(1).map(|v| v.case()) {
        Some(ValueCase::Number(n)) => n as u16,
        _ => 302,
    };
    let pairs = vec![("location".to_owned(), url)];
    let response = build_response_object(vm, status, "", &pairs, "", false, "")?;
    let ty = vm.alloc_string("default".to_owned());
    let _ = vm.set_property(Value::Object(response), "type", Value::Object(ty));
    Ok(Value::Object(response))
}

/// `Response.error()`。
pub(crate) fn response_static_error(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let response = build_response_object(vm, 0, "", &[], "", false, "")?;
    let ty = vm.alloc_string("error".to_owned());
    let _ = vm.set_property(Value::Object(response), "type", Value::Object(ty));
    Ok(Value::Object(response))
}

/// `Response.json(data[, init])` 静态。
fn response_static_json(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let data = args.first().copied().unwrap_or(Value::Undefined);
    let text = match vm.json_stringify(data) {
        Ok(v) => vm.format_value(v),
        Err(_) => "null".to_owned(),
    };
    let pairs = vec![("content-type".to_owned(), "application/json".to_owned())];
    let response = build_response_object(vm, 200, "", &pairs, &text, false, "")?;
    let ty = vm.alloc_string("default".to_owned());
    let _ = vm.set_property(Value::Object(response), "type", Value::Object(ty));
    Ok(Value::Object(response))
}

/// `Response.json(data)` 静态与 `response.json()` 实例按 receiver 分流。
pub(crate) fn response_json_dispatch(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let this = current_receiver();
    if let Ok(ValueCase::Boolean(true)) = vm.get_property(this, "_isResponse").map(ValueCase::from)
    {
        return response_json_handler(vm, args);
    }
    response_static_json(vm, args)
}

/// `response.clone()`。
pub(crate) fn response_clone(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = current_receiver();
    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Ok(ValueCase::Object(h)) = vm.get_property(this, "headers").map(ValueCase::from) {
        for (k, v) in vm.own_entries(h.0 as usize) {
            if k.starts_with('_') {
                continue;
            }
            pairs.push((k, vm.format_value(v)));
        }
    }
    let status = match vm.get_property(this, "status").map(|v| v.case()) {
        Ok(ValueCase::Number(n)) => n as u16,
        _ => 200,
    };
    let status_text = vm
        .get_property(this, "statusText")
        .map(|v| vm.format_value(v))
        .unwrap_or_default();
    let body_text = vm
        .get_property(this, "_bodyText")
        .map(|v| vm.format_value(v))
        .unwrap_or_default();
    let url = vm
        .get_property(this, "url")
        .map(|v| vm.format_value(v))
        .unwrap_or_default();
    let redirected = vm
        .get_property(this, "redirected")
        .is_ok_and(|v| v.as_bool() == Some(true));
    let response = build_response_object(vm, status, "", &pairs, &body_text, redirected, &url)?;
    let st = vm.alloc_string(status_text);
    let _ = vm.set_property(Value::Object(response), "statusText", Value::Object(st));
    let ty = vm.alloc_string("default".to_owned());
    let _ = vm.set_property(Value::Object(response), "type", Value::Object(ty));
    Ok(Value::Object(response))
}

/// `response.text()`。
pub(crate) fn response_text_handler(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let _ = vm.set_property(receiver, "bodyUsed", Value::Boolean(true));
    vm.get_property(receiver, "_bodyText")
}

/// `response.json()` 解析 body。
fn response_json_handler(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let _ = vm.set_property(receiver, "bodyUsed", Value::Boolean(true));
    let body = vm.get_property(receiver, "_bodyText")?;
    let text = vm.format_value(body);
    let str_ref = vm.alloc_string(text);
    vm.json_parse(&[Value::Object(str_ref)])
}

/// `response.arrayBuffer()`。
pub(crate) fn response_array_buffer_handler(
    vm: &mut Vm,
    _args: &[Value],
) -> Result<Value, VmError> {
    let receiver = current_receiver();
    let _ = vm.set_property(receiver, "bodyUsed", Value::Boolean(true));
    let body = vm.get_property(receiver, "_bodyText")?;
    let text = vm.format_value(body);
    let bytes: Vec<Value> = text
        .as_bytes()
        .iter()
        .map(|&b| Value::Number(b as f64))
        .collect();
    Ok(Value::Object(vm.alloc_array(bytes)))
}

// ---- Request ----

/// `new Request(input[, options])`。
pub(crate) fn request_ctor_impl(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let input = args.first().copied().unwrap_or(Value::Undefined);
    let (url, inherited) = if input.as_object().is_some() {
        if let Ok(ValueCase::Boolean(true)) =
            vm.get_property(input, "_isRequest").map(ValueCase::from)
        {
            (vm.get_property(input, "url")?, Some(input))
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

    let method = vm
        .get_property(opts, "method")
        .ok()
        .filter(|v| !v.is_undefined())
        .or_else(|| {
            inherited
                .and_then(|i| vm.get_property(i, "method").ok())
                .filter(|v| !v.is_undefined())
        })
        .unwrap_or(Value::Undefined);
    let _ = vm.set_property(Value::Object(req), "method", method);

    let headers_val = vm
        .get_property(opts, "headers")
        .ok()
        .filter(|v| !v.is_undefined())
        .or_else(|| {
            inherited
                .and_then(|i| vm.get_property(i, "headers").ok())
                .filter(|v| !v.is_undefined())
        })
        .unwrap_or(Value::Undefined);
    let headers_inst = super::headers::build_headers(vm, headers_val);
    let _ = vm.set_property(Value::Object(req), "headers", headers_inst);

    let body = vm
        .get_property(opts, "body")
        .ok()
        .filter(|v| !(v.is_undefined() || v.is_null()))
        .or_else(|| {
            inherited
                .and_then(|i| vm.get_property(i, "body").ok())
                .filter(|v| !(v.is_undefined() || v.is_null()))
        })
        .unwrap_or(Value::Undefined);
    let _ = vm.set_property(Value::Object(req), "body", body);

    let signal = match vm.get_property(opts, "signal") {
        Ok(s) if !matches!(s, Value::Undefined) => s,
        _ => Value::Null,
    };
    let _ = vm.set_property(Value::Object(req), "signal", signal);

    let redirect = opt_str(vm, opts, "redirect")
        .or_else(|| inherited.and_then(|i| opt_str(vm, i, "redirect")))
        .unwrap_or_else(|| "follow".to_owned());
    let redirect_val = vm.alloc_string(redirect);
    let _ = vm.set_property(Value::Object(req), "redirect", Value::Object(redirect_val));
    let _ = vm.set_property(Value::Object(req), "bodyUsed", Value::Boolean(false));
    let duplex = vm.alloc_string("half".to_owned());
    let _ = vm.set_property(Value::Object(req), "duplex", Value::Object(duplex));

    for method in ["text", "clone"] {
        let f = vm.alloc_native_fn(&format!("Request.{method}"));
        let _ = vm.set_property(Value::Object(req), method, Value::Object(f));
    }

    let method_text = vm
        .get_property(Value::Object(req), "method")
        .map(|v| vm.format_value(v))
        .unwrap_or_default()
        .to_uppercase();
    if matches!(method_text.as_str(), "GET" | "HEAD")
        && !matches!(body, Value::Undefined | Value::Null)
    {
        let err =
            vm.alloc_error_instance("Request constructor: GET/HEAD request cannot have a body");
        let n = vm.alloc_string("TypeError".to_owned());
        let _ = vm.set_property(Value::Object(err), "name", Value::Object(n));
        return Err(VmError::Thrown(Value::Object(err)));
    }
    Ok(Value::Object(req))
}

/// `request.clone()`。
pub(crate) fn request_clone(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = current_receiver();
    let url = vm.get_property(this, "url").unwrap_or(Value::Undefined);
    let method = vm.get_property(this, "method").unwrap_or(Value::Undefined);
    let headers = vm.get_property(this, "headers").unwrap_or(Value::Undefined);
    let body = vm.get_property(this, "body").unwrap_or(Value::Undefined);
    let signal = vm.get_property(this, "signal").unwrap_or(Value::Null);
    let redirect = vm
        .get_property(this, "redirect")
        .ok()
        .map(|v| vm.format_value(v))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "follow".to_owned());

    let req = vm.alloc_ordinary();
    let _ = vm.set_property(Value::Object(req), "_isRequest", Value::Boolean(true));
    let _ = vm.set_property(Value::Object(req), "url", url);
    let _ = vm.set_property(Value::Object(req), "method", method);
    let _ = vm.set_property(Value::Object(req), "headers", headers);
    let _ = vm.set_property(Value::Object(req), "body", body);
    let _ = vm.set_property(Value::Object(req), "signal", signal);
    let rv = vm.alloc_string(redirect);
    let _ = vm.set_property(Value::Object(req), "redirect", Value::Object(rv));
    let _ = vm.set_property(Value::Object(req), "bodyUsed", Value::Boolean(false));
    let dx = vm.alloc_string("half".to_owned());
    let _ = vm.set_property(Value::Object(req), "duplex", Value::Object(dx));
    for method in ["text", "clone"] {
        let f = vm.alloc_native_fn(&format!("Request.{method}"));
        let _ = vm.set_property(Value::Object(req), method, Value::Object(f));
    }
    Ok(Value::Object(req))
}

// ---- 内部辅助 ----

struct FetchInput {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<Value>,
    signal: Value,
    redirect: String,
}

/// 读取对象属性并格式化为字符串；Undefined/Null/空串视为未提供。
pub(crate) fn opt_str(vm: &mut Vm, obj: Value, key: &str) -> Option<String> {
    let val = vm.get_property(obj, key).ok()?;
    if matches!(val, Value::Undefined | Value::Null) {
        return None;
    }
    let s = vm.format_value(val);
    if s.is_empty() { None } else { Some(s) }
}

fn parse_fetch_input(vm: &mut Vm, first: Value, opts: Value) -> Result<FetchInput, VmError> {
    let is_request = matches!(first.case(), ValueCase::Object(_))
        && vm
            .get_property(first, "_isRequest")
            .is_ok_and(|v| v.as_bool() == Some(true));
    let url = if is_request {
        opt_str(vm, first, "url").unwrap_or_default()
    } else {
        vm.format_value(first)
    };
    let mut method = if is_request {
        opt_str(vm, first, "method").unwrap_or_else(|| "GET".to_owned())
    } else {
        "GET".to_owned()
    };
    if let Some(ms) = opt_str(vm, opts, "method") {
        method = ms.to_uppercase();
    }
    let mut headers: Vec<(String, String)> = Vec::new();
    if is_request {
        if let Ok(ValueCase::Object(ho)) = vm.get_property(first, "headers").map(ValueCase::from) {
            for (k, v) in vm.own_entries(ho.0 as usize) {
                headers.push((k, vm.format_value(v)));
            }
        }
    }
    if let Ok(ValueCase::Object(hdr_obj)) = vm.get_property(opts, "headers").map(ValueCase::from) {
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
            .filter(|v| !(v.is_undefined() || v.is_null()))
    } else {
        None
    };
    if let Ok(b) = vm.get_property(opts, "body") {
        if !(b.is_undefined() || b.is_null()) {
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
        opt_str(vm, first, "redirect").unwrap_or_else(|| "follow".to_owned())
    } else {
        "follow".to_owned()
    };
    if let Some(rs) = opt_str(vm, opts, "redirect") {
        redirect = rs;
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

fn default_abort_error(vm: &mut Vm) -> Value {
    let err = vm.alloc_error_instance("This operation was aborted");
    let name = vm.alloc_string("AbortError".to_owned());
    let _ = vm.set_property(Value::Object(err), "name", Value::Object(name));
    Value::Object(err)
}

// ---- HTTP 实现 ----

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

fn resolve_redirect_url(current: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_owned();
    }
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
        let dir = match cur_path.rfind('/') {
            Some(i) => &cur_path[..i + 1],
            None => "/",
        };
        format!("http://{host_port}{dir}")
    };
    let location_trimmed = location.trim_start_matches('/');
    format!("{base}/{location_trimmed}")
}

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
    // RFC 9112 §6.3 定界规则 1：HEAD 请求的响应没有 body
    let head_only = method.eq_ignore_ascii_case("HEAD");
    let mut response_bytes = Vec::new();
    let mut buf = [0u8; 8192];
    let mut complete = false;
    let mut closed = false;
    let mut timed_out = false;
    loop {
        match stream.read(&mut buf) {
            // 对端干净关闭：不直接当成“读到成功”，先看响应本身是否已按定界
            // 规则完整（无长度声明、以连接关闭定界的响应由下方例外分支放行）。
            Ok(0) => {
                closed = true;
                complete = matches!(response_complete(&response_bytes, head_only), Ok(true));
                break;
            }
            Ok(n) => {
                response_bytes.extend_from_slice(&buf[..n]);
                // undici 语义：响应完整（头完成且 body 按定界规则收满）即返回，
                // **不等连接关闭**——否则对 keep-alive 服务器空等到读超时。
                // （P0 修复：fetch → aluka http server 每次请求 10s 的根因）
                match response_complete(&response_bytes, head_only) {
                    Ok(true) => {
                        complete = true;
                        break;
                    }
                    Ok(false) => {}
                    // 畸形响应（状态行非法）：立即失败，不空等到读超时
                    Err(message) => return Err(message),
                }
            }
            // 读超时：连接仍在，但响应没有完整到达——不得当成成功
            Err(e) if is_read_timeout(&e) => {
                timed_out = true;
                break;
            }
            Err(e) => return Err(format!("fetch: 读取响应失败（IO 错误）: {e}")),
        }
    }
    let head = match parse_response_head(&response_bytes, head_only) {
        Ok(Some(head)) => head,
        Ok(None) => {
            return Err(if timed_out {
                "fetch: 读取响应超时：响应头未在 10s 内完整到达".to_owned()
            } else {
                "fetch: 连接已关闭，但未收到任何完整响应头".to_owned()
            });
        }
        Err(message) => return Err(message),
    };
    if !complete {
        // 例外（RFC 9112 §6.3 定界规则 4）：既非 bodyless、又无 TE/CL 的响应
        // 本来就以连接关闭定界——读到 EOF 即视为完整（HTTP/1.0 风格）。
        // 其余情况（Content-Length 未收满 / chunked 未到终止块）未收全即失败。
        let eof_delimited = closed && matches!(head.delim, BodyDelim::UntilClose);
        if !eof_delimited {
            return Err(if timed_out {
                "fetch: 读取响应超时：响应 body 未在 10s 内完整到达".to_owned()
            } else if closed {
                "fetch: 连接在响应完整到达前被对端关闭".to_owned()
            } else {
                "fetch: 响应不完整".to_owned()
            });
        }
    }
    let header_block = String::from_utf8_lossy(&response_bytes[..head.head_text_end]).to_string();
    let body_text = match head.delim {
        // bodyless（HEAD / 1xx / 204 / 304）：头块结束即无 body，多送字节也不计入
        BodyDelim::Inherent => String::new(),
        // chunked：复用 `builtins::http::wire` 的帧游走结果（同一份解码逻辑）
        BodyDelim::Chunked => wire::take_chunked_with_body(&response_bytes, head.body_start)
            .map(|(_, decoded)| String::from_utf8_lossy(&decoded).to_string())
            .unwrap_or_default(),
        _ => String::from_utf8_lossy(&response_bytes[head.body_start..]).to_string(),
    };
    Ok((head.status, header_block, body_text))
}

/// 响应体定界模式（RFC 9112 §6.3 的四条定界规则，按优先级判定）。
enum BodyDelim {
    /// 无 body：HEAD 请求，或 1xx / 204 / 304 状态码——头部块结束即响应完整。
    Inherent,
    /// `Transfer-Encoding: chunked`——chunk 帧游走到终止块（含可选 trailer）才算完整。
    Chunked,
    /// `Content-Length: n`——收满 n 字节 body 才算完整。
    Length(usize),
    /// 既无 bodyless 状态也无 TE/CL——以连接关闭（EOF）定界。
    UntilClose,
}

/// 解析出的响应头信息（`headers_text` 与 body 切片所需的偏移 + 定界模式）。
struct ResponseHeadInfo {
    /// 状态码
    status: u16,
    /// 头部块结束偏移（不含结束空行），即 `headers_text` 的切片终点
    head_text_end: usize,
    /// body 起始偏移
    body_start: usize,
    /// body 定界模式
    delim: BodyDelim,
}

/// 读超时判定（Windows 为 `WouldBlock`，Unix 为 `TimedOut`）。
fn is_read_timeout(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// 在缓冲中定位头部块结束：优先 `CRLFCRLF`，容忍裸 `LFLF`。
/// 返回 `(头部块结束偏移, body 起始偏移)`。
fn find_head_end(bytes: &[u8]) -> Option<(usize, usize)> {
    if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
        return Some((i, i + 4));
    }
    bytes
        .windows(2)
        .position(|w| w == b"\n\n")
        .map(|i| (i, i + 2))
}

/// 按名取头部块中的首个值（名大小写不敏感）。
fn header_first_value(head_text: &str, name: &str) -> Option<String> {
    head_text.split('\n').skip(1).find_map(|line| {
        let (key, value) = line.trim_end_matches('\r').split_once(':')?;
        if key.trim().eq_ignore_ascii_case(name) {
            Some(value.trim().to_owned())
        } else {
            None
        }
    })
}

/// 解析状态行并判定 body 定界模式：
/// - `Ok(None)`：头部块尚未收全（继续读）；
/// - `Ok(Some(_))`：头部块已收全；
/// - `Err(_)`：头部块已收全但状态行非法（畸形响应，立即失败，禁止按 status=0 兜底）。
fn parse_response_head(bytes: &[u8], head_only: bool) -> Result<Option<ResponseHeadInfo>, String> {
    let Some((head_text_end, body_start)) = find_head_end(bytes) else {
        return Ok(None);
    };
    let head_text = String::from_utf8_lossy(&bytes[..head_text_end]);
    let status_line = head_text
        .split('\n')
        .next()
        .unwrap_or("")
        .trim_end_matches('\r');
    let mut parts = status_line.split_whitespace();
    let version = parts.next().unwrap_or("");
    let status = parts.next().and_then(|token| token.parse::<u16>().ok());
    if !version.starts_with("HTTP/") {
        return Err(format!("fetch: 畸形响应：非法状态行 {status_line:?}"));
    }
    let Some(status) = status else {
        return Err(format!("fetch: 畸形响应：非法状态行 {status_line:?}"));
    };
    // 规则 1：HEAD 或 bodyless 状态码 → 无 body；规则 2：Transfer-Encoding；
    // 规则 3：Content-Length；规则 4：连接关闭定界。
    let delim = if head_only || wire::status_is_bodyless(status) {
        BodyDelim::Inherent
    } else if header_first_value(&head_text, "transfer-encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false)
    {
        BodyDelim::Chunked
    } else if let Some(cl) = header_first_value(&head_text, "content-length")
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        BodyDelim::Length(cl)
    } else {
        BodyDelim::UntilClose
    };
    Ok(Some(ResponseHeadInfo {
        status,
        head_text_end,
        body_start,
        delim,
    }))
}

/// 响应是否已按定界规则收全（undici 语义，不依赖连接关闭）。
/// `Err(_)` 表示头部块已收全但响应畸形。
fn response_complete(bytes: &[u8], head_only: bool) -> Result<bool, String> {
    let Some(head) = parse_response_head(bytes, head_only)? else {
        return Ok(false);
    };
    Ok(match head.delim {
        BodyDelim::Inherent => true,
        BodyDelim::Length(cl) => bytes.len() >= head.body_start.saturating_add(cl),
        BodyDelim::Chunked => chunked_complete(bytes, head.body_start),
        // 连接关闭定界：EOF 之前一律视为未完成（由调用方按“读到 EOF 即完整”放行）
        BodyDelim::UntilClose => false,
    })
}

/// chunked 体是否收全：游走 `chunk-size[;ext]CRLF` + 数据 + `CRLF`，直到 `size==0`，
/// 再吃掉可选 trailer 行与收尾 `CRLF`。
///
/// 帧游走复用 `builtins::http::wire::take_chunked_with_body`（同一 crate，无分层违规）；
/// 再补一次严格检查：该实现容忍终止块收尾空行尚未到齐（`0[;ext]CRLF` 即返回），
/// 而完整终止块的尾部必然是 `CRLF CRLF`——据此排除“半截终止块”。
fn chunked_complete(bytes: &[u8], body_start: usize) -> bool {
    let Some((end, _)) = wire::take_chunked_with_body(bytes, body_start) else {
        return false;
    };
    end >= 4 && &bytes[end - 4..end] == b"\r\n\r\n"
}

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

/// 响应定界规则的单元测试（纯函数，不依赖 VM / 网络）。
#[cfg(test)]
mod delim_tests {
    use super::*;

    fn complete(bytes: &[u8]) -> bool {
        response_complete(bytes, false).expect("不应被判为畸形响应")
    }

    /// bodyless 状态码（1xx / 204 / 304）：头块结束即完整，不得落回读超时兜底。
    #[test]
    fn bodyless_status_completes_at_header_end() {
        assert!(complete(b"HTTP/1.1 204 No Content\r\n\r\n"));
        assert!(complete(
            b"HTTP/1.1 304 Not Modified\r\nContent-Length: 1234\r\n\r\n"
        ));
        assert!(complete(b"HTTP/1.1 100 Continue\r\n\r\n"));
    }

    /// HEAD 请求的响应没有 body（即使带 Content-Length）。
    #[test]
    fn head_response_has_no_body() {
        let bytes = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n";
        assert!(!complete(bytes));
        assert!(response_complete(bytes, true).expect("HEAD 响应不应被判为畸形"));
    }

    /// Content-Length 定界：收满才算完整。
    #[test]
    fn content_length_delimits() {
        assert!(!complete(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhell"
        ));
        assert!(complete(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello"
        ));
    }

    /// 既非 bodyless 又无 TE/CL：以连接关闭定界，EOF 之前一律未完整。
    #[test]
    fn close_delimited_waits_for_eof() {
        assert!(!complete(b"HTTP/1.1 200 OK\r\n\r\nsome-body"));
    }

    /// 头部块未收全时不得提前判定。
    #[test]
    fn incomplete_head_is_not_complete() {
        assert!(!complete(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n"));
        assert!(response_complete(b"HTTP/1.1 200 OK\r\nContent-Len", false).is_ok());
    }

    /// 数据区含字面 `0\r\n\r\n` 时不得提前截断（按帧游走而非裸扫描）。
    #[test]
    fn chunked_data_containing_terminator_bytes_is_not_truncated() {
        // 12 字节单分块，数据 = `AB0\r\n\r\nCDEFG`，只到了前 7 字节
        assert!(!complete(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nc\r\nAB0\r\n\r\n"
        ));
        assert!(complete(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nc\r\nAB0\r\n\r\nCDEFG\r\n0\r\n\r\n"
        ));
    }

    /// 终止块带 chunk 扩展 / trailer 行都要识别；半截终止块不算完整。
    #[test]
    fn chunked_terminator_variants() {
        let head: &[u8] = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        let mut with_ext = head.to_vec();
        with_ext.extend_from_slice(b"5\r\nhello\r\n0;x=1\r\n\r\n");
        assert!(complete(&with_ext));

        let mut with_trailer = head.to_vec();
        with_trailer.extend_from_slice(b"5\r\nhello\r\n0\r\nx-trailer: v\r\n\r\n");
        assert!(complete(&with_trailer));

        let mut half_terminator = head.to_vec();
        half_terminator.extend_from_slice(b"5\r\nhello\r\n0\r\n");
        assert!(!complete(&half_terminator));

        let mut multi = head.to_vec();
        multi.extend_from_slice(b"3\r\nabc\r\n4\r\ndefg\r\n0\r\n\r\n");
        assert!(complete(&multi));
    }

    /// 畸形状态行必须报错（禁止 status=0 伪成功）。
    #[test]
    fn malformed_status_line_is_error() {
        assert!(response_complete(b"NONSENSE\r\n\r\n", false).is_err());
        assert!(response_complete(b"HTTP/1.1 ??? OK\r\n\r\n", false).is_err());
    }
}
