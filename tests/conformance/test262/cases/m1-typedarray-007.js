
var i32 = new Int32Array(2);
assert.isTrue(Int32Array.isTypedArray(i32), "isTypedArray true");
assert.isFalse(Int32Array.isTypedArray([1, 2]), "plain array not TA");
assert.isTrue(ArrayBuffer.isView(i32), "TA is view");
assert.isFalse(ArrayBuffer.isView(new ArrayBuffer(4)), "buffer not view");
