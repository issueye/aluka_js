async function fetchNumber(n) {
    return n * 2;
}
async function main() {
    const res = await fetchNumber(21);
    console.log("async result:", res);
}
main();
