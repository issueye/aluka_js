'use strict';
// 探针辅助模块：自定义 Error 子类的跨模块抛出
class AppError extends Error {
  constructor(message, code, status) {
    super(message);
    this.name = 'AppError';
    this.code = code;
    this.status = status;
  }
}

class SubError extends AppError {
  constructor(message) {
    super(message, 'SUBCODE', 400);
    this.name = 'SubError';
  }
}

function makeAndThrow(message, code) {
  throw new AppError(message, code, 409);
}

const PlainThrower = {
  throwIt() {
    throw new SubError('sub-boom');
  },
};

module.exports = { AppError, SubError, makeAndThrow, PlainThrower };
