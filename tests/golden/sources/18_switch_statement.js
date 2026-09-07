function testSwitch(val) {
    switch(val) {
        case 1: return "one";
        case 2: return "two";
        default: return "other";
    }
}
console.log(testSwitch(1), testSwitch(2), testSwitch(3));
