function tryLoop() {
    for (let i = 0; i < 3; i++) {
        try {
            if (i === 1) break;
        } finally {
            console.log("fin:", i);
        }
    }
}
tryLoop();
