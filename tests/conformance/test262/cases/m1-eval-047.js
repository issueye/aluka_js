
var greet = new Function("name", "return 'hello ' + name;");
assert.sameValue(greet("world"), "hello world", "string concat in body");
