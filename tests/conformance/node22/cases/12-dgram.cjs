const dgram = require('node:dgram');
console.log('d1:', typeof dgram.createSocket);
const s = dgram.createSocket('udp4');
console.log('d2:', typeof s.bind, typeof s.send, typeof s.close, typeof s.on, typeof s.addMembership);
console.log('d3:', typeof s.address, typeof s.setBroadcast, typeof s.setTTL);
s.on('message', (msg, rinfo) => {
  console.log('d4:', msg.toString(), rinfo.address, rinfo.family, rinfo.size);
  s.close();
});
s.on('listening', () => {
  const addr = s.address();
  console.log('d5:', addr.address, addr.family);
  s.send(Buffer.from('hello'), addr.port, '127.0.0.1');
});
s.bind(0, '127.0.0.1');
