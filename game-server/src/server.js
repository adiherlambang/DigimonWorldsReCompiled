'use strict';
const { loadConfig } = require('./config');
const { openDb, createStore } = require('./db');
const { SessionManager } = require('./sessions');
const { createUdp } = require('./udp');
const { createWs } = require('./ws');

function makeLogger(quiet) {
  const ts = () => new Date().toISOString();
  return {
    info: (...a) => { if (!quiet) console.log(ts(), '[INFO]', ...a); },
    error: (...a) => console.error(ts(), '[ERROR]', ...a),
  };
}

async function startServer(overrides = {}) {
  const cfg = loadConfig(overrides);
  const log = makeLogger(cfg.quiet);
  const db = openDb(cfg.dbPath, cfg.schemaPath);
  const store = createStore(db);
  const sessions = new SessionManager(store, cfg, log);
  const udp = createUdp(sessions, cfg, log);
  const udpPortRef = { port: cfg.udpPort };
  const ws = createWs(sessions, store, cfg, log, udpPortRef);

  udpPortRef.port = await udp.bind();
  const wsPort = await ws.listening();

  const timers = [
    setInterval(() => sessions.sweepStale(), 2000),
    setInterval(() => sessions.persistDirty(), cfg.persistIntervalMs),
  ];
  timers.forEach((t) => t.unref());
  log.info(`DB: ${cfg.dbPath}`);
  log.info(`WebSocket (auth) di ${cfg.host}:${wsPort} | UDP (fast-sync) di ${cfg.host}:${udpPortRef.port}`);

  return {
    cfg, db, sessions, udp, wsPort, udpPort: udpPortRef.port,
    async stop() {
      timers.forEach(clearInterval);
      sessions.removeAll('server berhenti');
      await ws.close();
      await udp.close();
      db.close();
    },
  };
}

module.exports = { startServer };
