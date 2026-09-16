const u = new TextEncoder().encode('hi');
console.log(u instanceof Uint8Array, Object.prototype.toString.call(u), u.constructor && u.constructor.name);
