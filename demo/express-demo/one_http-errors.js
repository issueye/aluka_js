try { var m = require('http-errors'); console.log('http-errors OK: ' + typeof m); } catch (e) { console.log('http-errors ERR: ' + (e.message || e)); }
