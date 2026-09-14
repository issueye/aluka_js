var r=/a(b)c/g;
console.log(JSON.stringify(["abc".match(r), "abc".match(/a(b)c/), r.source, r.flags, r.global]));
console.log(JSON.stringify(["a1b2".replace(/\d/g,'#'), "a1b2".split(/\d/)]));
console.log(JSON.stringify(["abc".search(/b/), /b/.test("abc"), /x/.exec("abc")]));
console.log(JSON.stringify([..."a1b2c3".matchAll(/([a-z])(\d)/g)].map(m=>m[0]+':'+m[1])));
console.log(JSON.stringify([/a/i.test("A"), "aAa".replace(/a/gi,'-')]));
console.log(JSON.stringify(["x".replace(/(x)/,'[$1]'), "abc".replace(/b/,(m,o)=>o)]));
console.log(JSON.stringify([new RegExp("a+","gi").toString(), /(\d)(\d)/.exec("12").slice(1)]));
