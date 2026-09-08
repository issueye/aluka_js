// 探针：检查 req.url 在路由中的状态
var express = require('express');
var http = require('http');

var app = express();

// 通用中间件：打印 req.url
app.use(function (req, res, next) {
    console.log('MIDDLEWARE req.url:', JSON.stringify(req.url));
    console.log('MIDDLEWARE req.url length:', req.url.length);
    console.log('MIDDLEWARE first char code:', req.url.charCodeAt(0));
    // 打印每个字符的 charCodeAt
    var codes = [];
    for (var i = 0; i < req.url.length; i++) {
        codes.push(req.url.charCodeAt(i));
    }
    console.log('MIDDLEWARE char codes:', JSON.stringify(codes));
    next();
});

app.get('/echo/:word', function (req, res) {
    console.log('HANDLER req.url:', JSON.stringify(req.url));
    console.log('HANDLER req.params:', JSON.stringify(req.params));
    res.send(req.params.word);
});

var server = app.listen(0, function () {
    var port = server.address().port;
    console.log('PORT_READY');
    
    var req = http.request({
        port: port,
        path: '/echo/aluka42',
        method: 'GET'
    }, function (res) {
        var chunks = [];
        res.on('data', function (c) { chunks.push(c); });
        res.on('end', function () {
            var text = '';
            for (var i = 0; i < chunks.length; i++) {
                text += chunks[i].toString('utf8');
            }
            console.log('STATUS:', res.statusCode);
            console.log('BODY:', JSON.stringify(text));
            server.close();
        });
    });
    req.end();
});