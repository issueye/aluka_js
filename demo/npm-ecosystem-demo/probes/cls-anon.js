const mk = () => class { constructor() { this.x = 42; } };
const C = mk();
console.log('ANON-CLASS', new C().x);
