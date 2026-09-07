const o = { a: 1, b: { c: 2 } };
const { a, b: { c } } = o;
const copy = { ...o, added: 99 };
console.log(a, c, copy.added);
