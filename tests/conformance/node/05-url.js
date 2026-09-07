// Node.js 官方测试风格子集：URL（WHATWG）。
const assert = require('node:assert');

const u = new URL('https://user:pass@example.com:8443/path/to?q=1&r=2#frag');
assert.strictEqual(u.protocol, 'https:');
assert.strictEqual(u.hostname, 'example.com');
assert.strictEqual(u.port, '8443');
assert.strictEqual(u.host, 'example.com:8443');
assert.strictEqual(u.pathname, '/path/to');
assert.strictEqual(u.search, '?q=1&r=2');
assert.strictEqual(u.hash, '#frag');
assert.strictEqual(u.username, 'user');
assert.strictEqual(u.origin, 'https://example.com:8443');
assert.strictEqual(u.href, 'https://user:pass@example.com:8443/path/to?q=1&r=2#frag');

// 相对解析。
const rel = new URL('/foo', 'http://example.com/base/');
assert.strictEqual(rel.href, 'http://example.com/foo');

// URLSearchParams。
const sp = new URLSearchParams('a=1&b=2&a=3');
assert.strictEqual(sp.get('a'), '1');
assert.strictEqual(sp.getAll('a').length, 2);
assert.strictEqual(sp.has('b'), true);
assert.strictEqual(sp.size, 3);
sp.append('c', '4');
assert.strictEqual(sp.toString(), 'a=1&a=3&b=2&c=4');

// searchParams 修改同步 URL。
u.searchParams.set('x', '9');
assert.ok(u.search.indexOf('x=9') >= 0);

// 对象/数组初始化。
const sp2 = new URLSearchParams({ k: 'v' });
assert.strictEqual(sp2.get('k'), 'v');
const sp3 = new URLSearchParams([['m', '1'], ['n', '2']]);
assert.strictEqual(sp3.toString(), 'm=1&n=2');

console.log('PASS url');
