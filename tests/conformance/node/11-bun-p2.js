// Phase 4 P2：Aluka.SQL（SQLite 零配置后端）CRUD + tagged template 参数绑定。
// 默认使用 :memory: SQLite，无需外部服务。
const assert = require('node:assert');

async function main() {
  const create = await Aluka.SQL`CREATE TABLE IF NOT EXISTS products (id INTEGER PRIMARY KEY, name TEXT, price REAL)`.run();
  assert.strictEqual(typeof create.changes, 'number');

  await Aluka.SQL`INSERT INTO products (name, price) VALUES ('apple', 1.5)`.run();
  await Aluka.SQL`INSERT INTO products (name, price) VALUES ('banana', 2.5)`.run();

  // all：行对象数组。
  const all = await Aluka.SQL`SELECT * FROM products ORDER BY id`.all();
  assert.strictEqual(all.length, 2);
  assert.strictEqual(all[0].name, 'apple');
  assert.strictEqual(all[1].price, 2.5);

  // get：首行对象。
  const one = await Aluka.SQL`SELECT name FROM products WHERE name = ${'banana'}`.get();
  assert.strictEqual(one.name, 'banana');

  // values：值数组。
  const vals = await Aluka.SQL`SELECT price FROM products ORDER BY id`.values();
  assert.strictEqual(vals[0][0], 1.5);
  assert.strictEqual(vals[1][0], 2.5);

  // 函数形式参数绑定（? 占位符）。
  const byId = await Aluka.SQL('SELECT name FROM products WHERE id = ?', [1]).get();
  assert.strictEqual(byId.name, 'apple');

  await Aluka.SQL`DELETE FROM products`.run();
  const empty = await Aluka.SQL`SELECT COUNT(*) AS n FROM products`.get();
  assert.strictEqual(empty.n, 0);

  console.log('PASS bun-p2 sqlite+tagged');
}

main().catch((e) => {
  console.error('FAIL', e && e.message ? e.message : e);
  process.exit(1);
});
