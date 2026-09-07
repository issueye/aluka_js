// N22-A1：for await...of 流迭代（同步与异步数据到达）。
const { Readable } = require('node:stream');
async function main() {
  const src = new Readable({ read() {} });
  src.push('a');
  src.push('b');
  src.push(null);
  let out = '';
  for await (const c of src) { out += c; }
  // 异步到达场景
  const src2 = new Readable({ read() {} });
  setTimeout(() => { src2.push('x'); src2.push(null); }, 5);
  let out2 = '';
  for await (const c of src2) { out2 += c; }
  console.log('result: ' + out + '|' + out2);
}
main().catch(e => console.log('FAIL: ' + e.message));
