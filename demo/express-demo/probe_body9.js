// 模拟 etagFn 调用
var express = require('express');
var http = require('http');
var app = express();

app.use(function (req, res, next) {
    var chunks = [];
    req.on('data', function (c) { chunks.push(c); });
    req.on('end', function () { next(); });
});

app.post('/json', function (req, res) {
    var etagFn = app.get('etag fn');
    console.log('E1 etagFn:', typeof etagFn);
    var chunk = '{"ok":true}';
    var encoding = 'utf8';
    var len = chunk.length < 1000 ? Buffer.byteLength(chunk, encoding) : 0;
    res.set('Content-Length', len);
    console.log('E2 before etag call');
    try {
        var etag = etagFn(chunk, encoding);
        console.log('E3 etag:', String(etag));
    } catch (e) {
        console.log('E3 CAUGHT:', e.message || e);
    }
    console.log('E4 after etag');
    res.end(chunk);
    console.log('E5 done');
});

var server = app.listen(0, function () {
    console.log('P1');
    var port = server.address().port;
    var req = http.request({
        port: port, path: '/json', method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Content-Length': 23 }
    }, function (res) {
        var chunks = [];
        res.on('data', function (c) { chunks.push(c); });
        res.on('end', function () {
            var text = '';
            for (var i = 0; i < chunks.length; i++) text += chunks[i].toString('utf8');
            console.log('STATUS:', res.statusCode, 'BODY:', JSON.stringify(text));
            server.close();
        });
    });
    req.write('{"name":"aluka","n":42}');
    req.end();
});