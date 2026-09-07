/*---
negative:
  phase: runtime
  type: TypeError
---*/

var bad = new Proxy(42, {});
