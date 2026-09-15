'use strict';
// 结构化日志：`[level] message key=value …`，字段按 key 排序保证输出稳定
const { LEVELS } = require('./config');

const WEIGHT = { debug: 10, info: 20, warn: 30, error: 40 };

function isStderrTarget(level) {
  return level === 'warn' || level === 'error';
}

function formatRecord(level, message, fields) {
  const parts = [`[${level}]`, message];
  const keys = Object.keys(fields || {}).sort();
  for (const key of keys) {
    const value = fields[key];
    parts.push(`${key}=${typeof value === 'string' ? value : JSON.stringify(value)}`);
  }
  return parts.join(' ');
}

/**
 * 创建日志器。
 * @param {object} options { level, sink(info, line), sinkError(line) }
 */
function createLogger(options) {
  const level = options.level || 'info';
  const lines = [];
  const threshold = WEIGHT[level];
  const sink = options.sink || ((text) => lines.push(text));
  const sinkError = options.sinkError || sink;

  function emit(lv, message, fields) {
    if (WEIGHT[lv] < threshold) return null;
    const line = formatRecord(lv, message, fields);
    if (isStderrTarget(lv)) {
      sinkError(line);
    } else {
      sink(line);
    }
    return line;
  }

  return {
    level,
    lines,
    debug: (m, f) => emit('debug', m, f),
    info: (m, f) => emit('info', m, f),
    warn: (m, f) => emit('warn', m, f),
    error: (m, f) => emit('error', m, f),
    // 预置字段的派生日志器（如 module=http 的固定字段）
    with(fields) {
      const base = fields || {};
      return {
        level,
        lines,
        debug: (m, f) => emit('debug', m, Object.assign({}, base, f)),
        info: (m, f) => emit('info', m, Object.assign({}, base, f)),
        warn: (m, f) => emit('warn', m, Object.assign({}, base, f)),
        error: (m, f) => emit('error', m, Object.assign({}, base, f)),
      };
    },
  };
}

module.exports = { createLogger, formatRecord, WEIGHT, LEVELS };
