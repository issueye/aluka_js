const express = require('express');
const app = express();
app.use(express.json());

const hits = { count: 0 };

app.get('/', (req, res) => {
  hits.count += 1;
  res.send('hello from express');
});

app.get('/echo/:word', (req, res) => {
  res.send('echo: ' + req.params.word);
});

app.post('/json', (req, res) => {
  res.json({ got: req.body });
});

app.get('/ctype', (req, res) => {
  res.set('Content-Type', 'application/xml; charset=utf-8');
  res.send('<root>ok</root>');
});

const server = app.listen(0, () => {
  const port = server.address().port;
  console.log('PORT_READY');
  const base = 'http://127.0.0.1:' + port;

  (async () => {
    console.log('GET / -> ' + (await (await fetch(base + '/')).text()));

    console.log('ECHO -> ' + (await (await fetch(base + '/echo/world')).text()));

    const pr = await fetch(base + '/json', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ n: 1 })
    });
    console.log('POST -> ' + (await pr.text()));

    const rs = await Promise.all([fetch(base + '/'), fetch(base + '/'), fetch(base + '/')]);
    const texts = await Promise.all(rs.map((r) => r.text()));
    console.log('CONCURRENT -> ' + texts.join(','));

    const cr = await fetch(base + '/ctype');
    console.log('CTYPE -> ' + cr.headers.get('content-type') + ' | ' + (await cr.text()));

    server.close(() => {
      console.log('CLOSED');
    });
  })().catch((e) => { console.error('CLIENT FAIL: ' + (e && e.message ? e.message : e)); process.exit(1); });
});
