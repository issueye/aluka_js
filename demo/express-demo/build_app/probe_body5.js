// 模拟 res.json 内部步骤
var express = require('express');
var http = require('http');
var app = express();

app.use(function (req, res, next) {
    var chunks = [];
    req.on('data', function (c) { chunks.push(c); });
    req.on('end', function () { next(); });
});

app.post('/json', function (req, res) {
    console.log('H url:', JSON.stringify(req.url));
    var body = JSON.stringify({ ok: true });
    console.log('H json stringify:', body);
    res.set('Content-Type', 'application/json');
    console.log('H after set, url:', JSON.stringify(req.url));
    res.end(body);
    console.log('H after end, url:', JSON.stringify(req.url));
});

var server = app.listen(0, function () {
    var port = server.address().port;
    console.log('PORT_READY');
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