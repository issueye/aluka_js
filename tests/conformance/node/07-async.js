// Node.js 官方测试风格子集：async/await + timers + queueMicrotask。
const assert = require('node:assert');

async function main() {
  // Promise 链。
  const v = await Promise.resolve(42);
  assert.strictEqual(v, 42);

  // async 函数。
  const double = async (n) => n * 2;
  assert.strictEqual(await double(21), 42);

  // 并发。
  const [a, b] = await Promise.all([Promise.resolve(1), Promise.resolve(2)]);
  assert.strictEqual(a + b, 3);

  // queueMicrotask。
  let order = [];
  await new Promise((resolve) => {
    queueMicrotask(() => order.push('micro'));
    order.push('sync');
    setTimeout(() => { order.push('timer'); resolve(); }, 5);
  });
  assert.strictEqual(order[0], 'sync');
  assert.strictEqual(order[1], 'micro');
  assert.strictEqual(order[2], 'timer');

  // 定时器清理。
  let n = 0;
  const iv = setInterval(() => { n++; if (n === 2) clearInterval(iv); }, 5);
  await new Promise((r) => setTimeout(r, 30));
  assert.strictEqual(n, 2);

  console.log('PASS async');
}

main().catch((e) => { console.error('FAIL', e); process.exit(1); });
