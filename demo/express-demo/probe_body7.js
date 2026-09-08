// 定位 send 挂点：分步打印
var express = require('express');
var http = require('http');
var app = express();

app.use(function (req, res, next) {
    console.log('M1');
    var chunks = [];
    req.on('data', function (c) { chunks.push(c); });
    req.on('end', function () { console.log('M2'); next(); });
});

app.post('/json', function (req, res) {
    console.log('H1');
    console.log('H2 get:', String(res.get('Content-Type')));
    console.log('H3');
    res.type('html');
    console.log('H4');
    res.send('{"ok":true}');
    console.log('H5 done');
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
            console.log('P3');
        });
    });
    req.write('{"name":"aluka","n":42}');
    req.end();
});