try { require('nonexistent-pkg'); console.log("not caught"); } catch (e) { console.log("caught: " + (e.code || e.message || e)); }
