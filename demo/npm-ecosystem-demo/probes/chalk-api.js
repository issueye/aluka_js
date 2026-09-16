// chalk v4 典型 API 对拍（确定性输出——强制 level=3 启用 ANSI）
const chalk = require('chalk');

function t(label, fn) {
  try {
    console.log(`${label}=${JSON.stringify(fn())}`);
  } catch (err) {
    console.log(`${label}=ERR ${err && err.name}: ${err && err.message}`);
  }
}

chalk.level = 3;
t('level', () => chalk.level);
t('red', () => chalk.red('hello'));
t('bold.blue', () => chalk.bold.blue('x'));
t('green.bold.underline', () => chalk.green.bold.underline('y'));
t('rgb', () => chalk.rgb(10, 20, 30).underline('z'));
t('bgRed.white', () => chalk.bgRed.white('w'));
t('hex', () => chalk.hex('#ff0000')('h'));
t('dim.reset', () => chalk.dim('a') + chalk.reset('b'));
t('template-literal', () => chalk.yellow(`n=${42}`));
t('supportsColor', () => typeof chalk.supportsColor);
console.log('CHALK_API_DONE');
