// 最小复现：1) 正则 exec 2) \uFFFD 转义 3) replace 回调
console.log('T1:', JSON.stringify('\uFFFD'));
console.log('T2:', JSON.stringify('$1\uFFFD$2'));
console.log('T3:', JSON.stringify(String('/echo/aluka42')));
console.log('T4:', JSON.stringify(encodeURI('/')));
console.log('T5:', JSON.stringify('/x/y'.replace(/x/g, function () { return 'Z'; })));

var re = new RegExp('^/echo(?:/((?:(?!/|).)+?))/?$', 'i');
console.log('T6 source:', re.source);
console.log('T6 exec:', JSON.stringify(re.exec('/echo/aluka42')));

var re2 = /^\/echo(?:\/((?:(?!\/|).)+?))\/?$/;
console.log('T7 exec:', JSON.stringify(re2.exec('/echo/aluka42')));

// 无断言版本
var re3 = /^\/echo\/([^\/]+)$/;
console.log('T8 exec:', JSON.stringify(re3.exec('/echo/aluka42')));

// 独立断言
var re4 = /(?![a-z])x/;
console.log('T9 exec:', JSON.stringify(re4.exec('axb'.replace(/x/, function(){return 'Y';}))));