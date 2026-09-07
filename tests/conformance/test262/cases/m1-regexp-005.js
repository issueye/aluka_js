
var m = /(?<year>\d{4})-(?<month>\d{2})/.exec("2026-08");
assert.sameValue(m.groups.year, "2026", "named group year");
assert.sameValue(m.groups.month, "08", "named group month");
