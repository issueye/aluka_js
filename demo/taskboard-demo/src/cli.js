#!/usr/bin/env node
'use strict';
// CLI：add / list / done / rm / stats / serve —— 退出码语义 0=成功 1=业务错误 2=用法错误
const path = require('node:path');
const { loadConfig } = require('./config');
const { createLogger } = require('./logger');
const { TaskStore } = require('./store');
const { TaskService } = require('./service');
const { AppError } = require('./errors');
const { TaskServer } = require('./http-server');

function parseArgv(argv) {
  const args = argv.slice(2);
  const command = args[0];
  const rest = args.slice(1);
  const flags = {};
  const positional = [];
  for (let i = 0; i < rest.length; i += 1) {
    const item = rest[i];
    if (item.startsWith('--')) {
      const eq = item.indexOf('=');
      if (eq > 0) {
        flags[item.slice(2, eq)] = item.slice(eq + 1);
      } else if (rest[i + 1] !== undefined && !rest[i + 1].startsWith('--')) {
        flags[item.slice(2)] = rest[i + 1];
        i += 1;
      } else {
        flags[item.slice(2)] = true;
      }
    } else {
      positional.push(item);
    }
  }
  return { command, flags, positional };
}

function buildContext(root) {
  const config = loadConfig(root);
  const logger = createLogger({ level: config.logLevel, sink: (line) => process.stdout.write(`${line}\n`) });
  const store = new TaskStore(config.dataFile).load();
  const service = new TaskService(store, { logger });
  return { config, logger, store, service };
}

const USAGE = [
  'taskboard <command> [options]',
  '',
  '命令:',
  '  add <标题> [--priority high] [--tags a,b]   新建任务',
  '  list [--status todo|doing|done] [--limit N] 列出任务',
  '  done <id>                                    标记完成',
  '  rm <id>                                      删除任务',
  '  stats                                        统计',
  '  serve [--port 3000]                          启动 HTTP 服务',
].join('\n');

async function main() {
  const root = process.cwd();
  const { command, flags, positional } = parseArgv(process.argv);
  if (command === undefined || command === 'help' || command === '--help') {
    process.stdout.write(`${USAGE}\n`);
    return 0;
  }
  const ctx = buildContext(root);

  switch (command) {
    case 'add': {
      const task = ctx.service.create({
        title: positional.join(' '),
        priority: flags.priority,
        tags: flags.tags ? String(flags.tags).split(',') : undefined,
      });
      ctx.store.save();
      process.stdout.write(`created ${task.id} ${task.status} ${task.fingerprint} ${task.title}\n`);
      return 0;
    }
    case 'list': {
      const items = ctx.service.list({
        status: flags.status,
        priority: flags.priority,
        tag: flags.tag,
        limit: flags.limit ? Number(flags.limit) : undefined,
      });
      process.stdout.write(`count=${items.length}\n`);
      for (const task of items) {
        process.stdout.write(
          `${task.id}\t${task.status}\t${task.priority}\t${task.tags.join(',') || '-'}\t${task.title}\n`
        );
      }
      return 0;
    }
    case 'done': {
      const task = ctx.service.update(positional[0], { status: 'done' });
      ctx.store.save();
      process.stdout.write(`done ${task.id} ${task.status}\n`);
      return 0;
    }
    case 'rm': {
      const removed = ctx.service.remove(positional[0]);
      ctx.store.save();
      process.stdout.write(`removed ${removed.id} ${removed.title}\n`);
      return 0;
    }
    case 'stats': {
      const stats = ctx.service.stats();
      process.stdout.write(`total=${stats.total} completion=${stats.completion}%\n`);
      process.stdout.write(
        `counts todo=${stats.counts.todo} doing=${stats.counts.doing} done=${stats.counts.done}\n`
      );
      for (const entry of stats.topTags) {
        process.stdout.write(`tag ${entry.tag}=${entry.count}\n`);
      }
      return 0;
    }
    case 'serve': {
      const server = new TaskServer(ctx.service, {
        logger: ctx.logger,
        maxBodyBytes: ctx.config.maxBodyBytes,
      });
      const address = await server.listen(
        flags.port ? Number(flags.port) : ctx.config.port,
        ctx.config.host
      );
      process.stdout.write(`listening ${address.address}:${address.port}\n`);
      process.stdout.write('READY\n');
      return 0;
    }
    default:
      process.stderr.write(`未知命令: ${command}\n`);
      process.stdout.write(`${USAGE}\n`);
      return 2;
  }
}

main()
  .then((code) => {
    process.exitCode = code;
  })
  .catch((err) => {
    const isApp = err instanceof AppError;
    const prefix = isApp ? `${err.code}: ` : '';
    process.stderr.write(`${prefix}${err && err.message}\n`);
    if (Array.isArray(err && err.details)) {
      for (const detail of err.details) {
        process.stderr.write(`  - ${detail.field}: ${detail.message}\n`);
      }
    }
    process.exitCode = 1;
  });
