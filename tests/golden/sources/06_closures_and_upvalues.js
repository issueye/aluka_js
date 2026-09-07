function makeCounter(init) {
    let count = init;
    return {
        inc: function() { count++; return count; },
        get: function() { return count; }
    };
}
const c = makeCounter(10);
c.inc();
c.inc();
console.log(c.get());

// 作用域局部捕获与循环关闭
const arr = [];
for (let i = 0; i < 3; i++) {
    arr.push(() => i);
}
console.log(arr.map(fn => fn()).join(","));
