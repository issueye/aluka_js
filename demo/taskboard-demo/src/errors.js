'use strict';
// 领域错误：带机器可读 code，供 HTTP 层映射状态码
class AppError extends Error {
  constructor(message, code, status) {
    super(message);
    this.name = 'AppError';
    this.code = code;
    this.status = status;
  }

  toJSON() {
    return { error: this.code, message: this.message };
  }
}

class ValidationError extends AppError {
  constructor(message, details) {
    super(message, 'VALIDATION_ERROR', 400);
    this.name = 'ValidationError';
    this.details = details || [];
  }

  toJSON() {
    return { error: this.code, message: this.message, details: this.details };
  }
}

class NotFoundError extends AppError {
  constructor(id) {
    super(`任务不存在: ${id}`, 'NOT_FOUND', 404);
    this.name = 'NotFoundError';
    this.id = id;
  }
}

class ConflictError extends AppError {
  constructor(message) {
    super(message, 'CONFLICT', 409);
    this.name = 'ConflictError';
  }
}

class PayloadTooLargeError extends AppError {
  constructor(limit) {
    super(`请求体超过上限 ${limit} 字节`, 'PAYLOAD_TOO_LARGE', 413);
    this.name = 'PayloadTooLargeError';
  }
}

/** 把任意异常归一为 { status, body } 的响应描述 */
function toHttpError(err) {
  if (err instanceof AppError) {
    return { status: err.status, body: err.toJSON() };
  }
  return { status: 500, body: { error: 'INTERNAL', message: String(err && err.message) } };
}

module.exports = {
  AppError,
  ValidationError,
  NotFoundError,
  ConflictError,
  PayloadTooLargeError,
  toHttpError,
};
