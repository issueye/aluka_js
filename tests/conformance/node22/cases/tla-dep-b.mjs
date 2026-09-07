import { one } from "./tla-dep-a.mjs";
export const two = await Promise.resolve(one + 1);
export function sum(a, b) { return a + b; }
