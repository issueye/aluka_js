// 探针：检查 path-to-regexp 是否正常
var pathToRegexp = require('path-to-regexp');

var keys = [];
var regexp = pathToRegexp('/echo/:word', keys);
console.log('REGEXP source:', regexp.source);
console.log('REGEXP flags:', regexp.flags);
console.log('KEYS:', JSON.stringify(keys.map(function(k) { return k.name; })));

var match = regexp.exec('/echo/aluka42');
console.log('MATCH:', JSON.stringify(match ? match.slice(0) : null));

// 测试 decodeURIComponent
try {
    var decoded = decodeURIComponent('/echo/aluka42');
    console.log('DECODED:', decoded);
} catch (e) {
    console.log('DECODE ERROR:', e.message);
}

// 测试 encodeUrl
var encodeUrl = require('encodeurl');
var encoded = encodeUrl('/echo/aluka42');
console.log('ENCODED:', JSON.stringify(encoded));
console.log('ENCODED codes:', JSON.stringify(encoded.split('').map(function(c) { return c.charCodeAt(0); })));

// 测试 parseurl
var parseurl = require('parseurl');
var fakeReq = { url: '/echo/aluka42' };
var parsed = parseurl(fakeReq);
console.log('PARSED pathname:', JSON.stringify(parsed.pathname));
console.log('PARSED pathname codes:', JSON.stringify(parsed.pathname.split('').map(function(c) { return c.charCodeAt(0); })));

// 测试 finalhandler 的 getResourceName 行为
var finalhandler = require('finalhandler');
// 构造一个假的 req/res/next 来测试
console.log('ALL_PROBES_OK');