// M4 差分用例：Web 标准事件基类与表单（EventTarget/CustomEvent/FormData）。
const et = new EventTarget();
const ce = new CustomEvent('ping', { detail: { n: 42 } });
let log = [];
et.addEventListener('ping', (e) => log.push('ping:' + e.detail.n));
et.addEventListener('ping', () => log.push('second'));
et.removeEventListener('ping', () => {});
console.log('dispatch-ret:', et.dispatchEvent(ce));
console.log('log:', log.join('|'));
console.log('ce.type:', ce.type, 'ce.detail:', ce.detail.n);

const fd = new FormData();
fd.append('name', 'aluka');
fd.append('tags', 'a');
fd.append('tags', 'b');
fd.set('name', 'aluka2');
console.log('fd.get:', fd.get('name'), '| fd.has:', fd.has('tags'), '| fd.has-x:', fd.has('x'));
console.log('fd.getAll:', fd.getAll('tags').join(','));
const keys = [];
fd.forEach((v, k) => keys.push(k + '=' + v));
console.log('forEach:', keys.join('|'));
fd.delete('tags');
console.log('after-delete:', fd.get('tags'), '| getAll-len:', fd.getAll('tags').length);
