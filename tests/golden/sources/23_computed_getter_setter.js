const sym = "computedProp";
const obj = {
    _val: 0,
    get [sym]() { return this._val; },
    set [sym](v) { this._val = v * 2; }
};
obj[sym] = 5;
console.log(obj[sym]);

const base = { greet() { return "hi"; } };
const derived = { __proto__: base, extra: 1 };
console.log(derived.greet());
console.log(derived.extra);
