class Base { hello() { return 'base-' + this.name; } }
const mk = (n) => class extends Base { name = n; };
const C = mk('x');
console.log('NAMED-EXTENDS', new C().hello());
