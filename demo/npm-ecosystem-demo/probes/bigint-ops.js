function t(label, fn) {
  try { console.log(label + '=' + JSON.stringify(fn())); }
  catch (e) { console.log(label + '=ERR ' + e.name + ': ' + e.message); }
}
t('shift', () => (1n << 41n).toString());
t('shiftNum', () => (1n << 41).toString());
t('sub', () => ((1n << 41n) - 1n).toString());
t('bigintOf', () => BigInt(255).toString());
t('number', () => Number(255n));
t('rshift', () => (255n >> 4n).toString());
t('and', () => (0x70 | Number((3n >> 1n) & 0x3fn)));
t('or', () => (0x80 | 0x01).toString(16));
t('mixedOr', () => (5n | 2n).toString());
