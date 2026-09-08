var iconv = require('iconv-lite');
console.log('G getCodec:', typeof iconv.getCodec);
try {
  var codec = iconv.getCodec('utf-8');
  console.log('OK codec:', typeof codec, 'decoder:', typeof codec.decoder);
  var d = new (codec.decoder)({}, codec);
  console.log('OK decoder:', typeof d);
} catch (e) {
  console.log('CAUGHT:', e.message);
  console.log('encodings:', typeof iconv.encodings);
  if (iconv.encodings) {
    console.log('_internal:', typeof iconv.encodings['_internal']);
  }
}
