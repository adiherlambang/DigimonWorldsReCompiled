'use strict';
const { startServer } = require('./src/server');

startServer().then((srv) => {
  const shutdown = async (sig) => {
    console.log(`\n${sig} diterima, menyimpan dan berhenti...`);
    await srv.stop();
    process.exit(0);
  };
  process.on('SIGINT', () => shutdown('SIGINT'));
  process.on('SIGTERM', () => shutdown('SIGTERM'));
}).catch((err) => {
  console.error('Server gagal start:', err);
  process.exit(1);
});
