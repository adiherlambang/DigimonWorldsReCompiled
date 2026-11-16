'use strict';
const crypto = require('crypto');
const { TokenBucket } = require('./limiter');

function send(ws, obj) {
  if (ws && ws.readyState === 1) ws.send(JSON.stringify(obj));
}

// Satu Session = satu pemain yang sudah login lewat WebSocket.
// Hanya SessionManager yang boleh mengubah daftar sesi dan keanggotaan map.
class SessionManager {
  constructor(store, cfg, log) {
    this.store = store;
    this.cfg = cfg;
    this.log = log;
    this.byId = new Map();      // sessionId -> session
    this.byTamer = new Map();   // tamerId   -> session
    this.maps = new Map();      // mapId     -> Set<session>  (instance/phasing)
  }

  get size() { return this.byId.size; }
  get(sessionId) { return this.byId.get(sessionId); }

  create({ accountId, tamer, ws }) {
    if (this.byTamer.has(tamer.TamerID)) this.remove(this.byTamer.get(tamer.TamerID), 'login dari tempat lain');
    let sessionId;
    do { sessionId = crypto.randomInt(1, 0xffffffff); } while (this.byId.has(sessionId));
    const s = {
      sessionId,
      key: crypto.randomBytes(32),
      accountId,
      tamerId: tamer.TamerID,
      tamerName: tamer.TamerName,
      ws,
      addr: null, port: null,
      lastSeq: null,
      mapId: null,
      species: 0,
      pos: null,          // {x,y,z} terakhir yang tervalidasi
      dirty: false,
      lastUdp: 0,
      bucket: new TokenBucket(40, 60),
    };
    this.byId.set(sessionId, s);
    this.byTamer.set(s.tamerId, s);
    return s;
  }

  // Dipanggil UDP handler untuk paket yang sudah lolos MAC + seq + rate limit.
  applySync(s, sync, rinfo) {
    s.addr = rinfo.address;
    s.port = rinfo.port;
    s.lastSeq = sync.seq;
    s.lastUdp = Date.now();
    s.species = sync.species;
    s.pos = { x: sync.x, y: sync.y, z: sync.z };
    s.dirty = true;
    if (s.mapId !== sync.mapId) this.changeMap(s, sync.mapId);
  }

  peersOnMap(s) {
    const set = this.maps.get(s.mapId);
    return set ? [...set].filter((p) => p !== s) : [];
  }

  changeMap(s, newMapId) {
    this.leaveMap(s);
    s.mapId = newMapId;
    if (!this.maps.has(newMapId)) this.maps.set(newMapId, new Set());
    const set = this.maps.get(newMapId);
    const peers = [...set];
    set.add(s);
    const info = (p) => ({ tamerId: p.tamerId, tamerName: p.tamerName, species: p.species });
    send(s.ws, { type: 'MAP_ROSTER', mapId: newMapId, players: peers.map(info) });
    for (const p of peers) send(p.ws, { type: 'PLAYER_JOIN', mapId: newMapId, ...info(s) });
    this.persist(s);
  }

  leaveMap(s) {
    if (s.mapId === null) return;
    const set = this.maps.get(s.mapId);
    if (set) {
      set.delete(s);
      for (const p of set) send(p.ws, { type: 'PLAYER_LEAVE', mapId: s.mapId, tamerId: s.tamerId });
      if (set.size === 0) this.maps.delete(s.mapId);
    }
    s.mapId = null;
  }

  persist(s) {
    if (!s.pos || s.mapId === null) return;
    try {
      this.store.savePosition(s.tamerId, s.mapId, s.pos.x, s.pos.y, s.pos.z);
      s.dirty = false;
    } catch (err) {
      this.log.error('gagal simpan posisi', s.tamerName, err.message);
    }
  }

  remove(s, reason) {
    if (!this.byId.has(s.sessionId)) return;
    this.persist(s);
    this.leaveMap(s);
    this.byId.delete(s.sessionId);
    if (this.byTamer.get(s.tamerId) === s) this.byTamer.delete(s.tamerId);
    this.log.info(`sesi berakhir: ${s.tamerName} (${reason})`);
    if (s.ws && s.ws.readyState === 1) {
      send(s.ws, { type: 'SESSION_END', reason });
      s.ws.close(4000, reason);
    }
  }

  // Pemain yang berhenti mengirim UDP dikeluarkan dari map-nya (sesi WS tetap hidup).
  sweepStale(now = Date.now()) {
    for (const s of this.byId.values()) {
      if (s.mapId !== null && now - s.lastUdp > this.cfg.udpStaleMs) this.leaveMap(s);
    }
  }

  persistDirty() {
    for (const s of this.byId.values()) if (s.dirty) this.persist(s);
  }

  removeAll(reason) { for (const s of [...this.byId.values()]) this.remove(s, reason); }
}

module.exports = { SessionManager, send };
