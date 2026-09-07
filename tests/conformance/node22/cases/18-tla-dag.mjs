// M2.2 跨模块 TLA 依赖 DAG：异步依赖链的导出时序与 node22 逐字对拍。
import { one } from "./tla-dep-a.mjs";
import { two, sum } from "./tla-dep-b.mjs";
const three = await Promise.resolve(sum(one, two));
console.log("dag:", one, two, three);
