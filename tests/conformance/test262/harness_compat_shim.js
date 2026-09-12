// test262 官方 assert 覆盖 runner 最小 harness 后的兼容垫片：
// 补回手写用例依赖的 isTrue/isFalse 面（官方 assert.js 未提供）
if (typeof assert.isTrue !== "function") {
  assert.isTrue = function (v, msg) {
    if (v !== true) throw new Error("assert.isTrue" + (msg ? ": " + msg : ""));
  };
}
if (typeof assert.isFalse !== "function") {
  assert.isFalse = function (v, msg) {
    if (v !== false) throw new Error("assert.isFalse" + (msg ? ": " + msg : ""));
  };
}
