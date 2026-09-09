//! Fetch API 全局：fetch / Response / Request + HTTP 实现。

use crate::builtins::current_receiver;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
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
    let status = match vm.get_property(init, "status") {
        Ok(Value::Number(n)) => n as u16,
        _ => 200,
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Ok(Value::Object(h)) = vm.get_property(init, "headers") {
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
    let status = match args.get(1) {
        Some(Value::Number(n)) => *n as u16,
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
    if let Ok(Value::Boolean(true)) = vm.get_property(this, "_isResponse") {
        return response_json_handler(vm, args);
    }
    response_static_json(vm, args)
}

/// `response.clone()`。
pub(crate) fn response_clone(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    let this = current_receiver();
    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Ok(Value::Object(h)) = vm.get_property(this, "headers") {
        for (k, v) in vm.own_entries(h.0 as usize) {
            if k.starts_with('_') {
                continue;
            }
            pairs.push((k, vm.format_value(v)));
        }
    }
    let status = match vm.get_property(this, "status") {
        Ok(Value::Number(n)) => n as u16,
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
    let redirected = matches!(
        vm.get_property(this, "redirected"),
        Ok(Value::Boolean(true))
    );
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
    let (url, inherited) = if let Value::Object(_) = input {
        if let Ok(Value::Boolean(true)) = vm.get_property(input, "_isRequest") {
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
        .filter(|v| !matches!(v, Value::Undefined))
        .or_else(|| {
            inherited
                .and_then(|i| vm.get_property(i, "method").ok())
                .filter(|v| !matches!(v, Value::Undefined))
        })
        .unwrap_or(Value::Undefined);
    let _ = vm.set_property(Value::Object(req), "method", method);

    let headers_val = vm
        .get_property(opts, "headers")
        .ok()
        .filter(|v| !matches!(v, Value::Undefined))
        .or_else(|| {
            inherited
                .and_then(|i| vm.get_property(i, "headers").ok())
                .filter(|v| !matches!(v, Value::Undefined))
        })
        .unwrap_or(Value::Undefined);
    let headers_inst = super::headers::build_headers(vm, headers_val);
    let _ = vm.set_property(Value::Object(req), "headers", headers_inst);

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
    let is_request = matches!(first, Value::Object(_))
        && matches!(
            vm.get_property(first, "_isRequest"),
            Ok(Value::Boolean(true))
        );
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
    let mut response_bytes = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                response_bytes.extend_from_slice(&buf[..n]);
                // undici 语义：响应完整（头完成且 Content-Length 收满 /
                // chunked 终止块已到）即返回，**不等连接关闭**——否则对
                // keep-alive 服务器（响应后连接保持）空等到读超时。
                // （P0 修复：fetch → aluka http server 每次请求 10s 的根因）
                if response_complete(&response_bytes) {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&response_bytes).to_string();
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

/// 响应完整判定（undici 语义，不依赖连接关闭）：
/// 头部结束标记存在且 (a) `Content-Length` 已收满，或
/// (b) `Transfer-Encoding: chunked` 的终止块已出现。
/// 无长度信息（HTTP/1.0 close 定界）时返回 false——由读循环等 EOF/超时。
fn response_complete(bytes: &[u8]) -> bool {
    let Some(he) = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
    else {
        return false;
    };
    let head = String::from_utf8_lossy(&bytes[..he]);
    let lower = head.to_ascii_lowercase();
    if lower.contains("transfer-encoding: chunked") {
        // 终止块 "0\r\n\r\n"（允许 trailer 行存在，容忍实现简化）
        return bytes[he..].windows(5).any(|w| w == b"0\r\n\r\n");
    }
    if let Some(cl) = head.lines().find_map(|l| {
        let mut it = l.splitn(2, ':');
        match (it.next(), it.next()) {
            (Some(k), Some(v)) if k.trim().eq_ignore_ascii_case("content-length") => {
                v.trim().parse::<usize>().ok()
            }
            _ => None,
        }
    }) {
        return bytes.len() >= he + cl;
    }
    false
}

fn decode_chunked_body(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(line_end) = rest.find("\r\n") {
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
        let next = (data_end + 2).min(rest.len());
        rest = &rest[next..];
        if truncated {
            break;
        }
    }
    out
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
