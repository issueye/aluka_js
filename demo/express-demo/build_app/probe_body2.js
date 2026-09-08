// 最小复现：POST 不读 body，仅检查 url 是否损坏
var http = require('http');
var server = http.createServer(function (req, res) {
    console.log('H1 url:', JSON.stringify(req.url));
    var chunks = [];
    req.on('data', function (c) { chunks.push(c); });
    req.on('end', function () {
        console.log('H1 end url:', JSON.stringify(req.url));
        res.end('got ' + chunks.length);
    });
});
server.listen(0, function () {
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