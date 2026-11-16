'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const dgram = require('node:dgram');
const os = require('node:os');
const fs = require('node:fs');
const path = require('node:path');
const WebSocket = require('ws');
const { startServer } = require('../src/server');
const P = require('../src/protocol');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

class Client {
  constructor(port) {
    this.ws = new WebSocket(`ws://127.0.0.1:${port}`);
    this.inbox = [];
    this.waiters = [];
    this.ws.on('message', (raw) => {
      const m = JSON.parse(raw.toString());
      const i = this.waiters.findIndex((w) => w.type === m.type);
      if (i >= 0) this.waiters.splice(i, 1)[0].resolve(m); else this.inbox.push(m);
    });
    this.opened = new Promise((res) => this.ws.once('open', res));
    this.udp = dgram.createSocket('udp4');
    this.udpIn = [];
    this.udp.on('message', (b) => this.udpIn.push(b));
    this.seq = 1000;
  }
  send(o) { this.ws.send(JSON.stringify(o)); }
  next(type, ms = 3000) {
    const i = this.inbox.findIndex((m) => m.type === type);
    if (i >= 0) return Promise.resolve(this.inbox.splice(i, 1)[0]);
    return new Promise((resolve, reject) => {
      const w = { type, resolve };
      this.waiters.push(w);
      setTimeout(() => { this.waiters = this.waiters.filter((x) => x !== w); reject(new Error(`timeout menunggu ${type}`)); }, ms);
    });
  }
  async auth(type, username, password, tamerName) {
    await this.opened;
    this.send({ type, username, password, tamerName });
    const m = await Promise.race([this.next('LOGIN_SUCCESS'), this.next('ERROR')]);
    if (m.type === 'LOGIN_SUCCESS') { this.sid = m.sessionId; this.key = Buffer.from(m.sessionKey, 'hex'); this.udpPort = m.udpPort; }
    return m;
  }
  sync(fields, opts = {}) {
    const seq = opts.seq ?? ++this.seq;
    const pkt = P.buildClientPacket(opts.sid ?? this.sid, opts.key ?? this.key, seq, fields);
    this.udp.send(opts.mutate ? opts.mutate(pkt) : pkt, this.udpPort, '127.0.0.1');
  }
  close() { try { this.ws.close(); } catch {} try { this.udp.close(); } catch {} }
}

async function withServer(fn, overrides = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dw1-'));
  const srv = await startServer({ wsPort: 0, udpPort: 0, host: '127.0.0.1', dbPath: path.join(dir, 't.sqlite'), bcryptCost: 4, quiet: true, ...overrides });
  const clients = [];
  const mk = () => { const c = new Client(srv.wsPort); clients.push(c); return c; };
  try { await fn(srv, mk); } finally {
    clients.forEach((c) => c.close());
    await srv.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

test('register lalu login; password disimpan sebagai bcrypt hash', () => withServer(async (srv, mk) => {
  const a = mk();
  const r = await a.auth('REGISTER', 'alice_01', 'secret123', 'Alice');
  assert.equal(r.type, 'LOGIN_SUCCESS');
  assert.equal(r.registered, true);
  assert.equal(r.tamer.TamerName, 'Alice');
  assert.equal(r.tamer.Bits, 500);
  assert.equal(r.digimon.Nickname, 'Agumon');
  assert.ok(!('PasswordHash' in r.tamer) && !JSON.stringify(r).includes('$2'));
  const row = srv.db.prepare('SELECT PasswordHash FROM Accounts WHERE Username=?').get('alice_01');
  assert.match(row.PasswordHash, /^\$2[aby]\$/);
  a.close(); await sleep(100);

  const b = mk();
  const r2 = await b.auth('LOGIN', 'ALICE_01', 'secret123');     // username case-insensitive
  assert.equal(r2.type, 'LOGIN_SUCCESS');
  assert.equal(r2.registered, false);
}));

test('password salah & user tidak ada -> error yang sama', () => withServer(async (srv, mk) => {
  const a = mk();
  await a.auth('REGISTER', 'bob_01', 'secret123', 'Bob');
  const w = mk(); const e1 = await w.auth('LOGIN', 'bob_01', 'WRONGpass1');
  const u = mk(); const e2 = await u.auth('LOGIN', 'ghost_99', 'WRONGpass1');
  assert.equal(e1.code, 'INVALID_CREDENTIALS');
  assert.deepEqual(e1, e2);
}));

test('tamerId yang dikirim klien diabaikan (bug prototipe lama)', () => withServer(async (srv, mk) => {
  const v = mk(); await v.auth('REGISTER', 'victim_1', 'secret123', 'Victim');
  const a = mk(); await a.opened;
  a.send({ type: 'LOGIN', username: 'victim_1', password: 'nope-nope', tamerId: 1 });
  assert.equal((await a.next('ERROR')).code, 'INVALID_CREDENTIALS');
}));

test('validasi input & duplikat', () => withServer(async (srv, mk) => {
  const a = mk(); await a.auth('REGISTER', 'carl_01', 'secret123', 'Carl');
  assert.equal((await mk().auth('REGISTER', 'a', 'secret123', 'Xyz')).code, 'INVALID_INPUT');
  assert.equal((await mk().auth('REGISTER', 'dave_01', 'short', 'Dave')).code, 'INVALID_INPUT');
  assert.equal((await mk().auth('REGISTER', 'dave_01', 'secret123', "x'; DROP--")).code, 'INVALID_INPUT');
  assert.equal((await mk().auth('REGISTER', 'CARL_01', 'secret123', 'Other')).code, 'USERNAME_TAKEN');
  assert.equal((await mk().auth('REGISTER', 'erin_01', 'secret123', 'carl')).code, 'TAMER_NAME_TAKEN');
  // password berisi karakter yang merusak JSON buatan-tangan di klien lama
  const q = mk(); const r = await q.auth('REGISTER', 'quote_01', 'pa"ss\\word99', 'Quote');
  assert.equal(r.type, 'LOGIN_SUCCESS');
}));

test('brute-force login diblokir per IP', () => withServer(async (srv, mk) => {
  const a = mk(); await a.auth('REGISTER', 'frank_01', 'secret123', 'Frank');
  let last;
  for (let i = 0; i < 9; i++) { const c = mk(); last = await c.auth('LOGIN', 'frank_01', `wrongpass${i}`); }
  assert.equal(last.code, 'RATE_LIMITED');
  const ok = mk(); assert.equal((await ok.auth('LOGIN', 'frank_01', 'secret123')).code, 'RATE_LIMITED'); // password benar pun ditolak saat blokir
}));

test('UDP: phasing per map, tidak echo ke pengirim, id pengirim dari server', () => withServer(async (srv, mk) => {
  const a = mk(), b = mk(), c = mk();
  const ra = await a.auth('REGISTER', 'ann_01', 'secret123', 'Ann');
  await b.auth('REGISTER', 'ben_01', 'secret123', 'Ben');
  await c.auth('REGISTER', 'cat_01', 'secret123', 'Cat');
  for (const x of [a, b, c]) x.udp.bind(0, '127.0.0.1');
  await sleep(50);

  a.sync({ mapId: 0xda, species: 3, x: 100, y: 5, z: 200 });
  await sleep(50);
  b.sync({ mapId: 0xda, species: 7, x: 111, y: 6, z: 222, yaw: -300, anim: 4, dAnim: 2, dx: 9, dz: 8 });
  c.sync({ mapId: 0xcc, species: 5, x: 1, y: 1, z: 1 });
  await sleep(150);

  assert.equal(a.udpIn.length, 1, 'A (map 0xDA) menerima B');
  const pkt = a.udpIn[0];
  assert.equal(pkt.length, 32);
  assert.equal(pkt[0], P.UDP_OUT_TYPE);
  assert.equal(pkt[1], 0xda);
  assert.equal(pkt.readInt32LE(4), 111);
  assert.equal(pkt.readInt16LE(16), -300);
  assert.equal(pkt.readUInt32LE(28), srv.sessions.get(b.sid).tamerId, 'id pengirim = tamerId dari server');
  a.sync({ mapId: 0xda, species: 3, x: 101, y: 5, z: 200 });        // A kirim lagi, sekarang B sudah di map
  await sleep(100);
  assert.equal(b.udpIn.length, 1, 'B menerima A');
  assert.equal(a.udpIn.length, 1, 'A tidak menerima echo miliknya sendiri');
  assert.equal(c.udpIn.length, 0, 'C di File City tidak menerima apa pun');
  assert.equal(srv.udp.stats.accepted, 4);
  assert.ok(ra.sessionId > 0);
}));

test('UDP: MAC palsu, sessionId salah, replay, panjang salah, map 0xFF ditolak', () => withServer(async (srv, mk) => {
  const a = mk(), b = mk();
  await a.auth('REGISTER', 'ann_01', 'secret123', 'Ann');
  await b.auth('REGISTER', 'ben_01', 'secret123', 'Ben');
  a.udp.bind(0, '127.0.0.1'); b.udp.bind(0, '127.0.0.1'); await sleep(50);
  a.sync({ mapId: 1, x: 1 }); await sleep(50);                      // A terdaftar di map 1
  const base = { mapId: 1, species: 3, x: 42 };

  b.sync(base, { key: Buffer.alloc(32, 7) });                        // key salah (penyerang)
  b.sync(base, { sid: a.sid });                                      // pakai sid korban, key milik B
  b.sync(base, { mutate: (p) => p.subarray(0, 43) });                // panjang salah
  b.sync({ ...base, mapId: 0xff });                                  // map invalid
  b.sync(base, { seq: 5000 }); await sleep(60);                      // sah
  b.sync(base, { seq: 5000 }); b.sync(base, { seq: 4999 });          // replay & lama
  await sleep(100);

  assert.equal(a.udpIn.length, 1, 'hanya satu paket sah yang sampai');
  const s = srv.udp.stats;
  assert.equal(s.badMac, 2); assert.equal(s.badLen, 1); assert.equal(s.badMap, 1); assert.equal(s.replay, 2);
  // paket yang dimodifikasi di tengah jalan (tamper) -> MAC gagal
  b.sync(base, { mutate: (p) => { p.writeInt32LE(999999, 4); return p; }, seq: 6000 });
  await sleep(60); assert.equal(a.udpIn.length, 1);
}));

test('UDP: rate limit per sesi', () => withServer(async (srv, mk) => {
  const a = mk(); await a.auth('REGISTER', 'ann_01', 'secret123', 'Ann');
  a.udp.bind(0, '127.0.0.1'); await sleep(30);
  for (let i = 0; i < 200; i++) a.sync({ mapId: 1, x: i });
  await sleep(200);
  assert.ok(srv.udp.stats.accepted <= 70, `accepted=${srv.udp.stats.accepted}`);
  assert.ok(srv.udp.stats.rateLimited > 100);
}));

test('WS: ROSTER / JOIN / LEAVE saat pindah map dan disconnect', () => withServer(async (srv, mk) => {
  const a = mk(), b = mk();
  await a.auth('REGISTER', 'ann_01', 'secret123', 'Ann');
  await b.auth('REGISTER', 'ben_01', 'secret123', 'Ben');
  a.udp.bind(0, '127.0.0.1'); b.udp.bind(0, '127.0.0.1'); await sleep(30);

  a.sync({ mapId: 0xda, species: 3 });
  const r1 = await a.next('MAP_ROSTER'); assert.deepEqual(r1.players, []);
  b.sync({ mapId: 0xda, species: 9 });
  const j = await a.next('PLAYER_JOIN'); assert.equal(j.tamerName, 'Ben'); assert.equal(j.species, 9);
  const r2 = await b.next('MAP_ROSTER'); assert.equal(r2.players.length, 1); assert.equal(r2.players[0].tamerName, 'Ann');

  b.sync({ mapId: 0xcc, species: 9 });                              // B pindah map
  const l = await a.next('PLAYER_LEAVE'); assert.equal(l.tamerId, srv.sessions.get(b.sid).tamerId);
  b.sync({ mapId: 0xda, species: 9 }); await a.next('PLAYER_JOIN');
  b.ws.close();
  await a.next('PLAYER_LEAVE');
}));

test('login kedua dari akun yang sama menendang sesi lama', () => withServer(async (srv, mk) => {
  const a = mk(); await a.auth('REGISTER', 'gina_01', 'secret123', 'Gina');
  const old = a.sid;
  const b = mk(); const r = await b.auth('LOGIN', 'gina_01', 'secret123');
  assert.equal(r.type, 'LOGIN_SUCCESS');
  assert.equal((await a.next('SESSION_END')).type, 'SESSION_END');
  assert.equal(srv.sessions.get(old), undefined);
  assert.equal(srv.sessions.size, 1);
}));

test('posisi & map tersimpan saat disconnect, dimuat saat login berikutnya', () => withServer(async (srv, mk) => {
  const a = mk(); await a.auth('REGISTER', 'hal_01', 'secret123', 'Hal');
  a.udp.bind(0, '127.0.0.1'); await sleep(30);
  a.sync({ mapId: 0xcc, x: 1234, y: -5, z: 777 }); await sleep(80);
  a.ws.close(); await sleep(150);
  const b = mk(); const r = await b.auth('LOGIN', 'hal_01', 'secret123');
  assert.equal(r.tamer.CurrentMapID, 0xcc);
  assert.deepEqual([r.tamer.PosX, r.tamer.PosY, r.tamer.PosZ], [1234, -5, 777]);
}));

test('pemain yang berhenti kirim UDP dikeluarkan dari map (stale)', () => withServer(async (srv, mk) => {
  const a = mk(), b = mk();
  await a.auth('REGISTER', 'ann_01', 'secret123', 'Ann'); await b.auth('REGISTER', 'ben_01', 'secret123', 'Ben');
  a.udp.bind(0, '127.0.0.1'); b.udp.bind(0, '127.0.0.1'); await sleep(30);
  a.sync({ mapId: 1 }); b.sync({ mapId: 1 }); await a.next('PLAYER_JOIN');
  await sleep(60);
  srv.sessions.get(b.sid).lastUdp = 0;                               // hanya B yang "diam"
  srv.sessions.sweepStale();
  await a.next('PLAYER_LEAVE');
  assert.equal(srv.sessions.get(b.sid).mapId, null);
  assert.equal(srv.sessions.maps.get(1).size, 1);
}));

test('pesan WS sebelum login: JSON rusak, tipe asing, biner', () => withServer(async (srv, mk) => {
  const a = mk(); await a.opened;
  a.ws.send('bukan json'); assert.equal((await a.next('ERROR')).code, 'BAD_JSON');
  a.send({ type: 'HACK' }); assert.equal((await a.next('ERROR')).code, 'UNKNOWN_TYPE');
  a.send({ type: 'PING' }); assert.equal((await a.next('PONG')).type, 'PONG');
}));
