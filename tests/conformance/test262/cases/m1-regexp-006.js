
var r = "2026-08".replace(/(?<y>\d{4})-(?<m>\d{2})/, "$<m>/$<y>");
assert.sameValue(r, "08/2026", "named group replacement");
