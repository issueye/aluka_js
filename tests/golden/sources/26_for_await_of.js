async function* asyncGen() {
    yield 10;
    yield 20;
}
async function run() {
    let s = 0;
    for await (const x of asyncGen()) {
        s += x;
    }
    console.log("async sum:", s);
}
run();
