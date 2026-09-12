
var express = require('express');
var http = require('http');

var app = express();
app.use(express.json());

app.get('/', function (req, res) { res.send('hello from express'); });
app.get('/echo/:word', function (req, res) { res.send('echo: ' + req.params.word); });
app.post('/json', function (req, res) { res.json({ got: req.body }); });
app.get('/ctype', function (req, res) {
  res.type('application/xml');
  res.send('<root>ok</root>');
});

function httpReq(port, path, method, body) {
  return new Promise(function (resolve, reject) {
    var payload = body === undefined ? null : JSON.stringify(body);
    var req = http.request({
      port: port, path: path, method: method || 'GET',
      headers: payload ? { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) } : {}
    }, function (res) {
      var chunks = [];
      res.on('data', function (c) { chunks.push(c); });
      res.on('end', function () {
        var text = Buffer.concat(chunks).toString('utf8');
        resolve({ status: res.statusCode, ctype: res.headers['content-type'] || '', body: text });
      });
    });
    req.on('error', reject);
    if (payload) req.write(payload);
    req.end();
  });
}

var server = app.listen(0, function () {
  var port = server.address().port;
  console.log('PORT_READY');
  (async function () {
    try {
      var r1 = await httpReq(port, '/', 'GET');
      console.log('GET / ->', r1.status, r1.body);
      var r2 = await httpReq(port, '/echo/world', 'GET');
      console.log('ECHO ->', r2.status, r2.body);
      var r3 = await httpReq(port, '/json', 'POST', { n: 1 });
      console.log('POST ->', r3.status, r3.body);
      var rs = await Promise.all([
        httpReq(port, '/echo/a', 'GET'),
        httpReq(port, '/echo/b', 'GET'),
        httpReq(port, '/echo/c', 'GET')
      ]);
      console.log('CONCURRENT ->', rs.map(function (r) { return r.status; }).join(','));
      var r4 = await httpReq(port, '/ctype', 'GET');
      console.log('CTYPE ->', r4.status, r4.ctype, '|', r4.body);
      server.close(function () { console.log('CLOSED'); });
    } catch (e) {
      console.log('SCENARIO FAIL:', e && e.message ? e.message : String(e));
      process.exit(1);
    }
  })();
});
