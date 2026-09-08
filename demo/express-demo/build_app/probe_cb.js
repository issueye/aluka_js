// 探针：path-to-regexp 核心 replace 回调参数对比
var path = '/echo/:word';
var re = /\\\.|(\/)?(\.)?:(\w+)(\(.*?\))?(\*)?(\?)?|[.*]|\/\(/g;
var out = path.replace(re, function(match, slash, format, key, capture, star, optional, offset) {
  console.log('CALLBACK match=' + JSON.stringify(match) + ' slash=' + JSON.stringify(slash) +
    ' format=' + JSON.stringify(format) + ' key=' + JSON.stringify(key) +
    ' capture=' + JSON.stringify(capture) + ' star=' + JSON.stringify(star) +
    ' optional=' + JSON.stringify(optional) + ' offset=' + JSON.stringify(offset));
  return '[R]';
});
console.log('OUT:', JSON.stringify(out));
// slice 行为
console.log('SLICE:', JSON.stringify(path.slice(0, 6)));