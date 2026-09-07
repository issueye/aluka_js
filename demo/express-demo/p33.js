console.log("lc:", typeof process.listenerCount, "listeners:", typeof process.listeners, "env:", typeof process.env);
var depd = require('depd');
console.log("factory:", typeof depd);
var deprecate = depd('http-errors');
console.log("deprecate:", typeof deprecate);
