'use strict';
const crypto = require('crypto');

// ---------------------------------------------------------------------------
// UDP protocol v2
//
// Klien -> Server (44 byte, little-endian):
//   0      u8   type = 0x02
//   1      u8   mapId (0xFF = tidak valid / menu)
//   2..4   u16  partner species
//   4..8   i32  tamer X
//   8..12  i32  tamer Y
//   12..16 i32  tamer Z
//   16..18 i16  tamer yaw
//   18     u8   tamer anim
//   19     u8   partner anim
//   20..24 i32  partner X
//   24..28 i32  partner Z
//   28..32 u32  sessionId  (dari LOGIN_SUCCESS)
//   32..36 u32  seq        (naik terus; aritmetika serial mod 2^32)
//   36..44 8B   MAC = HMAC-SHA256(sessionKey, bytes[0..36])[0..8]
//
// Server -> Klien (32 byte), disiarkan ke pemain lain di map yang sama:
//   layout byte 0..28 identik dengan di atas, kecuali:
//   0      u8   type = 0x03
//   28..32 u32  tamerId pengirim (ditentukan SERVER, bukan klien)
// ---------------------------------------------------------------------------

const UDP_IN_TYPE = 0x02;
const UDP_IN_LEN = 44;
const UDP_OUT_TYPE = 0x03;
const UDP_OUT_LEN = 32;
const MAC_LEN = 8;
const MAP_NONE = 0xff;

function computeMac(key, buf) {
  return crypto.createHmac('sha256', key).update(buf.subarray(0, 36)).digest().subarray(0, MAC_LEN);
}

function verifyMac(key, buf) {
  const expected = computeMac(key, buf);
  return crypto.timingSafeEqual(expected, buf.subarray(36, 36 + MAC_LEN));
}

// Aritmetika serial (RFC 1982): seq dianggap lebih baru bila selisihnya 1..2^31-1.
function isNewer(seq, last) {
  if (last === null) return true;
  const d = (seq - last) >>> 0;
  return d !== 0 && d < 0x80000000;
}

function parseSync(buf) {
  return {
    mapId: buf.readUInt8(1),
    species: buf.readUInt16LE(2),
    x: buf.readInt32LE(4),
    y: buf.readInt32LE(8),
    z: buf.readInt32LE(12),
    yaw: buf.readInt16LE(16),
    anim: buf.readUInt8(18),
    dAnim: buf.readUInt8(19),
    dx: buf.readInt32LE(20),
    dz: buf.readInt32LE(24),
    sessionId: buf.readUInt32LE(28),
    seq: buf.readUInt32LE(32),
  };
}

function buildRelay(inbound, tamerId) {
  const out = Buffer.alloc(UDP_OUT_LEN);
  out[0] = UDP_OUT_TYPE;
  inbound.copy(out, 1, 1, 28);
  out.writeUInt32LE(tamerId >>> 0, 28);
  return out;
}

// Dipakai test (dan sebagai referensi implementasi klien).
function buildClientPacket(sessionId, key, seq, f) {
  const b = Buffer.alloc(UDP_IN_LEN);
  b[0] = UDP_IN_TYPE;
  b.writeUInt8(f.mapId & 0xff, 1);
  b.writeUInt16LE(f.species || 0, 2);
  b.writeInt32LE(f.x || 0, 4);
  b.writeInt32LE(f.y || 0, 8);
  b.writeInt32LE(f.z || 0, 12);
  b.writeInt16LE(f.yaw || 0, 16);
  b.writeUInt8(f.anim || 0, 18);
  b.writeUInt8(f.dAnim || 0, 19);
  b.writeInt32LE(f.dx || 0, 20);
  b.writeInt32LE(f.dz || 0, 24);
  b.writeUInt32LE(sessionId >>> 0, 28);
  b.writeUInt32LE(seq >>> 0, 32);
  computeMac(key, b).copy(b, 36);
  return b;
}

module.exports = {
  UDP_IN_TYPE, UDP_IN_LEN, UDP_OUT_TYPE, UDP_OUT_LEN, MAC_LEN, MAP_NONE,
  computeMac, verifyMac, isNewer, parseSync, buildRelay, buildClientPacket,
};
