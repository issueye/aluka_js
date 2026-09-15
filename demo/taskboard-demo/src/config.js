'use strict';
// 配置装载：默认值 <- 配置文件 <- 环境变量（优先级递增）
const fs = require('node:fs');
const path = require('node:path');

const DEFAULTS = {
  host: '127.0.0.1',
  port: 0,
  dataDir: '.data',
  logLevel: 'info',
  maxBodyBytes: 64 * 1024,
};

const LEVELS = ['debug', 'info', 'warn', 'error'];

/**
 * 读取项目配置。
 * @param {string} root 项目根目录
 * @returns {object} 合并后的配置（含派生字段 dataFile）
 */
function loadConfig(root) {
  const configPath = path.join(root, 'taskboard.config.json');
  let fileConfig = {};
  if (fs.existsSync(configPath)) {
    fileConfig = JSON.parse(fs.readFileSync(configPath, 'utf8'));
  }

  const envConfig = {};
  if (process.env.TASKBOARD_LOG_LEVEL) {
    envConfig.logLevel = process.env.TASKBOARD_LOG_LEVEL;
  }
  if (process.env.TASKBOARD_PORT) {
    envConfig.port = Number(process.env.TASKBOARD_PORT);
  }

  const config = Object.assign({}, DEFAULTS, fileConfig, envConfig);
  if (!LEVELS.includes(config.logLevel)) {
    throw new RangeError(`未知日志级别: ${config.logLevel}`);
  }
  config.dataFile = path.join(root, config.dataDir, 'tasks.json');
  return config;
}

module.exports = { loadConfig, DEFAULTS, LEVELS };
