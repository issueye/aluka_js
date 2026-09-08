// M4 差分用例：Web Streams 互通（Readable/Writable fromWeb·toWeb）
// 与 AbortController → fetch 中断联动。
const { Readable, Writable } = require('node:stream');
const { ReadableStream, WritableStream } = require('node:stream/web');

async function main() {
  // fromWeb：start 阶段已 enqueue + close 的 web 流 → Node 可读流（补交语义）
  const web = new ReadableStream({
    start(c) {
      c.enqueue('a');
      c.enqueue('b');
      c.close();
    },
  });
  const node = Readable.fromWeb(web);
  const chunks = [];
  for await (const ch of node) chunks.push(String(ch));
  console.log('fromWeb:', chunks.join(','));

  // fromWeb live：挂桥后 enqueue 实时转发
  let ctl;
  const webLive = new ReadableStream({
    start(c) {
      ctl = c;
    },
  });
  const nodeLive = Readable.fromWeb(webLive);
  ctl.enqueue('x');
  ctl.close();
  const liveChunks = [];
  for await (const ch of nodeLive) liveChunks.push(String(ch));
  console.log('fromWeb-live:', liveChunks.join(','));

  // toWeb：Node 可读流（Readable.from 预缓冲）→ web 流 reader
  const node2 = Readable.from(['p', 'q']);
  const web2 = Readable.toWeb(node2);
  const reader = web2.getReader();
  const out = [];
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    out.push(String(value));
  }
  console.log('toWeb:', out.join(','));

  // Writable.fromWeb：Node 写入转发 underlyingSink.write（chunk.length 对齐）
  const received = [];
  const webW = new WritableStream({
    write(chunk) {
      received.push(chunk.length);
    },
  });
  const nodeW = Writable.fromWeb(webW);
  nodeW.write('ab');
  nodeW.write('c');
  nodeW.end();
  await new Promise((r) => setImmediate(r));
  console.log('writable-fromWeb:', received.join(','));

  // Writable.toWeb：web writer 写入转发 Node 可写流
  const sink = [];
  const nodeW2 = new Writable({
    write(chunk, enc, cb) {
      sink.push(chunk.length);
      cb();
    },
  });
  const webW2 = Writable.toWeb(nodeW2);
  const writer = webW2.getWriter();
  await writer.write('m');
  await writer.write('no');
  await writer.close();
  console.log('writable-toWeb:', sink.join(','));

  // AbortController：事件监听 + 幂等 + reason 缺省
  const ac = new AbortController();
  let fired = 0;
  ac.signal.addEventListener('abort', () => {
    fired += 1;
  });
  ac.abort();
  ac.abort();
  console.log('signal:', ac.signal.aborted, fired, ac.signal.reason.name);

  // fetch 中断联动：预中止 signal → 立即 AbortError（不发起连接）
  const ac2 = new AbortController();
  ac2.abort();
  try {
    await fetch('http://127.0.0.1:9/nope', { signal: ac2.signal });
    console.log('fetch-abort: no-reject');
  } catch (e) {
    console.log('fetch-abort:', e.name);
  }
}

main();
