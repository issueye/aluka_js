var s=Symbol('d');
console.log(JSON.stringify([typeof s, s.description, String(s), s.toString()]));
var o={[s]:1};
console.log(JSON.stringify([o[s], Object.getOwnPropertySymbols(o).length]));
console.log(JSON.stringify([Symbol.for('k')===Symbol.for('k'), Symbol.keyFor(Symbol.for('k'))]));
console.log(JSON.stringify([typeof Symbol.iterator, typeof Symbol.asyncIterator]));
console.log(JSON.stringify([typeof Symbol.dispose, typeof Symbol.asyncDispose]));
console.log(JSON.stringify([Symbol.iterator.toString(), Symbol().description===undefined]));
