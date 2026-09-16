const store = {};
for (const [k, v] of [['a', 'one'], ['b', 'two']]) {
  store[k] = {
    get val() { return v; }
  };
}
console.log('getterCapture=' + store.a.val + ',' + store.b.val);
