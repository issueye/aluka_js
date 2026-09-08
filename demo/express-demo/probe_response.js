try {
  var m = require('./node_modules/express/lib/response');
  console.log('OK response:', typeof m);
} catch (e) {
  console.log('ERR response:', e.message || e);
}