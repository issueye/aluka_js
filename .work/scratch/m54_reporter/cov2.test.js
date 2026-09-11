const { run, test } = require('node:test');
test('passes', () => { console.log('exec'); });
test('skipped-one', { skip: true }, () => {});
run().compose(require('node:test/reporters').lcov).pipe(process.stdout);
