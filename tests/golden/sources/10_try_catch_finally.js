function testTry(throwErr) {
    let log = [];
    try {
        log.push("try");
        if (throwErr) {
            throw new Error("fail");
        }
        log.push("try_end");
    } catch (e) {
        log.push("catch:" + e.message);
    } finally {
        log.push("finally");
    }
    return log.join("-");
}
console.log(testTry(false));
console.log(testTry(true));
