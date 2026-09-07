function Point(x, y) {
    this.x = x;
    this.y = y;
}
Point.prototype.dist = function() {
    return Math.sqrt(this.x * this.x + this.y * this.y);
};
const p = new Point(3, 4);
console.log(p.dist());
