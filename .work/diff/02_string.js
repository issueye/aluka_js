var s="Hello World";
console.log(JSON.stringify([s.slice(1,4), s.substring(4,1), s.split(' '), s.replace('o','0')]));
console.log(JSON.stringify([s.replaceAll('o','0'), s.padStart(14,'*'), s.padEnd(14,'*'), s.repeat(2)]));
console.log(JSON.stringify([s.trim(), s.toUpperCase(), s.charAt(1), s.charCodeAt(1), s.codePointAt(1)]));
console.log(JSON.stringify([s.indexOf('o'), s.lastIndexOf('o'), s.includes('World'), s.startsWith('He')]));
console.log(JSON.stringify(["a-b-c".split('-',2), "abc".at(-1), "abc".localeCompare("abd")<0]));
console.log(JSON.stringify(["abc".concat("d","e"), "  x  ".trimStart(), "  x  ".trimEnd()]));
console.log(JSON.stringify([s[0], s.length, [..."ab"].length]));
