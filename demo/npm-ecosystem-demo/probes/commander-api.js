// commander v12 典型 API 对拍（确定性输出——显式喂参数不走 process.argv）
const { Command, Option } = require('commander');

function t(label, fn) {
  try {
    console.log(`${label}=${JSON.stringify(fn())}`);
  } catch (err) {
    console.log(`${label}=ERR ${err && err.name}: ${err && err.message}`);
  }
}

t('Command-type', () => typeof Command);
t('Option-type', () => typeof Option);

// 子命令 + 选项 + action 回调
t('subcommand', () => {
  const out = [];
  const program = new Command();
  program.name('demo');
  program
    .command('greet <name>')
    .option('-u, --upper', 'uppercase')
    .action((name, opts) => {
      out.push([name, opts.upper, opts.upper ? name.toUpperCase() : name]);
    });
  program.parse(['greet', 'world', '--upper'], { from: 'user' });
  return out;
});

// 带短选项与值的选项
t('option-value', () => {
  let got = null;
  const program = new Command();
  program
    .command('serve')
    .option('-p, --port <n>', 'port', '8080')
    .action((opts) => {
      got = opts.port;
    });
  program.parse(['serve', '-p', '3000'], { from: 'user' });
  return got;
});

// version/help 信息面
t('name-version', () => {
  const program = new Command();
  program.name('myapp').version('2.5.0');
  return [program.name(), program.version()];
});

// args 收集（variadic）
t('variadic', () => {
  let got = null;
  const program = new Command();
  program
    .command('copy <src> [dest...]')
    .action((src, dest) => {
      got = [src, dest];
    });
  program.parse(['copy', 'a.txt', 'b.txt', 'c.txt'], { from: 'user' });
  return got;
});

// Command 实例方法面抽查
t('methods', () => {
  const program = new Command();
  return [typeof program.command, typeof program.parse, typeof program.opts, typeof program.help];
});
console.log('COMMANDER_API_DONE');
