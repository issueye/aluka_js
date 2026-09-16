// chalk/color-convert 根因复现：上下文关键字形参 + for-of 逐次绑定隔离
function t(label, fn) {
  try {
    console.log(`${label}=${JSON.stringify(fn())}`);
  } catch (err) {
    console.log(`${label}=ERR ${err && err.name}: ${err && err.message}`);
  }
}

// 1. `from` 作为形参名（color-convert route.js 的 link 函数形态）
t('kwParam', () => {
  const link = function (from, to) {
    return function (args) {
      return to(from(args));
    };
  };
  return link(x => x + 1, x => x * 2)(5);
});

// 2. for-of 解构 const 绑定的逐次迭代隔离（chalk styles 循环形态）
t('forOfPerIter', () => {
  const fns = [];
  for (const [a, b] of [[1, 'one'], [2, 'two'], [3, 'three']]) {
    fns.push(() => b);
  }
  return fns.map(f => f()).join(',');
});

// 3. for-of 普通标识符 const 绑定
t('forOfIdentPerIter', () => {
  const fns = [];
  for (const x of [10, 20, 30]) {
    fns.push(() => x);
  }
  return fns.map(f => f()).join(',');
});

// 4. 对照：普通块内 const（每次新建绑定）
t('blockConstPerIter', () => {
  const fns = [];
  for (const x of [1, 2, 3]) {
    const y = x * 100;
    fns.push(() => y);
  }
  return fns.map(f => f()).join(',');
});
console.log('BINDING_DONE');
