const re = /([a-z]+)-(\d+)/i;
const match = re.exec("Order-123");
console.log(match ? match[0] : "none");
console.log(typeof "text", typeof 123, typeof undefined, typeof true);
