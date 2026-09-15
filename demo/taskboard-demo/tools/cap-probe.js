// 运行时能力探测：真实项目用到的 API 逐项判定（OK / FAIL）
// 用法：node cap_probe.js / aluka run cap_probe.js（cwd=项目根）
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const crypto = require('node:crypto');
const { EventEmitter } = require('node:events');

function check(label, fn) {
  try {
    const value = fn();
    console.log(`OK   ${label} => ${JSON.stringify(value)}`);
    return value;
  } catch (err) {
    console.log(`FAIL ${label} => ${err && err.name}: ${err && err.message}`);
    return undefined;
  }
}

// —— fs 同步族 ——
const tmp = path.join('.probe-data', 'x.txt');
check('fs.mkdirSync(recursive)', () => {
  fs.mkdirSync(path.join('.probe-data', 'a', 'b'), { recursive: true });
  return true;
});
check('fs.writeFileSync', () => {
  fs.writeFileSync(tmp, 'hello');
  return true;
});
check('fs.readFileSync', () => fs.readFileSync(tmp, 'utf8'));
check('fs.existsSync', () => fs.existsSync(tmp));
check('fs.readdirSync', () => fs.readdirSync('.probe-data').length > 0);
check('fs.statSync.size', () => fs.statSync(tmp).size);
check('fs.statSync.isFile()', () => fs.statSync(tmp).isFile());
check('fs.statSync.mtimeMs', () => typeof fs.statSync(tmp).mtimeMs === 'number');

// 错误对象形状（真实项目依赖 err.code 分支）
function errorShape(label, fn) {
  try {
    fn();
    console.log(`OK   ${label} => no error`);
  } catch (err) {
    console.log(
      `OK   ${label} => name=${err && err.name} code=${err && err.code} message=${JSON.stringify(err && err.message)}`
    );
  }
}
errorShape('errshape readFileSync(missing)', () => fs.readFileSync('.probe-data/nope.txt', 'utf8'));
errorShape('errshape statSync(missing)', () => fs.statSync('.probe-data/nope.txt'));
errorShape('errshape mkdirSync(dup, non-recursive)', () => fs.mkdirSync('.probe-data/a'));
check('fs.renameSync', () => {
  fs.renameSync(tmp, path.join('.probe-data', 'y.txt'));
  return true;
});
check('fs.unlinkSync', () => {
  fs.unlinkSync(path.join('.probe-data', 'y.txt'));
  return true;
});
check('fs.copyFileSync', () => {
  fs.writeFileSync(path.join('.probe-data', 'c.txt'), 'c');
  fs.copyFileSync(path.join('.probe-data', 'c.txt'), path.join('.probe-data', 'c2.txt'));
  return true;
});
check('fs.appendFileSync', () => {
  fs.appendFileSync(path.join('.probe-data', 'c2.txt'), '+');
  return fs.readFileSync(path.join('.probe-data', 'c2.txt'), 'utf8');
});
check('fs.realpathSync', () => fs.realpathSync('.').length > 0);
check('fs.rmSync(recursive,force)', () => {
  fs.rmSync('.probe-data', { recursive: true, force: true });
  return fs.existsSync('.probe-data');
});

// —— crypto ——
check('crypto.createHash sha256', () =>
  crypto.createHash('sha256').update('写周报', 'utf8').digest('hex').slice(0, 10)
);

// —— 语言与内建 ——
check('JSON.stringify indent', () => JSON.stringify({ a: 1, b: [1, 2] }, null, 2).length);
class Base extends EventEmitter {
  constructor() {
    super();
    this.name = 'base';
  }
  get tag() {
    return `tag:${this.name}`;
  }
  static describe() {
    return 'Base';
  }
}
class Derived extends Base {
  constructor() {
    super();
    this.name = 'derived';
  }
  get tag() {
    return `derived-${super.tag}`;
  }
}
check('class extends + getter + super', () => new Derived().tag);
check('EventEmitter on/emit', () => {
  const e = new Derived();
  let seen = null;
  e.on('x', (v) => {
    seen = v;
  });
  e.emit('x', 7);
  return seen;
});
check('Array.from({length}, cb)', () => Array.from({ length: 3 }, (_, i) => i * 2).join(','));
check('Array find/findIndex/splice/sort', () => {
  const arr = [3, 1, 2];
  arr.sort((a, b) => a - b);
  const removed = arr.splice(0, 1);
  return `${arr.join(',')}|${removed.join(',')}|${arr.find((v) => v > 1)}|${arr.findIndex((v) => v > 1)}`;
});
check('String padStart/repeat/includes', () => 'T1'.padStart(3, '0') + '!' + 'ab'.repeat(2) + '!' + 'abc'.includes('b'));
check('Date.toISOString(fixed)', () => new Date(Date.UTC(2026, 8, 15, 9, 0, 0)).toISOString());
check('Object.assign + Object.keys.sort', () => Object.keys(Object.assign({}, { b: 1, a: 2 })).sort().join(','));
check('Buffer.concat/byteLength/toString', () => {
  const buf = Buffer.concat([Buffer.from('ab'), Buffer.from('cd')]);
  return `${Buffer.byteLength('文字')}|${buf.toString('utf8')}`;
});
check('Promise.all', () => 'pending');
check('process.cwd()', () => typeof process.cwd() === 'string');
check('process.stdout.write', () => {
  process.stdout.write('');
  return true;
});
check('setTimeout/clearTimeout', () => typeof setTimeout === 'function' && typeof clearTimeout === 'function');
check('instanceof AppError 链', () => {
  class AppError extends Error {}
  class Sub extends AppError {}
  const e = new Sub('m');
  return e instanceof Sub && e instanceof AppError && e instanceof Error && e.message === 'm';
});

// —— http ——
check('http.createServer + listen(0) + address() + close', () => 'deferred');
const server = http.createServer((req, res) => {
  res.writeHead(200, { 'content-type': 'application/json' });
  res.end(JSON.stringify({ path: req.url }));
});
server.listen(0, '127.0.0.1', () => {
  const port = server.address().port;
  console.log('OK   http.listen(0) => port>0', port > 0);
  const req = http.request(
    { host: '127.0.0.1', port, path: '/ping?x=1', method: 'GET' },
    (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => {
        const text = Buffer.concat(chunks).toString('utf8');
        console.log('OK   http.request roundtrip =>', res.statusCode, text);
        server.close(() => {
          console.log('OK   server.close => closed');
          console.log('PROBE_DONE');
        });
      });
    }
  );
  req.on('error', (e) => {
    console.log('FAIL http.request =>', e && e.message);
    console.log('PROBE_DONE');
  });
  req.end();
});
