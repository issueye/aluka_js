//@test
// M7-1 测试模式差分：TestContext 属性、t.assert 全方法、套件 hooks、
// describe.todo/suite 别名、子测试。归一化对比 `node --test` vs `aluka test`。
const { describe, it, test, before, after, beforeEach, afterEach, suite } = require("node:test");
const assert = require("node:assert");
const log = [];
function trace(name) { log.push(name); }

test("ctx props", async (t) => {
  trace("name:" + t.name);
  trace("fullName:" + t.fullName);
  trace("filePath:" + typeof t.filePath);
  trace("signal:" + typeof t.signal);
  trace("assert:" + typeof t.assert);
  trace("tMock:" + typeof t.mock);
  trace("before:" + typeof t.before);
  trace("after:" + typeof t.after);
  trace("beforeEach:" + typeof t.beforeEach);
  trace("afterEach:" + typeof t.afterEach);
  t.assert.equal(1, 1);
  t.assert.notEqual(1, 2);
  t.assert.strictEqual(1, 1);
  t.assert.notStrictEqual(1, "1");
  t.assert.deepEqual({ a: 1 }, { a: 1 });
  t.assert.deepStrictEqual({ a: 1 }, { a: 1 });
  t.assert.notDeepEqual({ a: 1 }, { a: 2 });
  t.assert.notDeepStrictEqual({ a: 1 }, { a: "1" });
  t.assert.ok(true);
  t.assert.throws(() => { throw new Error("x"); });
  t.assert.match("hello", /ell/);
  t.assert.doesNotMatch("hello", /xyz/);
  t.assert.ifError(null);
  await t.assert.rejects(async () => { throw new Error("r"); });
  await t.assert.doesNotReject(async () => 1);
  // register() 自定义断言（v22.14）。
  const { register } = require("node:test");
  register("customEven", (value) => {
    if (value % 2 !== 0) throw new Error("not even: " + value);
  });
  t.assert.customEven(4);
});

describe("s2", () => {
  before(() => trace("before-s2"));
  after(() => trace("after-s2"));
  beforeEach(() => trace("beforeEach-s2"));
  afterEach(() => trace("afterEach-s2"));
  it("c1", () => trace("c1"));
  it.skip("c2", () => trace("NO-c2"));
  it.todo("c3", () => trace("c3"));
  it("c4", (t) => { t.diagnostic("diag-msg"); trace("c4"); });
});

suite("s3", () => {
  it("c5", () => trace("c5"));
});

describe.todo("todo-suite", () => {
  it("tc1", () => trace("tc1"));
});

// 子测试 + 父级 t.beforeEach/t.afterEach 作用于子测试。
test("subtree", async (t) => {
  t.beforeEach(() => trace("sub-beforeEach"));
  t.afterEach(() => trace("sub-afterEach"));
  await t.test("sub1", () => trace("sub1"));
  await t.test("sub2", (st) => { st.assert.equal(2, 2); trace("sub2"); });
});

test("hook-order", () => {
  console.log("HOOKS>" + log.join(","));
});
