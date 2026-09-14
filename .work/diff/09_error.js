try { null.x; } catch(e) { console.log(JSON.stringify([e.name, e instanceof TypeError, e.message.length>0])); }
try { undefinedFn(); } catch(e) { console.log(JSON.stringify([e.name, e instanceof ReferenceError])); }
try { JSON.parse('{'); } catch(e) { console.log(JSON.stringify([e.name, e instanceof SyntaxError])); }
try { throw new RangeError('r'); } catch(e) { console.log(JSON.stringify([e.name, e.message, e instanceof Error])); }
try { new Array(-1); } catch(e) { console.log(JSON.stringify([e.name])); }
function f(){ try { return 1; } finally { console.log('finally ran'); } }
console.log(f());
class MyErr extends Error { constructor(m){super(m); this.name='MyErr';} }
try { throw new MyErr('custom'); } catch(e) { console.log(JSON.stringify([e.name, e.message, e instanceof MyErr, e instanceof Error])); }
console.log(JSON.stringify([new TypeError('x').constructor===TypeError, Error('e').name]));
