'use strict';
// lodash 真实 API 对拍（确定性输出）
const _ = require('lodash');

function t(label, fn) {
  try {
    console.log(`${label}=${JSON.stringify(fn())}`);
  } catch (err) {
    console.log(`${label}=ERR ${err && err.name}: ${err && err.message}`);
  }
}

t('chunk', () => _.chunk([1, 2, 3, 4, 5], 2));
t('flatten', () => _.flatten([1, [2, [3, [4]]]]));
t('flattenDeep', () => _.flattenDeep([1, [2, [3, [4]]]]));
t('uniq', () => _.uniq([2, 1, 2, 3, 1]));
t('uniqBy', () => _.uniqBy([{ x: 1 }, { x: 1 }, { x: 2 }], 'x').length);
t('groupBy', () => _.groupBy([6.1, 4.2, 6.3], Math.floor));
t('orderBy', () => _.orderBy([{ n: 2 }, { n: 1 }], ['n'], ['desc']));
t('merge', () => _.merge({ a: 1, b: { c: 2 } }, { b: { d: 3 } }));
t('cloneDeep', () => { const o = { a: [1, { b: 2 }] }; const c = _.cloneDeep(o); c.a[1].b = 99; return [o.a[1].b, c.a[1].b]; });
t('get', () => _.get({ a: { b: [{ c: 7 }] } }, 'a.b[0].c'));
t('set', () => { const o = {}; _.set(o, 'a.b[0].c', 5); return o.a.b[0].c; });
t('has', () => _.has({ a: { b: 2 } }, 'a.b'));
t('keys', () => _.keys({ a: 1, b: 2 }));
t('values', () => _.values({ a: 1, b: 2 }));
t('map', () => _.map([{ n: 1 }, { n: 2 }], 'n'));
t('filter', () => _.filter([{ a: 1 }, { a: 2 }], { a: 2 }).length);
t('find', () => _.find([{ a: 1 }, { a: 2 }], { a: 2 }).a);
t('sum', () => _.sum([1, 2, 3]));
t('times', () => _.times(3, String));
t('range', () => _.range(1, 4));
t('debounce', () => typeof _.debounce(() => {}, 10));
t('throttle', () => typeof _.throttle(() => {}, 10));
t('template', () => _.template('hi <%= name %>!')({ name: 'aluka' }));
t('isEqual', () => _.isEqual({ a: [1, 2] }, { a: [1, 2] }));
t('isEmpty', () => _.isEmpty({}));
t('camelCase', () => _.camelCase('foo bar-baz'));
t('kebabCase', () => _.kebabCase('fooBar'));
t('startCase', () => _.startCase('fooBar'));
t('pad', () => _.pad('abc', 7, '*'));
t('repeat', () => _.repeat('ab', 3));
t('sample-size', () => _.sampleSize([1, 2, 3], 2).length);
t('chain', () => _.chain([1, 2, 3]).map((n) => n * 2).filter((n) => n > 2).value());
t('matches', () => _.matches({ a: 1 })({ a: 1, b: 2 }));
t('once', () => { let n = 0; const f = _.once(() => ++n); f(); f(); return n; });
t('VERSION', () => typeof _.VERSION);
console.log('LODASH_API_DONE');
