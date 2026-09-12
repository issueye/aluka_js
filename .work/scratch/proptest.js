function Point(x, y) { this.x = x; this.y = y; }
Point.prototype.sum = function () { return this.x + this.y; };
const pts = [];
for (let i = 0; i < 40; i++) { pts.push(new Point(i, i * 2)); }
let acc = 0;
for (const p of pts) { acc += p.sum(); }
console.log(acc);
