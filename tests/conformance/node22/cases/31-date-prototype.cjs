// Date.prototype 方法面回归用例（Node 22 对拍；**只含 UTC 确定断言**）。
//
// 改前：`new Date(0).getTime` 等实例方法全缺（undefined / TypeError），
// `Date.UTC` 不存在，`Object.prototype.toString.call(d)` 得 `[object Object]`，
// `Object.keys(d)` 泄漏内部槽 `["_builtinNs","_isDate","_timeValue"]`。
//
// 本用例刻意**不含本地时区断言**（`getFullYear` / `getHours` / `toString` /
// `toLocale*` / `getTimezoneOffset` / 本地 `set*`）：仓库无时区依赖，这些方法按
// 「本地 = UTC（偏移 0）」口径实现，属预期内已登记偏离（见
// `.work/TODO/20260910/README.md §待办 15` 与 `builtins/global/date.rs` 模块文档）。
'use strict';

const show = (label, v) => console.log(label + ':', v);
const fmt = (v) => (typeof v === 'number' && Number.isNaN(v) ? 'NaN' : JSON.stringify(v));

const d0 = new Date(0);
const dBig = new Date(1234567890123);

// ---- 方法存在性 / 身份 / 内部槽不可枚举 ----
show('typeof-getTime', typeof d0.getTime);
show('typeof-getUTCFullYear', typeof d0.getUTCFullYear);
show('typeof-setUTCMilliseconds', typeof d0.setUTCMilliseconds);
show('typeof-Date.UTC', typeof Date.UTC);
show('constructor-name', d0.constructor.name);
show('constructor-is-Date', d0.constructor === Date);
show('instanceof', d0 instanceof Date);
show('tag', Object.prototype.toString.call(d0));
show('keys', fmt(Object.keys(d0)));

// ---- getTime / valueOf（含 Invalid Date 与 `.call` 形态）----
show('getTime-0', d0.getTime());
show('getTime-big', dBig.getTime());
show('getTime-neg1', new Date(-1).getTime());
show('valueOf-0', d0.valueOf());
show('getTime-invalid', fmt(new Date('invalid').getTime()));
show('getTime-call', Date.prototype.getTime.call(dBig));
show('getTime-call-nondate', (() => {
  try { Date.prototype.getTime.call({}); return 'no-throw'; } catch (e) { return e.name; }
})());

// ---- toISOString / toJSON / toUTCString（UTC 确定）----
show('iso-0', d0.toISOString());
show('iso-day', new Date(86400000).toISOString());
show('iso-big', dBig.toISOString());
show('iso-neg1', new Date(-1).toISOString());
show('iso-max', new Date(8640000000000000).toISOString());
show('iso-min', new Date(-8640000000000000).toISOString());
show('iso-invalid', (() => {
  try { new Date('invalid').toISOString(); return 'no-throw'; } catch (e) { return e.name + ': ' + e.message; }
})());
show('json-0', d0.toJSON());
show('json-invalid', fmt(new Date('invalid').toJSON()));
show('utcstr-0', d0.toUTCString());
show('utcstr-big', dBig.toUTCString());
show('utcstr-invalid', new Date('invalid').toUTCString());

// ---- getUTC* ----
show('utc-parts', [
  d0.getUTCFullYear(), d0.getUTCMonth(), d0.getUTCDate(), d0.getUTCDay(),
  d0.getUTCHours(), d0.getUTCMinutes(), d0.getUTCSeconds(), d0.getUTCMilliseconds(),
].join(','));
show('utc-parts-big', [
  dBig.getUTCFullYear(), dBig.getUTCMonth(), dBig.getUTCDate(), dBig.getUTCDay(),
  dBig.getUTCHours(), dBig.getUTCMinutes(), dBig.getUTCSeconds(), dBig.getUTCMilliseconds(),
].join(','));
show('utc-part-invalid', fmt(d0.constructor.prototype.getUTCFullYear.call(new Date('invalid'))));

// ---- Date.UTC（月份 0-based；缺省值；两位年；越界 TimeClip）----
show('UTC-noparen', fmt(Date.UTC()));
show('UTC-year-only', Date.UTC(1970));
show('UTC-epoch', Date.UTC(1970, 0, 1));
show('UTC-full', Date.UTC(1970, 0, 1, 0, 0, 0, 0));
show('UTC-two-digit', Date.UTC(99, 0, 1));
show('UTC-leap', Date.UTC(2024, 1, 29));
show('UTC-month-overflow', Date.UTC(1970, 13));
show('UTC-nan', fmt(Date.UTC(NaN)));
show('UTC-undefined-month', fmt(Date.UTC(1970, undefined)));
show('UTC-trunc', Date.UTC(1970.9, 0.9, 1.9));
show('UTC-max', Date.UTC(275760, 8, 13));
show('UTC-over-max', fmt(Date.UTC(275760, 8, 14)));

// ---- setTime / setUTC*（UTC 语义，返回值即新时间值）----
show('setTime', new Date(0).setTime(1000));
show('setTime-nan', fmt(new Date(0).setTime(NaN)));
show('setUTCFullYear', new Date(0).setUTCFullYear(2000));
show('setUTCFullYear-invalid', new Date('invalid').setUTCFullYear(2000));
show('setUTCMonth', new Date(0).setUTCMonth(1, 15));
show('setUTCDate', new Date(0).setUTCDate(15));
show('setUTCHours', new Date(0).setUTCHours(3, 4, 5, 6));
show('setUTCMinutes', new Date(0).setUTCMinutes(3, 4, 5));
show('setUTCSeconds', new Date(0).setUTCSeconds(3, 4));
show('setUTCMilliseconds', new Date(0).setUTCMilliseconds(7));
show('mutated-iso', (() => {
  const d = new Date(0);
  d.setUTCHours(12);
  return d.toISOString();
})());

// ---- 构造器（单实参 TimeClip / ISO；多参以 getTimezoneOffset 归一）----
show('ctor-num', new Date(0).getTime());
show('ctor-null', new Date(null).getTime());
show('ctor-clip', fmt(new Date(8640000000000001).getTime()));
show('ctor-iso', new Date('1970-01-01T00:00:01Z').getTime());
show('parse-iso', Date.parse('1970-01-01T00:00:01Z'));
show('ctor-multi', new Date(2024, 5, 15, 12, 34, 56, 789).getTime() ===
  Date.UTC(2024, 5, 15, 12, 34, 56, 789) + new Date(2024, 5, 15).getTimezoneOffset() * 60000);
show('ctor-multi-two-digit', new Date(99, 0, 1).getTime() ===
  Date.UTC(99, 0, 1) + new Date(99, 0, 1).getTimezoneOffset() * 60000);
