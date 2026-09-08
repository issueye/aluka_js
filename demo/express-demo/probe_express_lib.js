try {
  var lib = require('./node_modules/express/lib/express');
  console.log('OK lib:', typeof lib);
} catch (e) {
  console.log('ERR:', e.message || e);
}