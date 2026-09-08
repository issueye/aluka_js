// 探针：express 下 url 损坏点定位
var express = require('express');
var http = require('http');
var app = express();

app.use(function (req, res, next) {
    console.log('MW1 enter url:', JSON.stringify(req.url));
    var chunks = [];
    req.on('data', function (c) { chunks.push(c); });
    req.on('end', function () {
        console.log('MW1 end url:', JSON.stringify(req.url));
        console.log('MW1 pushed:', chunks.length);
        next();
    });
});

app.post('/json', function (req, res) {
    console.log('HANDLER url:', JSON.stringify(req.url));
    console.log('HANDLER path:', JSON.stringify(req.path));
    res.end('done');
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