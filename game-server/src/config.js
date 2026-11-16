'use strict';
const fs = require('fs');
const path = require('path');

const DEFAULTS = {
  host: '0.0.0.0',
  wsPort: 8080,
  udpPort: 7777,
  dbPath: path.join(__dirname, '..', '..', 'database', 'dw1_mmo.sqlite'),
  schemaPath: path.join(__dirname, '..', '..', 'database', 'schema.sql'),
  bcryptCost: 12,
  maxPlayers: 200,
  authTimeoutMs: 10_000,
  udpStaleMs: 10_000,
  persistIntervalMs: 30_000,
  saveVersionsKept: 5,        // versi memory card terakhir yang disimpan per Tamer & slot
  maxWsPayload: 256 * 1024,   // memory card 128 KiB dalam base64 + amplop JSON
  preAuthMaxPayload: 4096,    // sebelum login pesan tidak boleh sebesar ini
  quiet: false,
};

// Prioritas: DEFAULTS < config.json < env (DW1_*) < overrides (dipakai test).
function loadConfig(overrides = {}) {
  let file = {};
  try {
    file = JSON.parse(fs.readFileSync(path.join(__dirname, '..', 'config.json'), 'utf8'));
  } catch { /* config.json opsional */ }
  const env = {};
  if (process.env.DW1_HOST) env.host = process.env.DW1_HOST;
  if (process.env.DW1_WS_PORT) env.wsPort = Number(process.env.DW1_WS_PORT);
  if (process.env.DW1_UDP_PORT) env.udpPort = Number(process.env.DW1_UDP_PORT);
  if (process.env.DW1_DB_PATH) env.dbPath = process.env.DW1_DB_PATH;
  return { ...DEFAULTS, ...file, ...env, ...overrides };
}

module.exports = { loadConfig };
