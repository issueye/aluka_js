function outer(a) {
    return function middle(b) {
        return function inner(c) {
            return a + b + c;
        };
    };
}
console.log(outer(1)(2)(3));
