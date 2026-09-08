// 直接测 etag 链
var crypto = require('crypto');
console.log('C1 createHash type:', typeof crypto.createHash);
var h = crypto.createHash('sha1');
console.log('C2 update type:', typeof h.update, 'digest type:', typeof h.digest);
var d = h.update('{"ok":true}', 'utf8').digest('base64');
console.log('C3 digest:', d, typeof d, 'substring:', typeof d.substring);
console.log('C4 sub:', d.substring(0, 27));
try {
    var etag = require('etag');
    console.log('C5 etag fn:', typeof etag);
    var e = etag('{"ok":true}', 'utf8');
    console.log('C6 etag result:', e);
} catch (e2) {
    console.log('C7 etag CAUGHT:', e2.message || e2);
}