async function f(){ return 1; }
f().then(v=>console.log('then:',v));
Promise.resolve(2).then(v=>console.log('resolve:',v));
(async()=>{ console.log('await:', await Promise.resolve(3)); })();
Promise.all([1,Promise.resolve(2)]).then(v=>console.log('all:',JSON.stringify(v)));
Promise.race([Promise.resolve('fast')]).then(v=>console.log('race:',v));
new Promise(r=>r(4)).then(v=>console.log('exec:',v));
Promise.resolve(1).finally(()=>console.log('finally:ok')).then(v=>console.log('after-finally:',v));
Promise.reject(new Error('rej')).catch(e=>console.log('caught:',e.message));
console.log('sync-end');
