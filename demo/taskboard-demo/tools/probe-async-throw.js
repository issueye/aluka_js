'use strict';
async function syncThrow() { throw new Error('sync-throw-in-async'); }
async function afterAwait() { await Promise.resolve(1); throw new Error('throw-after-await'); }
async function inner() { throw new Error('inner-sync'); }
async function outer() { await inner(); }
syncThrow().catch(function (e) { console.log('CAUGHT-1:', e.message); });
afterAwait().catch(function (e) { console.log('CAUGHT-2:', e.message); });
outer().catch(function (e) { console.log('CAUGHT-3:', e.message); });
console.log('registered all');
setTimeout(function () { console.log('h29 DONE'); }, 50);
