// 表面验证：exports / spec 实例 / compose 返回
const reporters = require('node:test/reporters');
const { Transform } = require('node:stream');
console.log('exports:', typeof reporters.dot, typeof reporters.junit, typeof reporters.spec, typeof reporters.tap, typeof reporters.lcov);
console.log('names:', reporters.dot.name, reporters.junit.name, reporters.spec.name, reporters.tap.name);
const spec = new reporters.spec();
console.log('spec-ctor:', spec.constructor.name, '| isTransform:', spec instanceof Transform, '| wObjMode:', spec.writableObjectMode);
console.log('lcov-obj:', typeof reporters.lcov, typeof reporters.lcov.write);
const { run } = require('node:test');
const c = run().compose(reporters.spec);
console.log('composed-ctor:', c.constructor.name, '| pipe:', typeof c.pipe);
