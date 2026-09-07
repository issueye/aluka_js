/*---
esid: local-regexp
description: RegExp 内置对象与正则匹配
---*/

// 字面量匹配。
assert.sameValue(/ab+c/.test("abbbc"), true, "test match");
assert.sameValue(/ab+c/.test("ac"), false, "test no match");
assert.sameValue(/a\/b/.test("a/b"), true, "escaped slash");
assert.sameValue(/^v?(\d+)\.(\d+)\.(\d+)$/.test("v1.2.3"), true, "semver pattern");

// 标志位。
assert.sameValue(/hello/i.test("HELLO"), true, "ignoreCase");
assert.sameValue(/a.b/s.test("a\nb"), true, "dotAll");
assert.sameValue(/^b/m.test("a\nb"), true, "multiline");
assert.sameValue(/a/gi.flags, "gi", "flags order");
assert.sameValue(/a/g.source, "a", "source");
assert.sameValue(new RegExp().source, "(?:)", "empty source");

// exec 捕获组与命名组。
var m = /(\d{4})-(\d{2})/.exec("2026-08");
assert.sameValue(m[0], "2026-08", "exec full");
assert.sameValue(m[1], "2026", "exec group 1");
assert.sameValue(m[2], "08", "exec group 2");
assert.sameValue(m.index, 0, "exec index");
var g = /(?<y>\d{4})-(?<m>\d{2})/.exec("2026-08").groups;
assert.sameValue(g.y, "2026", "named group y");
assert.sameValue(g.m, "08", "named group m");

// lastIndex 与 global。
var re = /\d+/g;
var out = [];
var mm;
while ((mm = re.exec("a1 b22")) !== null) {
  out.push(mm[0]);
}
assert.sameValue(out.join(","), "1,22", "global exec iteration");
assert.sameValue(re.lastIndex, 0, "lastIndex reset after no match");

// 构造器。
assert.sameValue(new RegExp("ab+c").test("abbbc"), true, "new RegExp");
assert.sameValue(RegExp("a").test("a"), true, "RegExp without new");
assert.sameValue(new RegExp(/a/g).flags, "g", "copy flags");
assert.sameValue(new RegExp(/a/g, "i").flags, "i", "override flags");
assert.sameValue(RegExp.prototype.constructor === RegExp, true, "constructor chain");

// String 方法集成。
assert.sameValue("a,b,c".split(/,/).join("|"), "a|b|c", "split regex");
assert.sameValue("a1b2".split(/(\d)/).join("|"), "a|1|b|2|", "split captures");
assert.sameValue("a1b2".replace(/(\d)/g, "[$1]"), "a[1]b[2]", "replace $1");
assert.sameValue("abc".replace(/b/, "<$&>"), "a<b>c", "replace $&");
assert.sameValue("2026-08-04".match(/\d+/g).join("-"), "2026-08-04", "match global");
assert.sameValue("abcde".search(/d/), 3, "search");

// instanceof / typeof。
assert.sameValue(/a/ instanceof RegExp, true, "instanceof");
assert.sameValue(typeof /a/, "object", "typeof");
