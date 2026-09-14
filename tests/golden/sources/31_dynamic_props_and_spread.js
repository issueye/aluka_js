function testCallMethodSpread(obj, methodName, ...args) {
    return obj[methodName](...args);
}
const calc = { add(a, b) { return a + b; } };
console.log(testCallMethodSpread(calc, "add", 3, 4));
const direct = calc["add"](10, 5);
console.log(direct);
