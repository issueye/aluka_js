const insp = require('node:inspector');
console.log('i1:', typeof insp.open, typeof insp.close, typeof insp.console);
console.log('i2:', typeof insp.Session);
console.log('i3:', typeof insp.url);
console.log('i4:', insp.url());
const s = new insp.Session();
console.log('i5:', typeof s.connect, typeof s.disconnect, typeof s.post);
console.log('i6:', typeof s.on);
