var r = eval("var r2; try { nonexistentFn(); } catch (e) { r2 = 'recovered'; } r2");
assert.sameValue(r, "recovered", "try/catch inside eval");
