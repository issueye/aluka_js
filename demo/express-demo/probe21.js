try { require('http-errors'); console.log('OK http-errors'); } catch (e) { console.log('CAUGHT: ' + (e.message || e)); }
