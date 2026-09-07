const obj = { a: { b: 42 }, fn: () => "ok" };
const val1 = obj?.a?.b;
const val2 = obj?.missing?.b;
const val3 = obj?.fn?.();
const val4 = obj?.nonFn?.();
console.log(val1, val2, val3, val4);
