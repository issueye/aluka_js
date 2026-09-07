const key = "dynamicKey";
const o = {
    x: 1,
    y: 2,
    [key]: 3,
    get prop() { return this.x * 10; },
    set prop(v) { this.x = v; }
};
o.x = 42;
o[key] = 99;
delete o.y;
delete o["missing"];
console.log(o.x, o.prop, o[key]);
