const cluster = require('node:cluster');
console.log('c1:', typeof cluster.fork, typeof cluster.isMaster, typeof cluster.settings);
console.log('c2:', cluster.isPrimary !== undefined, cluster.isMaster !== undefined);
console.log('c3:', typeof cluster.Worker);
console.log('c4:', typeof cluster.schedulingPolicy);
console.log('c5:', typeof cluster.disconnect, typeof cluster.setupMaster);
console.log('c6:', typeof cluster.workers, cluster.isWorker !== undefined);
