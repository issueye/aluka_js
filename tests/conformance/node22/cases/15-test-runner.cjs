//@test
// node:test 测试器增强（T1）：before/after、skip/todo、t.plan、子测试、
// 注册顺序。差分对比：`node --test 15-test-runner.cjs` vs `aluka test`。
// 归一化输出：HOOKS> 日志行 + tests/pass/fail/cancelled/skipped/todo 统计。
const { describe, it, test, before, after, beforeEach, afterEach } = require("node:test");
const assert = require("node:assert");

// 钩子顺序探针（全局日志，运行后打印）。
const log = [];
function trace(name) { log.push(name); }

describe("hooks", () => {
  before(() => trace("before-hooks"));
  after(() => trace("after-hooks"));
  beforeEach(() => trace("beforeEach-hooks"));
  afterEach(() => trace("afterEach-hooks"));

  it("case-a", () => {
    trace("case-a");
  });

  describe("inner", () => {
    before(() => trace("before-inner"));
    after(() => trace("after-inner"));
    it("case-b", () => { trace("case-b"); });
  });

  it("case-c", () => {
    trace("case-c");
  });
});

// skip / todo / only 标记。
test("skipped via options", { skip: true }, () => { trace("SHOULD-NOT-RUN-skip"); });
test.skip("skipped via it.skip", () => { trace("SHOULD-NOT-RUN-skip2"); });
test.todo("todo via it.todo", () => { trace("todo-ran"); });
test("todo via options", { todo: true }, () => { trace("todo-ran2"); });
test.skip("skip without fn");
test("todo without fn", { todo: true });

// t.plan：断言计数校验。
test("plan exact", (t) => {
  t.plan(2);
  t.assert.equal(1, 1);
  t.assert.ok(true);
});
test("plan under", (t) => {
  t.plan(2);
  t.assert.equal(1, 1);
});

// 子测试（async 父 + await：Node 22 语义，子测试经微任务调度执行）。
test("parent with subtests", async (t) => {
  await t.test("sub-1", () => { trace("sub-1"); });
  await t.test("sub-2", (st) => {
    st.assert.equal(2, 2);
  });
});
// 失败子测试 → 父失败（subtestsFailed）。
test("parent with failing subtest", async (t) => {
  await t.test("sub-fail", () => {
    assert.strictEqual(1, 2);
  });
});

// 注册顺序：suite 与 test 混合注册（Node 按注册顺序执行）。
describe("mixed", () => {
  it("first-test", () => trace("mixed-first"));
  describe("middle-suite", () => {
    it("suite-test", () => trace("mixed-suite"));
  });
  it("last-test", () => trace("mixed-last"));
});

// 顶层 test 与 describe 混合。
test("top-level-test", () => trace("top-level"));

// 断言失败（todo 不失败）。
test("failing", () => {
  assert.strictEqual(1, 2);
});

// 运行后输出钩子日志。
test("hook-order-log", () => {
  console.log("HOOKS>" + log.join(","));
});
