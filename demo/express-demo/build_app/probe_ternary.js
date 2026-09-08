// 探针：三目运算与 || 行为
console.log('T1:', '' ? 'A' : 'B');
console.log('T2:', 'x' ? 'A' : 'B');
console.log('T3:', 0 ? 'A' : 'B');
console.log('T4:', null ? 'A' : 'B');
console.log('T5:', undefined ? 'A' : 'B');
console.log('T6:', '/' || 'R');
console.log('T7:', '' || 'R');
console.log('T8:', null || 'R');
console.log('T9:', undefined || 'R');

var backtrack = '';
console.log('T10:', backtrack ? 'ABRANCH' : 'BBRANCH');
console.log('T11:', backtrack ? '((?:(?!/|' + backtrack + ').)+?)' : '([^' + '/' + ']+?)');