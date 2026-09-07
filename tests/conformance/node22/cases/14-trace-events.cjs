const te = require('node:trace_events');
console.log('t1:', typeof te.createTracing, typeof te.getEnabledCategories);
console.log('t2:', te.getEnabledCategories());
const t = te.createTracing({ categories: ['node.fs'] });
console.log('t3:', typeof t.enable, typeof t.disable, t.enabled);
t.enable();
console.log('t4:', t.enabled);
t.disable();
console.log('t5:', t.enabled);
