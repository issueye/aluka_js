console.log("Number('404'):", Number("404"));
console.log("mul:", "5" * 2);
var codes = { "100": "Continue", "404": "Not Found" };
var list = Object.keys(codes).map(function (c) { return Number(c); });
console.log("list:", JSON.stringify(list));
