const proto = { inherited: 1 };
const child = Object.create(proto);
child.ownA = 2;
child.ownB = 3;
const keys = [];
for (const k in child) {
    keys.push(k);
}
console.log(keys.sort().join(","));
