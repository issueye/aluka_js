// 探针：POST body 事件流
var express = require('express');
var http = require('http');
var app = express();

app.use(function (req, res, next) {
    console.log('MW1 method:', req.method, 'url:', req.url);
    var chunks = [];
    req.on('data', function (c) {
        console.log('MW1 data len:', typeof c === 'string' ? c.length : (c.length !== undefined ? c.length : '?'));
        chunks.push(c);
    });
    req.on('end', function () {
        console.log('MW1 end, total chunks:', chunks.length);
        next();
    });
});

app.post('/json', function (req, res) {
    console.log('HANDLER reached');
    res.json({ ok: true });
});

var server = app.listen(0, function () {
    var port = server.address().port;
    console.log('PORT_READY');
    var req = http.request({
        port: port,
        path: '/json',
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Content-Length': 20 }
    }, function (res) {
        var chunks = [];
        res.on('data', function (c) { chunks.push(c); });
        res.on('end', function () {
            var text = '';
            for (var i = 0; i < chunks.length; i++) text += chunks[i].toString('utf8');
            console.log('STATUS:', res.statusCode);
            console.log('BODY:', JSON.stringify(text));
            server.close();
        });
    });
    req.write('{"name":"aluka","n":42}');
    req.end();
});