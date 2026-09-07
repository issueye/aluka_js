
var base = [3, 1, 2];
base.toSorted();
base.toReversed();
assert.sameValue(base.join(","), "3,1,2", "immutable methods do not mutate");
