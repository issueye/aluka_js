const { Worker } = require('node:worker_threads');
const w = new Worker('./no_such_worker_file.js');
w.on('error', (e) => { console.log('werr fired:', typeof e, '|', e instanceof Error, '|', e.code, '|', e.message.split('\n')[0]); });
w.on('exit', (code) => { console.log('wexit:', code); });
