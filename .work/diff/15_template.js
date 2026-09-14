var n=5;
console.log(`plain ${n} end`);
function tag(strings,...vals){ return strings.raw.join('|')+'#'+vals.join(','); }
console.log(tag`a${1}b${2}c`);
console.log(String.raw`x\ny`);
console.log(JSON.stringify(`multi
line`));
console.log(JSON.stringify([`${1+1}`, `${"a"}`, `${null}`, `${undefined}`, `${[1,2]}`]));
