
var total = eval("var t = 0; var list = [1, 2, 3]; for (var idx = 0; idx < list.length; idx++) { t += list[idx]; } t");
assert.sameValue(total, 6, "loop over array");
