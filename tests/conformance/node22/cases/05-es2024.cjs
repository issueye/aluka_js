// N22-C1/C2/C3：ES2024 全局与方法（避开 surrogate 字面量——aluka 字节字符串
// 模型在 lexer 层替换孤立 surrogate，为已知架构差异）。
async function main() {
  const out = [];
  // withResolvers
  const { promise, resolve } = Promise.withResolvers();
  promise.then(v => out.push('wr:' + v));
  resolve(42);
  // fromAsync（同步 + async 迭代器）
  await Array.fromAsync([1, 2, 3]).then(a => out.push('fa:' + a.join(',')));
  await Array.fromAsync((async function* () { yield 'x'; yield 'y'; })()).then(a => out.push('faa:' + a.join(',')));
  // groupBy
  const g = Object.groupBy([1, 2, 3, 4, 5], x => x % 2 === 0 ? 'even' : 'odd');
  out.push('og:' + g.odd.join(',') + '|' + g.even.join(','));
  const mg = Map.groupBy([1, 2, 3, 4], x => x % 2);
  out.push('mg:' + mg.get(0).join(',') + '|' + mg.get(1).join(','));
  // ES2023 不可变数组方法
  out.push('ts:' + [3, 1, 2].toSorted().join(','));
  out.push('tr:' + [1, 2, 3].toReversed().join(','));
  out.push('tsp:' + [1, 2, 3, 4].toSpliced(1, 2, 9).join(','));
  out.push('tw:' + [1, 2, 3].with(1, 9).join(','));
  // hasOwn + isWellFormed（正常字符串）
  out.push('ho:' + Object.hasOwn({ a: 1 }, 'a') + Object.hasOwn({ a: 1 }, 'b'));
  out.push('wf:' + 'abc'.isWellFormed() + '|' + 'abc'.toWellFormed());
  // 微任务后输出（withResolvers 的 then）
  await Promise.resolve();
  setTimeout(() => console.log('result: ' + out.join(';')), 5);
}
main().catch(e => console.log('FAIL: ' + e.message));
