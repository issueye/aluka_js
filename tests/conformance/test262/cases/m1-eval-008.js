
var threw = false;
try { eval("function("); } catch (e) { threw = e.name === "SyntaxError"; }
assert.isTrue(threw, "eval syntax error throws SyntaxError");
