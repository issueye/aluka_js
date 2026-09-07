const http2 = require('node:http2');
console.log('h1:', typeof http2.connect, typeof http2.createServer, typeof http2.constants);
console.log('h2:', typeof http2.getDefaultSettings);
const s = http2.getDefaultSettings();
console.log('h3:', JSON.stringify(Object.keys(s).sort()));
console.log('h4:', http2.constants.HTTP2_HEADER_METHOD !== undefined);
console.log('h5:', http2.constants.HTTP2_HEADER_STATUS, http2.constants.HTTP2_FRAME_DATA);
console.log('h6:', typeof http2.getPackedSettings, typeof http2.getUnpackedSettings);
console.log('h7:', typeof http2.sensitiveHeaders);
