#!/usr/bin/env node
'use strict';
// e2e 公共工具：确定性输出 + 进程内 HTTP 往返 + 固定时钟
const http = require('node:http');

// 项目根 = 当前工作目录（node / aluka 均从项目根调用，保证两方口径一致；
// 不用 __dirname——aluka 编译镜像下 __dirname 指向 aluka_build/）。
const ROOT = process.cwd();
const DATA_DIR = require('node:path').join(ROOT, '.data-e2e');
const DATA_FILE = require('node:path').join(DATA_DIR, 'tasks.json');

/** 固定时钟：保证 createdAt/updatedAt 稳定可比 */
function fixedClock() {
  return Date.UTC(2026, 8, 15, 9, 0, 0);
}

function request(port, method, urlPath, body) {
  return new Promise((resolve, reject) => {
    const payload = body === undefined ? null : JSON.stringify(body);
    const req = http.request(
      {
        host: '127.0.0.1',
        port,
        path: urlPath,
        method,
        headers: payload
          ? { 'content-type': 'application/json', 'content-length': Buffer.byteLength(payload) }
          : {},
      },
      (res) => {
        const chunks = [];
        res.on('data', (chunk) => chunks.push(chunk));
        res.on('end', () => {
          const text = Buffer.concat(chunks).toString('utf8');
          let parsed = null;
          try {
            parsed = JSON.parse(text);
          } catch (err) {
            parsed = { raw: text };
          }
          resolve({ status: res.statusCode, type: res.headers['content-type'], body: parsed });
        });
      }
    );
    req.on('error', reject);
    if (payload) req.write(payload);
    req.end();
  });
}

function section(name) {
  process.stdout.write(`\n### ${name}\n`);
}

function line(...parts) {
  process.stdout.write(`${parts.join(' ')}\n`);
}

module.exports = { request, section, line, fixedClock, ROOT, DATA_DIR, DATA_FILE };
