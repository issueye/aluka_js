// 探测 express.json()
var express = require('express');
var http = require('http');
var app = express();

app.use(express.json());

app.post('/json', function (req, res) {
    console.log('H body:', JSON.stringify(req.body));
    console.log('H header cl:', String(req.headers['content-length']));
    res.json({ received: req.body });
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