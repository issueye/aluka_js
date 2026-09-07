/*---
esid: local-regexp-negative
description: RegExp 非法输入应抛 SyntaxError
negative:
  phase: runtime
  type: SyntaxError
---*/
new RegExp("(", "");
