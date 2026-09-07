try { var m = require('send'); console.log('send OK: ' + typeof m); } catch (e) { console.log('send ERR: ' + (e.message || e)); }
