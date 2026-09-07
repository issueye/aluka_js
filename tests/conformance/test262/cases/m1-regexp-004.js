
var r = "2026-08-07".replace(/(?<=-)\d+/g, "XX");
assert.sameValue(r, "2026-XX-XX", "lookbehind in replace");
