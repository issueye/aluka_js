// Node.js 官方测试风格子集：crypto + zlib。
const crypto = require('node:crypto');
const zlib = require('node:zlib');
const assert = require('node:assert');

// 哈希。
assert.strictEqual(
  crypto.createHash('sha256').update('abc').digest('hex'),
  'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
);

// HMAC。
assert.strictEqual(
  crypto.createHmac('sha256', 'key').update('message').digest('hex').length,
  64
);

// 对称加密往返。
const key = Buffer.alloc(32, 7);
const iv = Buffer.alloc(16, 3);
const enc = crypto.createCipheriv('aes-256-cbc', key, iv);
enc.update('confidential');
const ciphertext = enc.final();
const dec = crypto.createDecipheriv('aes-256-cbc', key, iv);
dec.update(ciphertext);
assert.strictEqual(dec.final().toString(), 'confidential');

// PBKDF2。
const dk = crypto.pbkdf2Sync('password', 'salt', 1000, 32, 'sha256');
assert.strictEqual(dk.length, 32);

// scrypt（RFC 7914 向量：password/NaCl, N=1024, r=8, p=16）。
assert.strictEqual(
  crypto.scryptSync('password', 'NaCl', 64, { N: 1024, r: 8, p: 16 }).toString('hex'),
  'fdbabe1c9d3472007856e7190d01e9fe7c6ad7cbc8237830e77376634b3731622eaf30d92e22a3886ff109279d9830dac727afb94a83ee6d8360cbdfa2cc0640'
);

// randomBytes 返回 Buffer。
assert.ok(Buffer.isBuffer(crypto.randomBytes(16)));

// zlib 往返。
const data = Buffer.from('compress me compress me');
const gz = zlib.gzipSync(data);
assert.ok(gz.length > 0);
assert.strictEqual(zlib.gunzipSync(gz).toString(), data.toString());
const def = zlib.deflateSync(data);
assert.strictEqual(zlib.inflateSync(def).toString(), data.toString());

// brotli 往返。
const br = zlib.brotliCompressSync(data);
assert.strictEqual(zlib.brotliDecompressSync(br).toString(), data.toString());

console.log('PASS crypto-zlib');
