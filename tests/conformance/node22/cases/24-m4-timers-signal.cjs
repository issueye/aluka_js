// M4 差分用例：timers AbortSignal 联动（setTimeout/setImmediate 取消语义）。
const ac = new AbortController();
let fired = false;
setTimeout(() => { fired = true; }, 30, { signal: ac.signal });
ac.abort();
setTimeout(() => {
  console.log('timer-aborted:', fired);
  const ac2 = new AbortController();
  let imm = false;
  setImmediate(() => { imm = true; }, { signal: ac2.signal });
  ac2.abort();
  setTimeout(() => {
    console.log('immediate-aborted:', imm);
    // 未 abort 的 signal 不影响正常触发
    const ac3 = new AbortController();
    let normal = false;
    setTimeout(() => { normal = true; }, 5, { signal: ac3.signal });
    setTimeout(() => {
      console.log('timer-normal:', normal);
    }, 20);
  }, 20);
}, 60);
