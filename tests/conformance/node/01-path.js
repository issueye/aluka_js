// Node.js 官方测试风格子集：path 模块。
const path = require('node:path');
const assert = require('node:assert');

// posix 语义（跨平台稳定）。
assert.strictEqual(path.posix.join('a', 'b', 'c'), 'a/b/c');
assert.strictEqual(path.posix.join('/a', '/b'), '/a/b');
assert.strictEqual(path.posix.resolve('a', 'b'), path.posix.resolve('a') + '/b');
assert.strictEqual(path.posix.basename('/foo/bar/baz.txt'), 'baz.txt');
assert.strictEqual(path.posix.basename('/foo/bar/baz.txt', '.txt'), 'baz');
assert.strictEqual(path.posix.dirname('/foo/bar/baz.txt'), '/foo/bar');
assert.strictEqual(path.posix.extname('index.html'), '.html');
assert.strictEqual(path.posix.extname('.bashrc'), '');
assert.strictEqual(path.posix.isAbsolute('/foo'), true);
assert.strictEqual(path.posix.isAbsolute('foo'), false);
assert.strictEqual(path.posix.sep, '/');

// 本平台 API（Windows 用反斜杠）。
assert.strictEqual(path.join('a', 'b'), 'a' + path.sep + 'b');
assert.strictEqual(typeof path.resolve, 'function');
assert.strictEqual(typeof path.parse, 'function');

console.log('PASS path');
