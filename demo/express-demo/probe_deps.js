var tests = [
  'safe-buffer', 'content-disposition', 'http-errors', 'depd', 'encodeurl',
  'escape-html', 'on-finished', 'statuses', 'utils-merge', 'cookie-signature',
  'cookie', 'send', 'vary'
];
var results = [];
for (var i = 0; i < tests.length; i++) {
  try {
    var m = require('./node_modules/' + tests[i]);
    results.push(tests[i] + ':OK');
  } catch (e) {
    results.push(tests[i] + ':ERR=' + (e.message || e));
    break;
  }
}
console.log(results.join(' | '));