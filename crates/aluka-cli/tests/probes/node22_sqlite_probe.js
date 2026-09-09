// M5.3 sqlite 差分探针：Node 22.23.1 vs aluka 逐字对拍（:memory: 全程离线）
const { DatabaseSync } = require('node:sqlite');
const { Buffer } = require('node:buffer');

// ===== 1. 基础 CRUD 与语句结果形态 =====
const db = new DatabaseSync(':memory:');
console.log('open:', db.isOpen, db.isOpen === true);
db.exec('CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age REAL, nick TEXT)');
const ins = db.prepare('INSERT INTO users (name, age, nick) VALUES (?, ?, ?)');
console.log('surface:', typeof db.exec, typeof db.prepare, typeof db.close, typeof ins.run, typeof ins.get, typeof ins.all, typeof ins.iterate, typeof ins.columns, typeof ins.setReadBigInts, typeof ins.sourceSQL);
console.log('exec-ret:', db.exec('SELECT 1') === undefined);
let r = ins.run('Alice', 30.5, null);
console.log('r1:', r.changes, r.lastInsertRowid);
r = ins.run('Bob', 25, 'bobby');
console.log('r2:', r.changes, r.lastInsertRowid);
const all = db.prepare('SELECT id, name, age, nick FROM users ORDER BY id').all();
console.log('all:', all.length, all[0].id, all[0].name, all[0].age, all[0].nick, '|', all[1].name, all[1].nick);
console.log('none:', db.prepare('SELECT * FROM users WHERE id = 999').get() === undefined);
const agg = db.prepare('SELECT COUNT(*) AS c, SUM(age) AS s FROM users').get();
console.log('agg:', agg.c, agg.s);
const upd = db.prepare('UPDATE users SET age = ? WHERE name = ?').run(31, 'Alice');
console.log('upd:', upd.changes, upd.lastInsertRowid);
console.log('named:', db.prepare('SELECT id, name FROM users WHERE name = :name').get({ name: 'Bob' }).id, db.prepare('SELECT id FROM users WHERE name = $name').get({ name: 'Bob' }).id, db.prepare('SELECT id FROM users WHERE name = @name').get({ name: 'Bob' }).id);

// ===== 2. iterate 迭代器 =====
const it = db.prepare('SELECT name FROM users ORDER BY id').iterate();
const i1 = it.next();
const i2 = it.next();
console.log('iter:', i1.done, i1.value.name, i2.value.name);
const i3 = it.next();
console.log('iter-end:', i3.done, i3.value);

// ===== 3. columns / sourceSQL =====
const cols = db.prepare('SELECT id, name, age, nick FROM users').columns();
console.log('cols:', JSON.stringify(cols));
const src = 'SELECT name FROM users WHERE age >= ?';
console.log('src:', db.prepare(src).sourceSQL);

// ===== 4. 事务：exec 手动事务 / isTransaction（Node 22 LTS 无 db.transaction）=====
console.log('txn-before:', db.isTransaction);
db.exec('BEGIN');
console.log('manual-begin:', db.isTransaction);
ins.run('Trudy', 50, null);
db.exec('COMMIT');
console.log('manual-commit:', db.isTransaction);
console.log('txn-count:', db.prepare('SELECT COUNT(*) AS c FROM users').get().c);
db.exec('BEGIN');
ins.run('Mallory', 60, null);
db.exec('ROLLBACK');
console.log('rollback-count:', db.prepare('SELECT COUNT(*) AS c FROM users').get().c, db.isTransaction);

// ===== 5. 类型映射：bigint / blob / Boolean 拒绝 =====
db.exec('CREATE TABLE blobs (id INTEGER PRIMARY KEY, data BLOB, n INTEGER)');
db.prepare('INSERT INTO blobs (data, n) VALUES (?, ?)').run(Buffer.from([1, 2, 3, 250]), 10n);
db.prepare('INSERT INTO blobs (data, n) VALUES (?, ?)').run(new Uint8Array([7, 8]), 0n);
const b1 = db.prepare('SELECT data FROM blobs ORDER BY id LIMIT 1').get().data;
console.log('blob:', typeof b1, Buffer.isBuffer(b1), b1.length, [b1[0], b1[1], b1[2], b1[3]].join(','));
console.log('bigint-store:', db.prepare('SELECT n FROM blobs ORDER BY id').all().map(r => typeof r.n + ':' + r.n).join(' | '));
const bs = db.prepare('SELECT n FROM blobs ORDER BY id');
bs.setReadBigInts(true);
console.log('bigint-read:', bs.all().map(r => typeof r.n + ':' + r.n).join(' | '));
try { db.prepare('SELECT ? AS v').get(true); } catch (e) { console.log('bool:', e.name, '|', e.code, '|', e.message); }
try { db.prepare('SELECT ? AS v').get([1, 2]); } catch (e) { console.log('array:', e.name, '|', e.message); }
try { db.prepare('SELECT ? AS v').get({ a: 1 }); } catch (e) { console.log('objpos:', e.name, '|', e.message); }

// ===== 6. 缺参 / 多余参 / 未知命名参数 =====
const sp = db.prepare('SELECT ? AS v, ? AS w');
try { const gv = sp.get(1); console.log('missing:', gv.v, gv.w); } catch (e) { console.log('missing-err:', e.message); }
try { sp.get(1, 2, 3); } catch (e) { console.log('extra:', e.message); }
try { db.prepare('SELECT :x AS v').get({ y: 1 }); } catch (e) { console.log('unknown:', e.message); }
let okFlag = true;
try { db.prepare('SELECT ? AS v').get(undefined); } catch (e) { okFlag = false; console.log('undef:', e.message); }
if (okFlag) console.log('undef-ok');
okFlag = true;
try { db.prepare('SELECT 1').get({}); } catch (e) { okFlag = false; console.log('plainobj-extra:', e.message); }
if (okFlag) console.log('plainobj-ok');

// ===== 7. SQL 错误 / 约束（错误对象 attrs 对齐 Node：code/errcode/errstr）=====
try { db.exec('NOT REAL SQL'); } catch (e) { console.log('syn:', e.code, e.errcode, e.errstr, '|', e.message); }
try { db.prepare('SELECT * FROM').all(); } catch (e) { console.log('incomplete:', e.code, e.errcode, e.errstr, '|', e.message); }
try { db.prepare('SELECT * FROM no_such_table').all(); } catch (e) { console.log('nosuch:', e.code, e.errcode, e.errstr, '|', e.message); }
try { db.prepare("INSERT INTO users (id, name) VALUES (1, 'dup')").run(); } catch (e) { console.log('dup:', e.code, e.errcode, e.errstr, '|', e.message); }
try { db.prepare('INSERT INTO users (id, name) VALUES (99, NULL)').run(); } catch (e) { console.log('nn:', e.name, '|', e.message); }
try { db.prepare('INSERT INTO users (id, name) VALUES (98, ?)').run(); } catch (e) { console.log('noparam:', e.message); }
try { db.prepare('INSERT INTO users (id, name) VALUES (97, ?)').run(10n); console.log('bigint-bind: ok'); } catch (e) { console.log('bigint-bind-err:', e.message); }
const bv = db.prepare("SELECT name FROM users WHERE id = 97").get();
console.log('bigint-bound:', typeof bv.name, bv.name);

// ===== 8. 打开 / close 语义 =====
try { new DatabaseSync('Z:/no/such/dir/x.db'); } catch (e) { console.log('noopen:', e.name, '|', e.message); }
const d2 = new DatabaseSync(':memory:');
d2.close();
console.log('closed:', d2.isOpen, d2.isOpen === false);
try { d2.prepare('SELECT 1'); } catch (e) { console.log('closed-prepare:', e.message); }
try { d2.close(); } catch (e) { console.log('closed-close:', e.name, '|', e.message); }
try { new DatabaseSync(); } catch (e) { console.log('nopath:', e.name, '|', e.message); }
try { new DatabaseSync(123); } catch (e) { console.log('badpath:', e.name, '|', e.message); }
