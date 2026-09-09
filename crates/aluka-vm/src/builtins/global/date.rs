//! Date 构造器 + 静态方法（now / parse）+ 实例方法。

use crate::builtins::current_receiver;
use crate::heap::HeapObject;
use crate::interpreter::{Vm, VmError};
use crate::value::Value;

fn date_now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

pub(crate) fn date_now(_vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Number(date_now_ms()))
}

pub(crate) fn date_parse(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let text = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
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

impl Vm {
    /// `new Date([value])`。
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

/// Date 实例方法。
pub(crate) fn date_instance_method(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
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
