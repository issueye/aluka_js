/*---
negative:
  phase: runtime
  type: TypeError
description: 运行时错误应抛 TypeError
---*/
var obj = null;
obj.foo;
