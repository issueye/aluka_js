'use strict';
// 业务层：校验 + 状态流转 + 统计（纯逻辑，可单测；不依赖 fs/网络）
const crypto = require('node:crypto');
const { ValidationError, NotFoundError, ConflictError } = require('./errors');

const STATUSES = ['todo', 'doing', 'done'];
const PRIORITIES = ['low', 'normal', 'high'];

/** 稳定指纹：标题的 sha256 前 10 位（用于去重与幂等） */
function fingerprint(title) {
  return crypto.createHash('sha256').update(title, 'utf8').digest('hex').slice(0, 10);
}

function nowIso(clock) {
  const ms = typeof clock === 'function' ? clock() : Date.now();
  return new Date(ms).toISOString();
}

class TaskService {
  /**
   * @param {import('./store').TaskStore} store
   * @param {object} options { clock, logger, maxTitleLength }
   */
  constructor(store, options) {
    const opts = options || {};
    this.store = store;
    this.clock = opts.clock || (() => Date.now());
    this.logger = opts.logger || null;
    this.maxTitleLength = opts.maxTitleLength || 80;
    this.seq = 0;
  }

  _log(level, message, fields) {
    if (this.logger && typeof this.logger[level] === 'function') {
      this.logger[level](message, fields);
    }
  }

  validateInput(input) {
    const errors = [];
    const data = input || {};
    const title = typeof data.title === 'string' ? data.title.trim() : '';
    if (title === '') {
      errors.push({ field: 'title', message: '标题不能为空' });
    } else if (title.length > this.maxTitleLength) {
      errors.push({ field: 'title', message: `标题过长（上限 ${this.maxTitleLength}）` });
    }
    if (data.status !== undefined && !STATUSES.includes(data.status)) {
      errors.push({ field: 'status', message: `状态非法: ${data.status}` });
    }
    if (data.priority !== undefined && !PRIORITIES.includes(data.priority)) {
      errors.push({ field: 'priority', message: `优先级非法: ${data.priority}` });
    }
    if (data.tags !== undefined && !Array.isArray(data.tags)) {
      errors.push({ field: 'tags', message: '标签必须是数组' });
    }
    if (errors.length > 0) {
      throw new ValidationError('任务字段校验失败', errors);
    }
    return {
      title,
      status: data.status || 'todo',
      priority: data.priority || 'normal',
      tags: (data.tags || []).map((tag) => String(tag)),
      fingerprint: fingerprint(title),
    };
  }

  create(input) {
    const data = this.validateInput(input);
    const existing = this.store
      .all()
      .find((task) => task.fingerprint === data.fingerprint && task.status !== 'done');
    if (existing) {
      throw new ConflictError(`重复任务（指纹 ${data.fingerprint}）: ${existing.id}`);
    }
    this.seq += 1;
    const id = `T${String(this.seq).padStart(3, '0')}`;
    const task = Object.assign({ id }, data, {
      createdAt: nowIso(this.clock),
      updatedAt: nowIso(this.clock),
    });
    this.store.insert(task);
    this._log('info', 'task.created', { id: task.id, status: task.status });
    return task;
  }

  get(id) {
    const task = this.store.get(id);
    if (!task) throw new NotFoundError(id);
    return task;
  }

  update(id, patch) {
    const current = this.get(id);
    const data = patch || {};
    if (data.title !== undefined && String(data.title).trim() === '') {
      throw new ValidationError('任务字段校验失败', [
        { field: 'title', message: '标题不能为空' },
      ]);
    }
    if (data.status !== undefined && !STATUSES.includes(data.status)) {
      throw new ValidationError('任务字段校验失败', [
        { field: 'status', message: `状态非法: ${data.status}` },
      ]);
    }
    const next = Object.assign({}, current, data, { updatedAt: nowIso(this.clock) });
    if (data.title !== undefined) {
      next.title = String(data.title).trim();
      next.fingerprint = fingerprint(next.title);
    }
    this.store.replace(id, next);
    this._log('info', 'task.updated', { id, status: next.status });
    return next;
  }

  remove(id) {
    const removed = this.store.remove(id);
    if (!removed) throw new NotFoundError(id);
    this._log('info', 'task.removed', { id });
    return removed;
  }

  list(filter) {
    const opts = filter || {};
    let items = this.store.all();
    if (opts.status) {
      items = items.filter((task) => task.status === opts.status);
    }
    if (opts.priority) {
      items = items.filter((task) => task.priority === opts.priority);
    }
    if (opts.tag) {
      items = items.filter((task) => task.tags.includes(opts.tag));
    }
    const sorted = items.slice().sort((a, b) => {
      if (a.status !== b.status) return a.status < b.status ? -1 : 1;
      return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
    });
    return opts.limit ? sorted.slice(0, opts.limit) : sorted;
  }

  stats() {
    const counts = { todo: 0, doing: 0, done: 0 };
    const byPriority = { low: 0, normal: 0, high: 0 };
    const tags = {};
    for (const task of this.store.all()) {
      counts[task.status] = (counts[task.status] || 0) + 1;
      byPriority[task.priority] = (byPriority[task.priority] || 0) + 1;
      for (const tag of task.tags) {
        tags[tag] = (tags[tag] || 0) + 1;
      }
    }
    const total = this.store.size;
    const topTags = Object.keys(tags)
      .sort((a, b) => (tags[b] - tags[a]) || (a < b ? -1 : 1))
      .map((tag) => ({ tag, count: tags[tag] }));
    return {
      total,
      counts,
      byPriority,
      topTags,
      completion: total === 0 ? 0 : Math.round((counts.done / total) * 100),
    };
  }
}

module.exports = { TaskService, STATUSES, PRIORITIES, fingerprint };
