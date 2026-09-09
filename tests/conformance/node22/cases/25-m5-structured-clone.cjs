// M5.1 结构化克隆差分用例：worker 跨线程传值（类型面/循环引用/transfer/
// detach/DataCloneError）——输出与 Node 22.23.1 逐字对拍。
// M5.1 结构化克隆 Node 22 oracle（worker 跨线程）
const { Worker, isMainThread, parentPort, MessageChannel, markAsUntransferable } = require('node:worker_threads');

function callTry(fn) { try { return String(fn()); } catch (e) { return 'X:' + e.name; } }
if (!isMainThread) {
  parentPort.on('message', (msg) => {
    if (msg && msg.kind === 'echo') {
      const m = msg.data;
      const out = {
        kind: 'result',
        undef: m.undef === undefined,
        nil: m.nil,
        str: m.str,
        num: m.num,
        big: typeof m.big + ':' + m.big,
        arr: String(m.arr[0]) + ',' + String(m.arr[1]) + ',' + String(m.arr[2]),
        cyc: m.cyc.self === m.cyc,
        share: m.cyc === m.cycAgain,
        date: (typeof m.date === 'object' && m.date !== null) ? 'date-obj' : 'no',
        rx: (typeof m.rx === 'object' && m.rx !== null) ? m.rx.source + '/' + m.rx.flags : 'no',
        map: (m.map !== null && typeof m.map === 'object') ? callTry(function () { return m.map.get('k1') + '=' + m.map.get('k2'); }) : 'no',
        set: (m.set !== null && typeof m.set === 'object') ? callTry(function () { return m.set.has('3') + ':' + m.set.has('zz'); }) : 'no',
        ta: (typeof m.ta === 'object' && m.ta !== null) ? m.ta.length + ':' + m.ta[0] + ',' + m.ta[1] + ',' + m.ta[2] : 'no',
        nested: m.nested.deep.value,
        protoLost: Object.getPrototypeOf(m.plain) === Object.prototype,
      };
      parentPort.postMessage(out);
    } else if (msg && msg.kind === 'transfer') {
      const out = {
        kind: 'result',
        abLen: msg.ab.byteLength,
        ab0: msg.ab[0],
        taLen: msg.ta.byteLength,
        ta0: msg.ta[0],
        dvLen: msg.dv.byteLength,
        cycTransfer: msg.cyc.self === msg.cyc,
      };
      parentPort.postMessage(out);
    }
  });
} else {
  const run = async () => {
    // 1. 值载荷
    const cyc = { name: 'cyc' };
    cyc.self = cyc;
    const m = {
      undef: undefined,
      nil: null,
      str: 'hello',
      num: 42,
      big: 9007199254740993n,
      arr: [1, 'two', null],
      cyc,
      cycAgain: cyc,
      date: new Date(1700000000123),
      rx: /ab+c/gi,
      map: new Map([['k1', 'v1'], ['k2', 2]]),
      set: new Set(['3', 'x']),
      ta: new Uint8Array([10, 20, 30]),
      nested: { deep: { value: 'deep-val' } },
      plain: { a: 1 },
    };
    const w = new Worker(__filename);
    w.on('message', (r) => {
      console.log('val:', JSON.stringify(r));
      // 2. transfer
      const ab = new ArrayBuffer(8);
      new Uint8Array(ab).set([1, 2, 3, 4, 5, 6, 7, 8]);
      const ta = new Uint8Array(ab, 2, 3);
      const dv = new DataView(ab, 1, 4);
      const cyc2 = {}; cyc2.self = cyc2;
      w.postMessage({ kind: 'transfer', ab, ta, dv, cyc: cyc2 }, [ab]);
      w.once('message', (r2) => {
        console.log('transfer:', JSON.stringify(r2));
        console.log('after-ab:', ab.byteLength, ab[0]);
        console.log('after-ta:', ta.byteLength, ta[0]);
        try { console.log('after-dv:', dv.byteLength); } catch (e) { console.log('after-dv-throw:', e.name); }
        // 3. 错误形态
        try { w.postMessage({ fn: () => 1 }); } catch (e) { console.log('fn-err:', e.name); }
        try { w.postMessage(Symbol('s')); } catch (e) { console.log('sym-err:', e.name); }
        try { w.postMessage({ p: Promise.resolve(1) }); } catch (e) { console.log('prom-err:', e.name); }
        // 4. markAsUntransferable
        const nb = new ArrayBuffer(4);
        markAsUntransferable(nb);
        try { w.postMessage(nb, [nb]); } catch (e) { console.log('mark-err:', e.name, '|', e.message); }
        // 5. transfer 二次使用(已 detach 再 transfer)
        try { w.postMessage(ab, [ab]); } catch (e) { console.log('twice-err:', e.name, '|', e.message); }
        // 6. 对象含不可枚举?
        const obj = { a: 1 };
        Object.defineProperty(obj, 'hidden', { value: 9, enumerable: false });
        w.postMessage({ kind: 'echo', data: { plain2: obj } });
        w.once('message', (r3) => {
          console.log('nomenum:', JSON.stringify(r3));
          w.terminate();
        });
      });
    });
    w.on('error', (e) => console.log('worker-error:', e.message));
    w.postMessage({ kind: 'echo', data: m });
  };
  run();
}
