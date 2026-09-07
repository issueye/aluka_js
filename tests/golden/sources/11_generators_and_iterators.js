function* countUp(max) {
    for (let i = 1; i <= max; i++) {
        yield i;
    }
}
const gen = countUp(3);
let sum = 0;
for (const v of gen) {
    sum += v;
}
console.log("sum:", sum);
