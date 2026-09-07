const list = [1, 2, 3];
list.push(4, 5);
const spreadList = [0, ...list, 6];
const [first, second, ...rest] = spreadList;
console.log(first, second, rest.join(","));
