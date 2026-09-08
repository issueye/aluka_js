try {
  var m = require('./node_modules/express/lib/middleware/init');
  console.log('OK:', typeof m);
} catch (e) {
  console.log('ERR:', e.message);
}