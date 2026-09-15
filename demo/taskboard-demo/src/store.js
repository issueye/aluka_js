'use strict';
// JSON 文件存储：读-改-写，原子落盘（临时文件 + rename）
const fs = require('node:fs');
const path = require('node:path');

/** 防御性拷贝：任务对象浅拷贝 + `tags` 数组深拷贝（避免调用方改到内部状态） */
function cloneTask(task) {
  const copy = Object.assign({}, task);
  if (Array.isArray(copy.tags)) {
    copy.tags = copy.tags.slice();
  }
  return copy;
}

class TaskStore {
  constructor(file) {
    this.file = file;
    this.tasks = [];
  }

  load() {
    if (!fs.existsSync(this.file)) {
      this.tasks = [];
      return this;
    }
    const raw = fs.readFileSync(this.file, 'utf8');
    const parsed = raw.trim() === '' ? [] : JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      throw new TypeError(`数据文件格式损坏（期望数组）: ${this.file}`);
    }
    this.tasks = parsed.map((task) => cloneTask(task));
    return this;
  }

  save() {
    fs.mkdirSync(path.dirname(this.file), { recursive: true });
    const payload = JSON.stringify(this.tasks, null, 2);
    const tmp = `${this.file}.tmp`;
    fs.writeFileSync(tmp, payload, 'utf8');
    fs.renameSync(tmp, this.file);
    return payload.length;
  }

  all() {
    return this.tasks.map((task) => cloneTask(task));
  }

  get(id) {
    const found = this.tasks.find((task) => task.id === id);
    return found ? cloneTask(found) : null;
  }

  insert(task) {
    this.tasks.push(cloneTask(task));
    return cloneTask(task);
  }

  replace(id, task) {
    const index = this.tasks.findIndex((item) => item.id === id);
    if (index < 0) return null;
    this.tasks[index] = cloneTask(task);
    return cloneTask(task);
  }

  remove(id) {
    const index = this.tasks.findIndex((item) => item.id === id);
    if (index < 0) return null;
    const [removed] = this.tasks.splice(index, 1);
    return cloneTask(removed);
  }

  get size() {
    return this.tasks.length;
  }
}

module.exports = { TaskStore, cloneTask };

