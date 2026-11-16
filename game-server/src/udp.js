'use strict';
const dgram = require('dgram');
const P = require('./protocol');

// Urutan pemeriksaan sengaja dari yang paling murah ke yang paling mahal,
// tetapi rate limit SESUDAH MAC: sessionId bukan rahasia (terkirim polos),
// jadi tanpa MAC penyerang bisa menghabiskan kuota milik korban.
function createUdp(sessions, cfg, log) {
  const sock = dgram.createSocket('udp4');
  const stats = { accepted: 0, badLen: 0, noSession: 0, badMac: 0, replay: 0, rateLimited: 0, badMap: 0 };

  sock.on('message', (msg, rinfo) => {
    if (msg.length !== P.UDP_IN_LEN || msg[0] !== P.UDP_IN_TYPE) { stats.badLen++; return; }
    const s = sessions.get(msg.readUInt32LE(28));
    if (!s) { stats.noSession++; return; }
    if (!P.verifyMac(s.key, msg)) { stats.badMac++; return; }
    const sync = P.parseSync(msg);
    if (!P.isNewer(sync.seq, s.lastSeq)) { stats.replay++; return; }
    if (!s.bucket.take()) { stats.rateLimited++; return; }
    if (sync.mapId === P.MAP_NONE) { stats.badMap++; return; }

    sessions.applySync(s, sync, rinfo);
    stats.accepted++;

    // Phasing: hanya pemain di Map ID yang sama yang menerima siaran.
    const out = P.buildRelay(msg, s.tamerId);
    for (const peer of sessions.peersOnMap(s)) {
      if (peer.addr) sock.send(out, peer.port, peer.addr);
    }
  });

  sock.on('error', (err) => log.error('UDP error:', err.message));

  return {
    stats,
    bind: () => new Promise((resolve, reject) => {
      sock.once('error', reject);
      sock.bind(cfg.udpPort, cfg.host, () => { sock.off('error', reject); resolve(sock.address().port); });
    }),
    close: () => new Promise((resolve) => { try { sock.close(resolve); } catch { resolve(); } }),
  };
}

module.exports = { createUdp };
