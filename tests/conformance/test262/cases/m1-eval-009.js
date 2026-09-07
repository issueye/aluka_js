
var threw = false;
try { new Function("return ((("); } catch (e) { threw = true; }
assert.isTrue(threw, "new Function invalid body throws");
