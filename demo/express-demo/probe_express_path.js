try {
  var express = require('./node_modules/express');
  console.log('OK express:', typeof express);
} catch (e) {
  console.log('ERR:', e.message);
}