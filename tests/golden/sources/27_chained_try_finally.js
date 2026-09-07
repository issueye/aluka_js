function chained() {
    let out = [];
    try {
        out.push("A");
        try {
            out.push("B");
            return "VAL";
        } finally {
            out.push("FIN_B");
        }
    } finally {
        out.push("FIN_A");
    }
}
console.log(chained());
