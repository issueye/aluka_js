console.log(JSON.stringify([Number.isInteger(1.0), Number.isSafeInteger(2**53), Number.parseInt("0x1f",16)]));
console.log(JSON.stringify([Number.parseFloat("1.5x"), (0.1+0.2).toFixed(2), (1234.5678).toPrecision(6)]));
console.log(JSON.stringify([(255).toString(16), (255).toString(2), Math.max(1,2,3), Math.min(1,2,3)]));
console.log(JSON.stringify([Math.round(2.5), Math.round(-2.5), Math.trunc(-2.7), Math.sign(-3)]));
console.log(JSON.stringify([Math.hypot(3,4), Math.cbrt(27), Math.log2(8), Number("  12  ")]));
console.log(JSON.stringify([(1e21).toString(), (1e-7).toString(), Number.MAX_SAFE_INTEGER]));
console.log(JSON.stringify([Number.isNaN(NaN), Number.isFinite(1), Math.abs(-3), Math.floor(-2.5)]));
console.log(JSON.stringify([(1234.5678).toExponential(2), Number.EPSILON>0, Number.isInteger("1")]));
