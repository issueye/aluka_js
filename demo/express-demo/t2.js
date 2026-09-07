console.log("process:", typeof process, "lc:", typeof process.listenerCount);
try { require('http-errors'); console.log("loaded ok"); } catch (e) { console.log("caught: " + (e.message || e)); }
