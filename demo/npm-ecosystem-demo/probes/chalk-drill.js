// chalk 缺陷钻取：ansi-styles 底层面
const styles = require('ansi-styles');
const chalk = require('chalk');

function t(label, fn) {
  try {
    console.log(`${label}=${JSON.stringify(fn())}`);
  } catch (err) {
    console.log(`${label}=ERR ${err && err.name}: ${err && err.message}`);
  }
}

t('styles.red', () => styles.red);
t('styles.red.open', () => styles.red.open);
t('styles.blue.open', () => styles.blue.open);
t('styles.color.codes', () => styles.color ? JSON.stringify(styles.color.codes) : 'none');
t('rgbToAnsi256', () => typeof styles.rgbToAnsi256 ? styles.rgbToAnsi256(10, 20, 30) : 'none');
t('rgbToAnsi16', () => typeof styles.rgbToAnsi16 ? styles.rgbToAnsi16(255, 0, 0) : 'none');

// chalk 侧：直接读命名样式对象
t('chalk.red.isGetter', () => {
  const d = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(chalk), 'red');
  return d ? [typeof d.get, typeof d.value] : 'no-desc';
});
t('chalk.styles.red', () => chalk.styles ? chalk.styles.red.open : 'none');
// 链式 builder 的函数面
t('chalk.red.typeof', () => typeof chalk.red);
t('chalk.red.open', () => chalk.red.open);
// rgb 分量流转
t('rgb-input-type', () => {
  let captured = null;
  const orig = styles.rgbToAnsi256;
  styles.rgbToAnsi256 = (r, g, b) => {
    captured = [typeof r, r, g, b];
    return orig ? orig(r, g, b) : 0;
  };
  try {
    chalk.rgb(10, 20, 30)('x');
  } catch (e) {
    captured = ['throw', e.message];
  }
  styles.rgbToAnsi256 = orig;
  return captured;
});
console.log('CHALK_DRILL_DONE');
