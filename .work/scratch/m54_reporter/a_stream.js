const { run, test } = require('node:test');
test('a', () => {});
const s = run();
console.log('on:', typeof s.on, '| emit:', typeof s.emit, '| pipe:', typeof s.pipe, '| compose:', typeof s.compose);
s.on('test:pass', (d) => console.log('EVT', JSON.stringify(d)));
