try {
  throw new Error('test');
} catch (e) {
  console.log('CAUGHT:', e.message);
}