// Node.js 官方测试风格子集：stream（Readable/Writable/pipeline）。
const { Readable, Writable, pipeline } = require('node:stream');
const assert = require('node:assert');

// Readable from 数组。
const chunks = [];
const src = Readable.from(['a', 'b', 'c']);
src.on('data', (c) => chunks.push(String(c)));
src.on('end', () => {
  assert.strictEqual(chunks.join(''), 'abc');

  // Writable 自定义。
  let written = '';
  const ws = new Writable({
    write(chunk, enc, cb) { written += String(chunk); cb(); }
  });
  ws.on('finish', () => {
    assert.strictEqual(written, 'xyz');
    console.log('PASS stream');
  });
  ws.write('xy');
  ws.end('z');
});
