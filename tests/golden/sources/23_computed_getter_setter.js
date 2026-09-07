const sym = "computedProp";
const obj = {
    _val: 0,
    get [sym]() { return this._val; },
    set [sym](v) { this._val = v * 2; }
};
obj[sym] = 5;
console.log(obj[sym]);
