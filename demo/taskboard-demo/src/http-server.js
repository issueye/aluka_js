'use strict';
// HTTP 层：路由 + JSON 体解析（带上限）+ 统一错误映射
const http = require('node:http');
const { EventEmitter } = require('node:events');
const { ValidationError, PayloadTooLargeError, toHttpError } = require('./errors');

/** 读取请求体（带上限），返回 Promise<string> */
function readBody(req, limit) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    req.on('data', (chunk) => {
      size += chunk.length;
      if (size > limit) {
        // 超限：停止累积并**先让调用方发回 413**（此前这里 req.destroy() 会
        // 抢在响应前销毁 socket，客户端只见 ECONNRESET、服务端也拿不到写回
        // 机会）；残余数据 drain 掉，避免连接卡在半读状态。
        req.removeAllListeners('data');
        req.resume();
        reject(new PayloadTooLargeError(limit));
        return;
      }
      chunks.push(chunk);
    });
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    req.on('error', reject);
  });
}

function sendJson(res, status, payload, extraHeaders) {
  const body = JSON.stringify(payload);
  const headers = Object.assign(
    {
      'content-type': 'application/json',
      'content-length': Buffer.byteLength(body),
    },
    extraHeaders || {}
  );
  res.writeHead(status, headers);
  res.end(body);
}

function parseQuery(url) {
  const index = url.indexOf('?');
  if (index < 0) return {};
  const out = {};
  for (const pair of url.slice(index + 1).split('&')) {
    if (pair === '') continue;
    const eq = pair.indexOf('=');
    const key = eq < 0 ? pair : pair.slice(0, eq);
    const value = eq < 0 ? '' : decodeURIComponent(pair.slice(eq + 1));
    out[key] = value;
  }
  return out;
}

class TaskServer extends EventEmitter {
  /**
   * @param {import('./service').TaskService} service
   * @param {object} options { logger, maxBodyBytes }
   */
  constructor(service, options) {
    super();
    const opts = options || {};
    this.service = service;
    this.logger = opts.logger || null;
    this.maxBodyBytes = opts.maxBodyBytes || 64 * 1024;
    this.server = http.createServer((req, res) => {
      this.handle(req, res).catch((err) => {
        const mapped = toHttpError(err);
        sendJson(res, mapped.status, mapped.body);
      });
    });
  }

  listen(port, host) {
    return new Promise((resolve) => {
      this.server.listen(port, host, () => {
        const address = this.server.address();
        this.emit('listening', address);
        resolve(address);
      });
    });
  }

  close() {
    return new Promise((resolve, reject) => {
      this.server.close((err) => (err ? reject(err) : resolve()));
    });
  }

  async route(req) {
    const method = req.method;
    const url = req.url;
    const query = parseQuery(url);
    const pathname = url.indexOf('?') < 0 ? url : url.slice(0, url.indexOf('?'));
    const segments = pathname.split('/').filter((part) => part !== '');

    if (segments[0] === 'health') {
      return { status: 200, body: { ok: true, uptimeClass: 'static' } };
    }
    if (segments[0] === 'stats' && method === 'GET') {
      return { status: 200, body: this.service.stats() };
    }
    if (segments[0] === 'tasks') {
      if (segments.length === 1) {
        if (method === 'GET') {
          const items = this.service.list(query);
          return { status: 200, body: { items, count: items.length } };
        }
        if (method === 'POST') {
          const raw = await readBody(req, this.maxBodyBytes);
          if (raw.trim() === '') {
            throw new ValidationError('请求体不能为空', [
              { field: 'body', message: '缺少 JSON 请求体' },
            ]);
          }
          let parsed;
          try {
            parsed = JSON.parse(raw);
          } catch (err) {
            throw new ValidationError('请求体不是合法 JSON', [
              { field: 'body', message: String(err && err.message) },
            ]);
          }
          const task = this.service.create(parsed);
          this.service.store.save();
          return { status: 201, body: task, headers: { location: `/tasks/${task.id}` } };
        }
        return { status: 405, body: { error: 'METHOD_NOT_ALLOWED', method } };
      }
      const id = segments[1];
      if (method === 'GET') {
        return { status: 200, body: this.service.get(id) };
      }
      if (method === 'PATCH') {
        const raw = await readBody(req, this.maxBodyBytes);
        const patch = raw.trim() === '' ? {} : JSON.parse(raw);
        const updated = this.service.update(id, patch);
        this.service.store.save();
        return { status: 200, body: updated };
      }
      if (method === 'DELETE') {
        const removed = this.service.remove(id);
        this.service.store.save();
        return { status: 200, body: { removed: removed.id } };
      }
      return { status: 405, body: { error: 'METHOD_NOT_ALLOWED', method } };
    }
    return { status: 404, body: { error: 'NOT_FOUND', path: pathname } };
  }

  async handle(req, res) {
    const result = await this.route(req);
    if (this.logger) {
      this.logger.info('http.request', { method: req.method, path: req.url, status: result.status });
    }
    // 写回失败（对端已断开等）不应再抛进事件循环
    try {
      sendJson(res, result.status, result.body, result.headers);
    } catch (err) {
      if (this.logger) {
        this.logger.warn('http.write_failed', { path: req.url, message: String(err && err.message) });
      }
    }
  }
}

module.exports = { TaskServer, readBody, sendJson, parseQuery };
