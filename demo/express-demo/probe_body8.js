// 模拟 send 内部每一步
var express = require('express');
var http = require('http');
var app = express();

app.use(function (req, res, next) {
    var chunks = [];
    req.on('data', function (c) { chunks.push(c); });
    req.on('end', function () { next(); });
});

app.post('/json', function (req, res) {
    console.log('S1');
    if (!res.get('Content-Type')) { console.log('S2 type'); res.type('html'); }
    console.log('S3 ct:', String(res.get('Content-Type')));
    var etagFn = app.get('etag fn');
    console.log('S4 etagFn:', typeof etagFn);
    var len = Buffer.byteLength('{"ok":true}', 'utf8');
    console.log('S5 len:', len);
    res.set('Content-Length', len);
    console.log('S6 set CL');
    console.log('S7 fresh:', String(req.fresh));
    res.end('{"ok":true}');
    console.log('S8 end done');
});

var server = app.listen(0, function () {
    console.log('P1');
    var port = server.address().port;
    console.log('P2');
    var req = http.request({
        port: port, path: '/json', method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Content-Length': 23 }
    }, function (res) {
        console.log('R1');
        var chunks = [];
        res.on('data', function (c) { chunks.push(c); });
        res.on('end', function () {
            console.log('R2');
            var text = '';
            for (var i = 0; i < chunks.length; i++) text += chunks[i].toString('utf8');
            console.log('STATUS:', res.statusCode, 'BODY:', JSON.stringify(text));
            server.close();
        });
    });
    req.write('{"name":"aluka","n":42}');
    req.end();
});