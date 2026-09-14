function* g(){ yield 1; yield* [2,3]; return 4; }
console.log(JSON.stringify([...g()]));
var it=g(); console.log(JSON.stringify([it.next(), it.next(), it.next(), it.next()]));
var m=new Map([['a',1],['b',2]]);
console.log(JSON.stringify([[...m], [...m.keys()], [...m.values()], m.size, m.get('a')]));
var s=new Set([1,2,2,3]);
console.log(JSON.stringify([[...s], s.size, s.has(2)]));
console.log(JSON.stringify([...[1,2].entries()].map(([i,v])=>i+':'+v)));
function* gen2(){ const x=yield 'q'; yield x*2; }
var g2=gen2(); console.log(JSON.stringify([g2.next(), g2.next(5)]));
