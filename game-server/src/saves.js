'use strict';
const crypto = require('crypto');
const { CARD_SIZE, inspect } = require('./memcard');
const { send } = require('./sessions');

// Protokol (setelah login, semua balasan membawa kembali `reqId` dari permintaan):
//   SAVE_LIST {reqId}                         -> SAVE_LIST_RESULT {saves:[{slot,version,sha256,usedBlocks,hasGameSave,uploadedAt}]}
//   SAVE_GET  {reqId, slot}                   -> SAVE_DATA {slot,version,sha256,data(base64)} | SAVE_REJECTED NO_SAVE
//   SAVE_PUT  {reqId, slot, baseVersion,
//              sha256, data(base64)}          -> SAVE_OK {slot,version,sha256,unchanged} | SAVE_REJECTED {code,...}
// Kode penolakan: BAD_REQUEST, BAD_SIZE, BAD_FORMAT, BAD_HASH, CONFLICT, EMPTY_OVERWRITE, NO_SAVE, RATE_LIMITED, INTERNAL
// Server tidak pernah menimpa versi yang lebih baru: CONFLICT berarti klien harus menarik versi server dulu.

const B64_RE = /^[A-Za-z0-9+/]+={0,2}$/;

function createSaveHandler(store, cfg, log) {
  function reject(ws, m, code, message, extra = {}) {
    send(ws, { type: 'SAVE_REJECTED', reqId: m.reqId ?? null, slot: m.slot ?? null, code, message, ...extra });
  }

  const validSlot = (s) => s === 1 || s === 2;

  function onList(ws, m) {
    const rows = store.listSaves(ws.session.tamerId);
    send(ws, {
      type: 'SAVE_LIST_RESULT',
      reqId: m.reqId ?? null,
      saves: rows.map((r) => ({
        slot: r.Slot, version: r.Version, sha256: r.Sha256,
        usedBlocks: r.UsedBlocks, hasGameSave: !!r.HasGameSave, uploadedAt: r.UploadedAt,
      })),
    });
  }

  function onGet(ws, m) {
    if (!validSlot(m.slot)) return reject(ws, m, 'BAD_REQUEST', 'Slot harus 1 atau 2.');
    const row = store.getSave(ws.session.tamerId, m.slot);
    if (!row) return reject(ws, m, 'NO_SAVE', 'Belum ada save di server.');
    send(ws, {
      type: 'SAVE_DATA', reqId: m.reqId ?? null, slot: m.slot, version: row.Version,
      sha256: row.Sha256, data: Buffer.from(row.Data).toString('base64'),
    });
  }

  function onPut(ws, m, budget) {
    if (!validSlot(m.slot)) return reject(ws, m, 'BAD_REQUEST', 'Slot harus 1 atau 2.');
    if (!Number.isInteger(m.baseVersion) || m.baseVersion < 0) return reject(ws, m, 'BAD_REQUEST', 'baseVersion tidak valid.');
    if (typeof m.data !== 'string' || typeof m.sha256 !== 'string') return reject(ws, m, 'BAD_REQUEST', 'data / sha256 hilang.');
    if (!budget.take()) return reject(ws, m, 'RATE_LIMITED', 'Terlalu sering mengunggah save.');
    if (m.data.length > Math.ceil(CARD_SIZE / 3) * 4 + 4 || !B64_RE.test(m.data)) return reject(ws, m, 'BAD_SIZE', 'Data bukan base64 memory card.');
    const data = Buffer.from(m.data, 'base64');
    const info = inspect(data);
    if (!info.ok) return reject(ws, m, info.code, info.message);
    const sha = crypto.createHash('sha256').update(data).digest('hex');
    if (sha !== m.sha256.toLowerCase()) return reject(ws, m, 'BAD_HASH', 'sha256 tidak cocok dengan isi data.');

    const res = store.putSave(ws.session.tamerId, m.slot, m.baseVersion, data, sha, info, cfg.saveVersionsKept);
    if (!res.ok) {
      return reject(ws, m, res.code,
        res.code === 'CONFLICT' ? 'Server punya versi yang lebih baru. Tarik dulu.' : 'Save kosong tidak boleh menimpa save yang berisi.',
        { version: res.current.version, sha256: res.current.sha256 });
    }
    if (!res.unchanged) log.info(`save: ${ws.session.tamerName} slot ${m.slot} -> v${res.version} (${info.usedBlocks} blok)`);
    send(ws, { type: 'SAVE_OK', reqId: m.reqId ?? null, slot: m.slot, version: res.version, sha256: sha, unchanged: res.unchanged });
  }

  // Satu handler per koneksi supaya batas unggah bersifat per sesi.
  return function forConnection(TokenBucket) {
    const putBudget = new TokenBucket(0.2, 4); // ~1 unggahan / 5 detik, burst 4
    return (ws, m) => {
      try {
        if (m.type === 'SAVE_LIST') return onList(ws, m);
        if (m.type === 'SAVE_GET') return onGet(ws, m);
        if (m.type === 'SAVE_PUT') return onPut(ws, m, putBudget);
        return reject(ws, m, 'BAD_REQUEST', 'Perintah save tidak dikenal.');
      } catch (err) {
        log.error('save handler error:', err.message);
        reject(ws, m, 'INTERNAL', 'Kesalahan server.');
      }
    };
  };
}

module.exports = { createSaveHandler };
