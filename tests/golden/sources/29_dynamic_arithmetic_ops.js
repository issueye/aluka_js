function mathOps(a, b) {
    let sub = a - b;
    let div = a / b;
    let notVal = !a;
    let undef = undefined;
    return [sub, div, notVal, undef];
}
console.log(mathOps(10, 2));
