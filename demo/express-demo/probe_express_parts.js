try {
  var m = require('./node_modules/express/lib/request');
  console.log('OK request:', typeof m);
} catch (e) {
  console.log('ERR request:', e.message);
}
try {
  var m2 = require('./node_modules/express/lib/response');
  console.log('OK response:', typeof m2);
} catch (e2) {
  console.log('ERR response:', e2.message);
}
try {
  var m3 = require('./node_modules/express/lib/utils');
  console.log('OK utils:', typeof m3);
} catch (e3) {
  console.log('ERR utils:', e3.message);
}
try {
  var m4 = require('./node_modules/express/lib/view');
  console.log('OK view:', typeof m4);
} catch (e4) {
  console.log('ERR view:', e4.message);
}