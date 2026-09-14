var a=[3,1,2];
console.log(JSON.stringify([a.sort(), a.map(x=>x*2), a.filter(x=>x>1), a.reduce((s,x)=>s+x,0)]));
console.log(JSON.stringify([a.find(x=>x>1), a.findIndex(x=>x>1), a.includes(2), a.indexOf(2)]));
console.log(JSON.stringify([a.slice(1), a.concat([9]), a.join('-'), a.slice().reverse()]));
console.log(JSON.stringify([a.fill(0,1,2), a.slice().copyWithin(0,1), a.flat(), [[1,[2]]].flat(2)]));
console.log(JSON.stringify([a.at(-1), a.flatMap(x=>[x,x]), a.some(x=>x>2), a.every(x=>x>-1)]));
console.log(JSON.stringify([Array.from({length:3},(_,i)=>i), Array.of(1,2), Array.isArray(a)]));
console.log(JSON.stringify([a.lastIndexOf(2), a.toString(), [1,2,3].reduceRight((s,x)=>s+''+x)]));
