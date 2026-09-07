// Node.js 官方测试风格子集：Phase 3 Web API。
const assert = require('node:assert');

// Blob。
const blob = new Blob(['hello', ' ', 'world'], { type: 'text/plain' });
assert.strictEqual(blob.size, 11);
assert.strictEqual(blob.type, 'text/plain');

// URLPattern。
const pattern = new URLPattern('/users/:id');
assert.strictEqual(pattern.test('/users/42'), true);
assert.strictEqual(pattern.test('/other'), false);
const match = pattern.exec('/users/99');
assert.strictEqual(match.pathname.groups.id, '99');

// crypto.subtle + randomUUID + getRandomValues。
crypto.subtle.digest('SHA-256', Buffer.from('abc')).then((d) => {
  assert.strictEqual(
    d.toString('hex'),
    'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
  );
  assert.strictEqual(crypto.randomUUID().length, 36);
  assert.strictEqual(crypto.getRandomValues(Buffer.alloc(8)).length, 8);

  // MessageChannel 双向。
  const mc = new MessageChannel();
  mc.port2.onmessage = (e) => {
    assert.strictEqual(e.data, 'ping');
    console.log('PASS webapi');
  };
  mc.port1.postMessage('ping');
});
