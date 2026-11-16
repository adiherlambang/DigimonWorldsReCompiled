'use strict';

// Pemeriksaan memory card PS1 mentah (.mcd): 1024 sektor x 128 byte = 131072 byte.
//   frame 0        header: "MC", byte 127 = XOR byte 0..126
//   frame 1..15    direktori; byte 0 = status blok (0x51/0x52/0x53 terpakai, 0xA0 bebas)
//                  byte 0x0A..0x1D = nama file
// Checksum frame direktori sengaja tidak dipaksakan: yang ditolak hanya kartu yang
// jelas bukan memory card, bukan kartu yang ditulis dengan cara berbeda.

const CARD_SIZE = 128 * 1024;
const FRAME = 128;
const GAME_PREFIX = 'BASLUS-01032'; // Digimon World (USA), SLUS-01032

function xor127(buf, off) {
  let x = 0;
  for (let i = 0; i < 127; i++) x ^= buf[off + i];
  return x;
}

// -> { ok: true, usedBlocks, hasGameSave } | { ok: false, code, message }
function inspect(buf) {
  if (!Buffer.isBuffer(buf) || buf.length !== CARD_SIZE) {
    return { ok: false, code: 'BAD_SIZE', message: `Memory card harus ${CARD_SIZE} byte.` };
  }
  if (buf[0] !== 0x4d || buf[1] !== 0x43 || xor127(buf, 0) !== buf[127]) {
    return { ok: false, code: 'BAD_FORMAT', message: 'Header memory card tidak valid.' };
  }
  let usedBlocks = 0;
  let hasGameSave = false;
  for (let i = 1; i < 16; i++) {
    const off = i * FRAME;
    const state = buf[off];
    if (state >= 0x51 && state <= 0x53) usedBlocks++;
    if (state === 0x51) {
      const name = buf.toString('latin1', off + 0x0a, off + 0x1e).replace(/\0.*$/s, '');
      if (name.startsWith(GAME_PREFIX)) hasGameSave = true;
    }
  }
  return { ok: true, usedBlocks, hasGameSave };
}

module.exports = { CARD_SIZE, GAME_PREFIX, inspect };
