'use strict';
// rmSync 缺失路径语义对照
const fs = require('node:fs');
function t(label, fn) {
  try {
    fn();
    console.log(label, '=> OK');
  } catch (e) {
    console.log(label, '=> THROW', e && e.code);
  }
}
t('rmSync(missing)', () => fs.rmSync('nope1'));
t('rmSync(missing,{recursive:true})', () => fs.rmSync('nope2', { recursive: true }));
t('rmSync(missing,{recursive:true,force:true})', () => fs.rmSync('nope3', { recursive: true, force: true }));
t('rmSync(missing,{force:true})', () => fs.rmSync('nope4', { force: true }));
// 已存在目录：recursive 删除（无 force）
fs.mkdirSync('d1');
fs.writeFileSync('d1/a.txt', 'x');
t('rmSync(existing-dir,{recursive:true})', () => fs.rmSync('d1', { recursive: true }));
console.log('gone:', !fs.existsSync('d1'));
