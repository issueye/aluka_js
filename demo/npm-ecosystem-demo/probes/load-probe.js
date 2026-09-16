'use strict';
// 逐包加载探针（**静态 require 字面量**——构建器按字面量扫描依赖闭包，
// 变量形式的动态 require 不会入镜像；见 README「已知限制」）
function probe(label, fn) {
  try {
    console.log(`${label}: ok ${fn()}`);
  } catch (err) {
    const desc =
      err && typeof err === 'object'
        ? `${err.name}: ${err.message}`
        : `${typeof err}: ${String(err)}`;
    console.log(`${label}: LOAD_FAIL ${desc}`);
  }
}

probe('lodash', () => {
  const _ = require('lodash');
  return `typeof=${typeof _} version=${_.VERSION}`;
});
probe('zod', () => {
  const z = require('zod');
  return `typeof=${typeof z} keys=${Object.keys(z).length}`;
});
probe('chalk', () => {
  const chalk = require('chalk');
  return `typeof=${typeof chalk} level=${chalk.level}`;
});
probe('commander', () => {
  const { Command } = require('commander');
  return `Command=${typeof Command}`;
});
console.log('LOAD_PROBE_DONE');
