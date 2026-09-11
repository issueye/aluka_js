//! Date 构造器 + 静态方法（`now` / `parse` / `UTC`）+ 实例方法面。
//!
//! ## 时区口径（**已登记偏离**，见 `.work/TODO/20260910/README.md §待办 15`）
//!
//! 仓库无任何时间/时区依赖（无 chrono/time/jiff），workspace 级
//! `unsafe_code = "deny"` 亦禁止 FFI 取系统时区，故本模块按两类口径实现：
//!
//! - **UTC 类**（`getUTC*` / `toISOString` / `toJSON` / `toUTCString` / `Date.UTC`）
//!   与 Node.js 22 精确对齐（逐字节）；
//! - **本地时间类**（`getFullYear` / `getMonth` / `getDate` / `getDay` / `getHours` /
//!   `getMinutes` / `getSeconds` / `getMilliseconds` / `getYear` / `toString` /
//!   `toDateString` / `toTimeString` / `toLocale*` / `getTimezoneOffset`，以及
//!   `set*` 的本地语义）一律按**「本地 = UTC（偏移 0）」**计算。
//!   UTC 机器上与 Node 一致；UTC+8 等非零偏移机器上 `getHours()` 等与 Node 相差
//!   一个偏移量、`toString()` 输出 `GMT+0000 (Coordinated Universal Time)` 形态，
//!   均为**预期内的已登记偏离**（非回归）：`getTimezoneOffset()` 保持返回 `0`。
//!
//! ## 分派接线
//!
//! 实例方法 NativeFn 挂在 `Date.prototype`（`date_proto`）上，并以**双键**登记
//! 分派表（见 `builtins/global/mod.rs` 的 Date 段）：
//! - `"Date.{m}"`：实例 `CALL_METHOD` 路径（`try_dispatch` 按 `_builtinNs` 拼键）；
//! - `"Date.prototype.{m}"`：`Date.prototype.getTime.call(d)` 形态
//!   （`invoke_callable` 按 NativeFn 全名查表）。
//!
//! 方法名取真统一走 `pending_native_name()` 末段——**不再**从接收者堆变体取
//! （接收者是 Date 实例 `Ordinary`，此前恒取到空名 → 所有方法静默返回 undefined）。

use crate::builtins::current_receiver;
use crate::interpreter::{Vm, VmError};
use crate::value::{Value, ValueCase};

/// 时间值上界（ECMA-262 `TimeClip`：|t| > 8.64e15 → NaN）。
const MAX_TIME_VALUE: f64 = 8_640_000_000_000_000.0;

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

/// `Date.UTC(year, month[, date[, hours[, minutes[, seconds[, ms]]]]])`。
///
/// 月份 **0-based**；缺省 `date=1`、`h/mi/s/ms=0`；年份 `0..=99` 映射为
/// `1900+y`；越界范围（`TimeClip`）→ NaN。`undefined` 实参按 ToNumber → NaN
/// （仅**缺位**实参取缺省值，与 Node 一致：`Date.UTC(1970, undefined)` → NaN）。
pub(crate) fn date_utc(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    let year = arg_number(vm, args, 0, f64::NAN);
    // 缺位 month 按 +0（Node：`Date.UTC(1970)` → 0）
    let month = arg_number(vm, args, 1, 0.0);
    let day = arg_number(vm, args, 2, 1.0);
    let hours = arg_number(vm, args, 3, 0.0);
    let minutes = arg_number(vm, args, 4, 0.0);
    let seconds = arg_number(vm, args, 5, 0.0);
    let millis = arg_number(vm, args, 6, 0.0);
    Ok(Value::Number(make_date_ms(
        two_digit_year(year),
        month,
        day,
        hours,
        minutes,
        seconds,
        millis,
    )))
}

/// 实参 ToNumber（`i` 缺位时取 `default`）。
fn arg_number(vm: &mut Vm, args: &[Value], i: usize, default: f64) -> f64 {
    match args.get(i) {
        Some(v) => vm.to_number_value(*v),
        None => default,
    }
}

/// `Date.prototype` 实例方法统一分派（`CALL_METHOD` 与 `.call` 形态同源）。
pub(crate) fn date_instance_method(vm: &mut Vm, args: &[Value]) -> Result<Value, VmError> {
    // 方法名取真：分派键末段（"Date.getTime" / "Date.prototype.getTime" → "getTime"）
    let method = crate::builtins::pending_native_name()
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_owned();
    let receiver = current_receiver();
    let t = date_time_value(vm, receiver);
    // 非 Date 接收者（`Date.prototype.getTime.call({})`）→ TypeError
    let is_date = matches!(receiver.case(), ValueCase::Object(r) if vm.has_own_slot(r.0 as usize, "_isDate"));
    if !is_date {
        let msg = if method == "toJSON" {
            "toISOString is not a function"
        } else {
            "this is not a Date object."
        };
        return Err(error_throw(vm, "TypeError", msg));
    }
    match method.as_str() {
        "getTime" | "valueOf" => Ok(Value::Number(t)),
        // 已登记偏离：本地 = UTC（偏移 0）
        "getTimezoneOffset" => Ok(Value::Number(0.0)),
        "toISOString" => {
            if t.is_nan() {
                // Node 22 文本：RangeError: Invalid time value
                Err(error_throw(vm, "RangeError", "Invalid time value"))
            } else {
                Ok(vm_string(vm, to_iso_string(t)))
            }
        }
        // 等价 `toISOString()`，但 Invalid Date 返回 null（不抛）
        "toJSON" => {
            if t.is_nan() {
                Ok(Value::Null)
            } else {
                Ok(vm_string(vm, to_iso_string(t)))
            }
        }
        "toString" => Ok(vm_string(vm, to_local_string(t))),
        "toDateString" => Ok(vm_string(vm, to_date_string(t))),
        "toTimeString" => Ok(vm_string(vm, to_time_string(t))),
        "toUTCString" => Ok(vm_string(vm, to_utc_string(t))),
        // toLocale*：时区与 locale 数据不可得，按「本地 = UTC」口径输出
        // （已登记偏离：Node 形如 "1/1/1970, 12:00:00 AM"）
        "toLocaleString" => Ok(vm_string(vm, to_local_string(t))),
        "toLocaleDateString" => Ok(vm_string(vm, to_date_string(t))),
        "toLocaleTimeString" => Ok(vm_string(vm, to_time_string(t))),
        // get*（本地类与 UTC 类同一实现：本地 = UTC，见模块文档）
        "getFullYear" | "getUTCFullYear" => Ok(Value::Number(part(t, |p| p.year))),
        "getMonth" | "getUTCMonth" => Ok(Value::Number(part(t, |p| p.month))),
        "getDate" | "getUTCDate" => Ok(Value::Number(part(t, |p| p.day))),
        "getDay" | "getUTCDay" => Ok(Value::Number(part(t, |p| p.weekday))),
        "getHours" | "getUTCHours" => Ok(Value::Number(part(t, |p| p.hour))),
        "getMinutes" | "getUTCMinutes" => Ok(Value::Number(part(t, |p| p.minute))),
        "getSeconds" | "getUTCSeconds" => Ok(Value::Number(part(t, |p| p.second))),
        "getMilliseconds" | "getUTCMilliseconds" => Ok(Value::Number(part(t, |p| p.ms))),
        // 已登记偏离（本地 = UTC 下的同值）：Node 为 year-1900
        "getYear" => Ok(Value::Number(part(t, |p| p.year - 1900))),
        "setTime" => {
            let v = arg_number(vm, args, 0, f64::NAN);
            Ok(Value::Number(set_time_value(vm, receiver, time_clip(v))))
        }
        // setFullYear / setUTCFullYear：t 为 NaN 时按 +0 起算（Node 实测：
        // `new Date("invalid").setFullYear(2000)` → 946656000000）
        "setFullYear" | "setUTCFullYear" => {
            let base = if t.is_nan() { 0.0 } else { t };
            let p = date_parts(base).expect("base 已非 NaN");
            let year = arg_number(vm, args, 0, f64::NAN);
            let month = arg_number(vm, args, 1, p.month as f64);
            let day = arg_number(vm, args, 2, p.day as f64);
            let new_t = make_date_ms(
                year,
                month,
                day,
                p.hour as f64,
                p.minute as f64,
                p.second as f64,
                p.ms as f64,
            );
            Ok(Value::Number(set_time_value(vm, receiver, new_t)))
        }
        "setMonth" | "setUTCMonth" => {
            let p = date_parts(t);
            let month = arg_number(vm, args, 0, f64::NAN);
            let day = arg_number(vm, args, 1, part(t, |p| p.day));
            let new_t = match p {
                Some(p) => make_date_ms(
                    p.year as f64,
                    month,
                    day,
                    p.hour as f64,
                    p.minute as f64,
                    p.second as f64,
                    p.ms as f64,
                ),
                None => f64::NAN,
            };
            Ok(Value::Number(set_time_value(vm, receiver, new_t)))
        }
        "setDate" | "setUTCDate" => {
            let p = date_parts(t);
            let day = arg_number(vm, args, 0, f64::NAN);
            let new_t = match p {
                Some(p) => make_date_ms(
                    p.year as f64,
                    p.month as f64,
                    day,
                    p.hour as f64,
                    p.minute as f64,
                    p.second as f64,
                    p.ms as f64,
                ),
                None => f64::NAN,
            };
            Ok(Value::Number(set_time_value(vm, receiver, new_t)))
        }
        "setHours" | "setUTCHours" => {
            let p = date_parts(t);
            let hours = arg_number(vm, args, 0, f64::NAN);
            let minutes = arg_number(vm, args, 1, part(t, |p| p.minute));
            let seconds = arg_number(vm, args, 2, part(t, |p| p.second));
            let millis = arg_number(vm, args, 3, part(t, |p| p.ms));
            let new_t = match p {
                Some(p) => make_date_ms(
                    p.year as f64,
                    p.month as f64,
                    p.day as f64,
                    hours,
                    minutes,
                    seconds,
                    millis,
                ),
                None => f64::NAN,
            };
            Ok(Value::Number(set_time_value(vm, receiver, new_t)))
        }
        "setMinutes" | "setUTCMinutes" => {
            let p = date_parts(t);
            let minutes = arg_number(vm, args, 0, f64::NAN);
            let seconds = arg_number(vm, args, 1, part(t, |p| p.second));
            let millis = arg_number(vm, args, 2, part(t, |p| p.ms));
            let new_t = match p {
                Some(p) => make_date_ms(
                    p.year as f64,
                    p.month as f64,
                    p.day as f64,
                    p.hour as f64,
                    minutes,
                    seconds,
                    millis,
                ),
                None => f64::NAN,
            };
            Ok(Value::Number(set_time_value(vm, receiver, new_t)))
        }
        "setSeconds" | "setUTCSeconds" => {
            let p = date_parts(t);
            let seconds = arg_number(vm, args, 0, f64::NAN);
            let millis = arg_number(vm, args, 1, part(t, |p| p.ms));
            let new_t = match p {
                Some(p) => make_date_ms(
                    p.year as f64,
                    p.month as f64,
                    p.day as f64,
                    p.hour as f64,
                    p.minute as f64,
                    seconds,
                    millis,
                ),
                None => f64::NAN,
            };
            Ok(Value::Number(set_time_value(vm, receiver, new_t)))
        }
        "setMilliseconds" | "setUTCMilliseconds" => {
            let p = date_parts(t);
            let millis = arg_number(vm, args, 0, f64::NAN);
            let new_t = match p {
                Some(p) => make_date_ms(
                    p.year as f64,
                    p.month as f64,
                    p.day as f64,
                    p.hour as f64,
                    p.minute as f64,
                    p.second as f64,
                    millis,
                ),
                None => f64::NAN,
            };
            Ok(Value::Number(set_time_value(vm, receiver, new_t)))
        }
        _ => Ok(Value::Undefined),
    }
}

/// 构造错误实例（`name` 对齐 Node 文本），供抛 `RangeError` / `TypeError`。
fn error_throw(vm: &mut Vm, name: &str, msg: &str) -> VmError {
    let err = vm.alloc_error_instance(msg);
    let n = vm.alloc_string(name.to_owned());
    let _ = vm.set_property(Value::Object(err), "name", Value::Object(n));
    VmError::Thrown(Value::Object(err))
}

fn vm_string(vm: &mut Vm, s: String) -> Value {
    Value::Object(vm.alloc_string(s))
}

/// 读取接收者的 `_timeValue`（非 Date 接收者 → NaN）。
fn date_time_value(vm: &Vm, v: Value) -> f64 {
    match v.case() {
        ValueCase::Object(r) => match vm.own_value(r.0 as usize, "_timeValue").map(|v| v.case()) {
            Some(ValueCase::Number(n)) => n,
            _ => f64::NAN,
        },
        _ => f64::NAN,
    }
}

/// 写回接收者的 `_timeValue` 并返回新的时间值（set* 返回值即新时间值）。
fn set_time_value(vm: &mut Vm, receiver: Value, t: f64) -> f64 {
    if let Some(r) = receiver.as_object() {
        let _ = vm.set_property(Value::Object(r), "_timeValue", Value::Number(t));
    }
    t
}

impl Vm {
    /// `new Date([value])` / `new Date(y, m[, d[, h[, mi[, s[, ms]]]]])`。
    ///
    /// 单实参：数值按 `TimeClip`，字符串按 ISO 解析（`Date.parse` 同一口径）。
    /// **多实参**（≥2）：按本地时间口径构造——本地 = UTC（见模块文档），
    /// 月份 0-based、年份 `0..=99` 映射 `1900+y`。
    pub(crate) fn construct_date(&mut self, args: &[Value]) -> Result<Value, VmError> {
        let time = if args.len() >= 2 {
            let year = self.to_number_value(args[0]);
            let month = self.to_number_value(args[1]);
            let day = arg_number(self, args, 2, 1.0);
            let hours = arg_number(self, args, 3, 0.0);
            let minutes = arg_number(self, args, 4, 0.0);
            let seconds = arg_number(self, args, 5, 0.0);
            let millis = arg_number(self, args, 6, 0.0);
            make_date_ms(
                two_digit_year(year),
                month,
                day,
                hours,
                minutes,
                seconds,
                millis,
            )
        } else {
            match args.first().map(|v| v.case()) {
                None | Some(Value::Undefined) => date_now_ms(),
                Some(ValueCase::Number(n)) => time_clip(n),
                // null / true / false：ToPrimitive 后退化为数值（Node 实测）
                Some(Value::Null) => 0.0,
                Some(ValueCase::Boolean(b)) => {
                    if b {
                        1.0
                    } else {
                        0.0
                    }
                }
                Some(other) => {
                    let text = self.format_value(other);
                    parse_iso_date(&text).unwrap_or(f64::NAN)
                }
            }
        };
        // `[[Prototype]]` 指向 Date.prototype：`d.getTime` / `d instanceof Date` /
        // `d.constructor === Date` 均沿原型链判定（`alloc_ordinary_with_proto`
        // 在 `vm.date_proto` 缺失时回退 Object.prototype，保持旧行为不至于崩）
        let inst = match self.date_proto {
            Some(p) => self.alloc_ordinary_with_exact_proto(Some(p)),
            None => self.alloc_ordinary(),
        };
        // 内部标记：值为可读槽位但**不可枚举**（`Object.keys(d)` → `[]`，
        // 对齐 Node；分派读 `_builtinNs` 走 `own_value` 原始槽，不受影响）
        let ns = self.alloc_string("Date".to_owned());
        self.define_proto_method(Value::Object(inst), "_builtinNs", Value::Object(ns))?;
        self.define_proto_method(Value::Object(inst), "_isDate", Value::Boolean(true))?;
        self.define_proto_method(Value::Object(inst), "_timeValue", Value::Number(time))?;
        Ok(Value::Object(inst))
    }
}

/// JS 时间值 → 日历分量（UTC 口径；本地 = UTC，见模块文档）。
struct DateParts {
    year: i64,
    /// 0-based
    month: i64,
    day: i64,
    /// 0 = 周日
    weekday: i64,
    hour: i64,
    minute: i64,
    second: i64,
    ms: i64,
}

/// NaN / 非有限 / 超 `TimeClip` 范围 → `None`（此时各 get* 返回 NaN）。
fn date_parts(t: f64) -> Option<DateParts> {
    if !t.is_finite() || t.abs() > MAX_TIME_VALUE {
        return None;
    }
    let days = (t / 86_400_000.0).floor() as i64;
    let ms_of_day = (t - days as f64 * 86_400_000.0).round() as i64;
    let (year, month, day) = civil_from_days(days);
    Some(DateParts {
        year,
        month,
        day,
        weekday: (days + 4).rem_euclid(7),
        hour: ms_of_day / 3_600_000,
        minute: (ms_of_day % 3_600_000) / 60_000,
        second: (ms_of_day % 60_000) / 1000,
        ms: ms_of_day % 1000,
    })
}

/// 分量取值；Invalid Date → NaN。
fn part(t: f64, pick: impl Fn(&DateParts) -> i64) -> f64 {
    date_parts(t).map(|p| pick(&p) as f64).unwrap_or(f64::NAN)
}

/// `MakeDay` + `MakeTime` + `TimeClip` 合体（UTC 口径，月份 0-based 且允许越界）。
fn make_date_ms(year: f64, month: f64, day: f64, h: f64, mi: f64, s: f64, ms: f64) -> f64 {
    if !(year.is_finite()
        && month.is_finite()
        && day.is_finite()
        && h.is_finite()
        && mi.is_finite()
        && s.is_finite()
        && ms.is_finite())
    {
        return f64::NAN;
    }
    // ToIntegerOrInfinity（向零截断）：Date.UTC(1970.9, 0.9, 1.9) → 0
    let (year, month, day) = (year.trunc(), month.trunc(), day.trunc());
    let (h, mi, s, ms) = (h.trunc(), mi.trunc(), s.trunc(), ms.trunc());
    let ym = year + (month / 12.0).floor();
    let mn = month - 12.0 * (month / 12.0).floor();
    // 远超 TimeClip 上界（±273790 年）→ NaN，同时保证 i64 日历运算不溢出
    if ym.abs() > 300_000.0 {
        return f64::NAN;
    }
    let days = days_from_civil(ym as i64, mn as i64 + 1, 1) as f64 + (day - 1.0);
    time_clip(days * 86_400_000.0 + h * 3_600_000.0 + mi * 60_000.0 + s * 1000.0 + ms)
}

/// 两位年份映射（仅 `new Date(y, ...)` 与 `Date.UTC`）：`0..=99` → `1900+y`。
fn two_digit_year(y: f64) -> f64 {
    if (0.0..=99.0).contains(&y) {
        1900.0 + y
    } else {
        y
    }
}

/// `TimeClip`：非有限或 |t| > 8.64e15 → NaN，否则向零截断。
fn time_clip(t: f64) -> f64 {
    if !t.is_finite() || t.abs() > MAX_TIME_VALUE {
        f64::NAN
    } else {
        t.trunc()
    }
}

/// 公历日期 → 相对 1970-01-01 的天数（`m` 为 **1-based** 月份）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// 天数 → 公历年月日（`month` 为 0-based）。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m - 1, d)
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// ISO 8601 年份：`0000..=9999` 四位定长，其余带符号六位
/// （Node：`new Date(8.64e15).toISOString()` → `+275760-09-13T...`）。
fn fmt_year(y: i64) -> String {
    if (0..=9999).contains(&y) {
        format!("{y:04}")
    } else if y > 9999 {
        format!("+{y:06}")
    } else {
        format!("-{:06}", -y)
    }
}

/// `YYYY-MM-DDTHH:mm:ss.sssZ`（调用方须先排除 Invalid Date）。
fn to_iso_string(t: f64) -> String {
    let p = date_parts(t).expect("调用方已排除 Invalid Date");
    format!(
        "{}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        fmt_year(p.year),
        p.month + 1,
        p.day,
        p.hour,
        p.minute,
        p.second,
        p.ms
    )
}

/// `Thu, 01 Jan 1970 00:00:00 GMT`（确定、无时区依赖）。
fn to_utc_string(t: f64) -> String {
    match date_parts(t) {
        None => "Invalid Date".to_owned(),
        Some(p) => format!(
            "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
            WEEKDAYS[p.weekday as usize],
            p.day,
            MONTHS[p.month as usize],
            fmt_year(p.year),
            p.hour,
            p.minute,
            p.second
        ),
    }
}

/// `Thu Jan 01 1970`（本地口径 = UTC，已登记偏离）。
fn to_date_string(t: f64) -> String {
    match date_parts(t) {
        None => "Invalid Date".to_owned(),
        Some(p) => format!(
            "{} {:02} {} {}",
            WEEKDAYS[p.weekday as usize],
            p.day,
            MONTHS[p.month as usize],
            fmt_year(p.year)
        ),
    }
}

/// `00:00:00 GMT+0000 (Coordinated Universal Time)`（本地口径 = UTC，已登记偏离）。
fn to_time_string(t: f64) -> String {
    match date_parts(t) {
        None => "Invalid Date".to_owned(),
        Some(p) => format!(
            "{:02}:{:02}:{:02} GMT+0000 (Coordinated Universal Time)",
            p.hour, p.minute, p.second
        ),
    }
}

/// `Thu Jan 01 1970 00:00:00 GMT+0000 (Coordinated Universal Time)`
/// （本地口径 = UTC，已登记偏离：非 UTC 机器上 Node 输出带本地偏移与时区名）。
fn to_local_string(t: f64) -> String {
    match date_parts(t) {
        None => "Invalid Date".to_owned(),
        Some(p) => format!(
            "{} {:02} {} {} {:02}:{:02}:{:02} GMT+0000 (Coordinated Universal Time)",
            WEEKDAYS[p.weekday as usize],
            p.day,
            MONTHS[p.month as usize],
            fmt_year(p.year),
            p.hour,
            p.minute,
            p.second
        ),
    }
}

/// ISO 8601 子集解析（`YYYY-MM-DD[THH:MM[:SS[.sss]]][Z]`），失败 → None。
///
/// 仅支持显式偏移 0 的形态（`Z` 或省略时按 UTC——本地 = UTC，见模块文档）。
fn parse_iso_date(text: &str) -> Option<f64> {
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
    }
    let days = days_from_civil(y, m, d) as f64;
    Some(time_clip(
        days * 86_400_000.0 + (hh * 3600 + mm * 60 + ss) as f64 * 1000.0,
    ))
}
