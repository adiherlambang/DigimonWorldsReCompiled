'use strict';
const WebSocket = require('ws');
const bcrypt = require('bcryptjs');
const { DuplicateError } = require('./db');
const { WindowCounter, TokenBucket } = require('./limiter');
const { send } = require('./sessions');
const { createSaveHandler } = require('./saves');

const USERNAME_RE = /^[A-Za-z0-9_]{3,16}$/;
const TAMER_RE = /^[A-Za-z0-9]+( [A-Za-z0-9]+)*$/; // tanpa spasi di tepi / spasi ganda

function validateCredentials(m, withTamer) {
  if (typeof m.username !== 'string' || !USERNAME_RE.test(m.username)) {
    return 'Username harus 3-16 karakter: huruf, angka, atau underscore.';
  }
  if (typeof m.password !== 'string' || m.password.length < 8 || Buffer.byteLength(m.password) > 72) {
    return 'Password harus 8-72 karakter.';
  }
  if (withTamer) {
    if (typeof m.tamerName !== 'string' || m.tamerName.length < 3 || m.tamerName.length > 16 || !TAMER_RE.test(m.tamerName)) {
      return 'Nama Tamer harus 3-16 karakter (huruf, angka, spasi tunggal).';
    }
  }
  return null;
}

function createWs(sessions, store, cfg, log, udpPortRef) {
  const wss = new WebSocket.Server({ host: cfg.host, port: cfg.wsPort, maxPayload: cfg.maxWsPayload });
  const saveHandlerFor = createSaveHandler(store, cfg, log);
  const loginFails = new WindowCounter(8, 10 * 60_000);   // 8 gagal / 10 menit / IP
  const registers = new WindowCounter(5, 60 * 60_000);    // 5 akun / jam / IP
  // Hash palsu agar waktu respon "user tidak ada" sama dengan "password salah".
  const dummyHash = bcrypt.hashSync('dummy-password', cfg.bcryptCost);

  function fail(ws, code, message) { send(ws, { type: 'ERROR', code, message }); }

  function establish(ws, accountId, registered) {
    const profile = store.loadProfile(accountId);
    if (!profile) return fail(ws, 'NO_TAMER', 'Akun tidak punya Tamer.');
    if (sessions.size >= cfg.maxPlayers) return fail(ws, 'SERVER_FULL', 'Server penuh.');
    store.touchLogin(accountId);
    const s = sessions.create({ accountId, tamer: profile.tamer, ws });
    ws.session = s;
    log.info(`login: ${s.tamerName} (tamer ${s.tamerId})`);
    send(ws, {
      type: 'LOGIN_SUCCESS',
      protocol: 2,
      registered: !!registered,
      sessionId: s.sessionId,
      sessionKey: s.key.toString('hex'),
      udpPort: udpPortRef.port,
      tamer: profile.tamer,
      digimon: profile.digimon,
      worldPhasingFlags: profile.worldPhasingFlags,
      bossClears: profile.bossClears,
      inventory: profile.inventory,
    });
  }

  async function doLogin(ws, ip, m) {
    const bad = validateCredentials(m, false);
    if (bad) return fail(ws, 'INVALID_INPUT', bad);
    if (loginFails.blocked(ip)) return fail(ws, 'RATE_LIMITED', 'Terlalu banyak percobaan login. Coba lagi nanti.');
    const acc = store.getAccount(m.username);
    let ok = false;
    try { ok = await bcrypt.compare(m.password, acc ? acc.PasswordHash : dummyHash); } catch { ok = false; }
    if (!acc || !ok) {
      loginFails.record(ip);
      return fail(ws, 'INVALID_CREDENTIALS', 'Username atau password salah.');
    }
    loginFails.clear(ip);
    establish(ws, acc.AccountID, false);
  }

  async function doRegister(ws, ip, m) {
    const bad = validateCredentials(m, true);
    if (bad) return fail(ws, 'INVALID_INPUT', bad);
    if (registers.blocked(ip)) return fail(ws, 'RATE_LIMITED', 'Terlalu banyak pendaftaran dari alamat ini.');
    registers.record(ip);
    const hash = await bcrypt.hash(m.password, cfg.bcryptCost);
    try {
      const { accountId } = store.createAccount(m.username, hash, m.tamerName);
      establish(ws, accountId, true);
    } catch (err) {
      if (err instanceof DuplicateError) {
        return fail(ws, err.field === 'username' ? 'USERNAME_TAKEN' : 'TAMER_NAME_TAKEN',
          err.field === 'username' ? 'Username sudah dipakai.' : 'Nama Tamer sudah dipakai.');
      }
      log.error('register gagal:', err.message);
      fail(ws, 'INTERNAL', 'Kesalahan server.');
    }
  }

  wss.on('connection', (ws, req) => {
    const ip = req.socket.remoteAddress || 'unknown';
    ws.session = null;
    ws.isAlive = true;
    ws.busy = false;
    const budget = new TokenBucket(5, 10); // maks ~5 pesan/detik
    const saveHandler = saveHandlerFor(TokenBucket);
    ws.on('pong', () => { ws.isAlive = true; });
    const authTimer = setTimeout(() => { if (!ws.session) ws.close(4001, 'auth timeout'); }, cfg.authTimeoutMs);

    ws.on('message', async (raw, isBinary) => {
      if (isBinary) return ws.close(1003, 'binary tidak didukung');
      // Pesan besar (memory card) hanya untuk yang sudah login.
      if (!ws.session && raw.length > cfg.preAuthMaxPayload) return ws.close(1009, 'pesan terlalu besar');
      if (!budget.take()) return fail(ws, 'RATE_LIMITED', 'Terlalu cepat.');
      let m;
      try { m = JSON.parse(raw.toString('utf8')); } catch { return fail(ws, 'BAD_JSON', 'JSON tidak valid.'); }
      if (!m || typeof m !== 'object' || typeof m.type !== 'string') return fail(ws, 'BAD_MESSAGE', 'Pesan tidak valid.');

      if (m.type === 'PING') return send(ws, { type: 'PONG' });
      if (m.type.startsWith('SAVE_')) {
        if (!ws.session) return fail(ws, 'NOT_LOGGED_IN', 'Login dulu.');
        return saveHandler(ws, m);
      }
      if (m.type !== 'LOGIN' && m.type !== 'REGISTER') return fail(ws, 'UNKNOWN_TYPE', 'Tipe pesan tidak dikenal.');
      if (ws.session) return fail(ws, 'ALREADY_LOGGED_IN', 'Sudah login.');
      if (ws.busy) return fail(ws, 'BUSY', 'Permintaan sebelumnya masih diproses.');
      ws.busy = true;
      try {
        if (m.type === 'LOGIN') await doLogin(ws, ip, m); else await doRegister(ws, ip, m);
      } catch (err) {
        log.error('handler error:', err.message);
        fail(ws, 'INTERNAL', 'Kesalahan server.');
      } finally { ws.busy = false; }
    });

    ws.on('close', () => {
      clearTimeout(authTimer);
      if (ws.session) sessions.remove(ws.session, 'koneksi terputus');
    });
    ws.on('error', (err) => log.error('WS error:', err.message));
  });

  const heartbeat = setInterval(() => {
    for (const ws of wss.clients) {
      if (!ws.isAlive) { ws.terminate(); continue; }
      ws.isAlive = false;
      ws.ping();
    }
  }, 15_000);
  heartbeat.unref();

  return {
    wss,
    listening: () => new Promise((resolve) => (wss.address() ? resolve(wss.address().port) : wss.once('listening', () => resolve(wss.address().port)))),
    close: () => new Promise((resolve) => {
      clearInterval(heartbeat); loginFails.close(); registers.close();
      for (const c of wss.clients) c.terminate();
      wss.close(() => resolve());
    }),
  };
}

module.exports = { createWs };
