'use strict';
// 探针：src/errors.js 的三级继承链（Error → AppError → ConflictError）实例形态
const { AppError, ConflictError, ValidationError } = require('./src/errors');

function dump(label, e) {
  console.log(`--- ${label} ---`);
  console.log('  typeof:', typeof e, 'isObject:', e !== null && typeof e === 'object');
  console.log('  ownProps:', JSON.stringify(Object.getOwnPropertyNames(e)));
  console.log('  name:', JSON.stringify(e.name));
  console.log('  message:', JSON.stringify(e.message));
  console.log('  code:', JSON.stringify(e.code));
  console.log('  status:', JSON.stringify(e.status));
  console.log('  instanceof Error:', e instanceof Error);
  console.log('  instanceof AppError:', e instanceof AppError);
  console.log('  instanceof ConflictError:', e instanceof ConflictError);
  console.log('  ctor.name:', JSON.stringify(e.constructor && e.constructor.name));
  console.log('  typeof toJSON:', typeof e.toJSON);
}

dump('new ConflictError("m1")', new ConflictError('m1'));
dump('new AppError("m2","C2",409)', new AppError('m2', 'C2', 409));
dump('new ValidationError("m3",[])', new ValidationError('m3', []));

// 在**类方法**中抛出（同模块）
class Svc {
  static boom() {
    throw new ConflictError('from-class-method');
  }
}
try {
  Svc.boom();
  console.log('class-method throw: NO_ERROR');
} catch (err) {
  console.log('class-method throw: typeof=', typeof err, 'name=', JSON.stringify(err && err.name), 'code=', JSON.stringify(err && err.code));
}
console.log('PROBE8_DONE');
