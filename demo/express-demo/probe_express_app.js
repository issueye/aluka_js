try {
  var m = require('./node_modules/express/lib/application');
  console.log('OK:', typeof m);
} catch (e) {
  console.log('ERR:', e.message);
}