let sum = 0;
for (let i = 0; i < 10; i++) {
    if (i % 2 === 0) {
        sum += i;
    } else if (i === 7) {
        break;
    } else {
        continue;
    }
}
let cond = sum > 10 ? "yes" : "no";
let shortOr = false || "fallback";
let shortAnd = true && "passed";
let nullish = null ?? "default";
console.log(sum, cond, shortOr, shortAnd, nullish);
