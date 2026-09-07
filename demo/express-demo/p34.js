console.log("A setPrototypeOf:", typeof require('setprototypeof'));
console.log("B inherits:", typeof require('inherits'));
var toId = require('toidentifier');
console.log("C toIdentifier:", typeof toId, toId && toId("not found"));
var statuses = require('statuses');
console.log("D statuses.codes type:", typeof statuses.codes, "isArr:", statuses.codes ? statuses.codes.length : "-");
var codes = statuses.codes;
codes.forEach(function forEachCode (code) {
  var name = toId(statuses.message[code]);
  console.log("code:", code, "name:", name);
});
