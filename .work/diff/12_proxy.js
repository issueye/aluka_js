var t={a:1};
var p=new Proxy(t,{get(o,k){return k in o?o[k]:'missing';}, set(o,k,v){o[k]=v*2; return true;}});
console.log(JSON.stringify([p.a, p.nope])); p.b=5;
console.log(JSON.stringify([t.b]));
console.log(JSON.stringify([Reflect.get(t,'a'), Reflect.has(t,'a'), Reflect.ownKeys(t)]));
console.log(JSON.stringify([Reflect.construct(Array,[3]).length]));
console.log(JSON.stringify([Reflect.apply(Math.max,null,[1,5,3])]));
console.log(JSON.stringify([Reflect.set(t,'c',9), t.c, Reflect.deleteProperty(t,'c'), 'c' in t]));
console.log(JSON.stringify([Reflect.getPrototypeOf(t)===Object.prototype]));
