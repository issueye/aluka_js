
function f() { var arr = [1, 2]; eval("arr.push(3)"); return arr.join(","); }
assert.sameValue(f(), "1,2,3", "mutates object through eval");
