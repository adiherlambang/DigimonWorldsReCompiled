# DW1 MMO — Protokol Klien ⇄ Server (v2)

Server: `game-server/` (Node ≥ 22.5, `node:sqlite`). Jalankan: `cd game-server && npm install && npm start`.
Konfigurasi: `game-server/config.json` atau env `DW1_HOST`, `DW1_WS_PORT`, `DW1_UDP_PORT`, `DW1_DB_PATH`.

## 1. WebSocket (`:8080`) — otoritatif, JSON teks

Satu koneksi = satu sesi. Sesi berakhir saat koneksi putus. Harus login dalam 10 detik.

### Klien → Server
| type | field | catatan |
|---|---|---|
| `REGISTER` | `username` (3-16 `[A-Za-z0-9_]`), `password` (8-72 byte), `tamerName` (3-16, huruf/angka/spasi tunggal) | membuat akun + Tamer + Agumon awal, lalu otomatis login |
| `LOGIN` | `username`, `password` | username tidak peka huruf besar/kecil |
| `PING` | – | dibalas `PONG` |

### Server → Klien
- `LOGIN_SUCCESS`: `{ protocol:2, registered, sessionId, sessionKey (hex 32 byte), udpPort, tamer{TamerID,TamerName,Bits,ProsperityPoints,MentorPoints,CurrentMapID,PosX,PosY,PosZ}, digimon, worldPhasingFlags[], bossClears[], inventory[] }`
- `ERROR`: `{ code, message }` — `INVALID_INPUT`, `INVALID_CREDENTIALS`, `USERNAME_TAKEN`, `TAMER_NAME_TAKEN`, `RATE_LIMITED`, `SERVER_FULL`, `ALREADY_LOGGED_IN`, `BUSY`, `BAD_JSON`, `UNKNOWN_TYPE`, `INTERNAL`
- `MAP_ROSTER` `{mapId, players[{tamerId,tamerName,species}]}` — dikirim ke pemain yang baru masuk sebuah map
- `PLAYER_JOIN` `{mapId, tamerId, tamerName, species}` / `PLAYER_LEAVE` `{mapId, tamerId}` — ke pemain lain di map itu
- `SESSION_END` `{reason}` — mis. login dari tempat lain

Batas: 5 pesan/detik, payload ≤ 4 KB, 8 login gagal / 10 menit / IP, 5 pendaftaran / jam / IP.

## 2. UDP (`:7777`) — fast-sync posisi

Klien → Server, 44 byte little-endian:

| offset | tipe | isi |
|---|---|---|
| 0 | u8 | `0x02` |
| 1 | u8 | mapId (`0xFF` ditolak) |
| 2 | u16 | species partner |
| 4 / 8 / 12 | i32 | tamer X / Y / Z |
| 16 | i16 | yaw |
| 18 / 19 | u8 | anim tamer / anim partner |
| 20 / 24 | i32 | partner X / Z |
| 28 | u32 | `sessionId` |
| 32 | u32 | `seq` — harus naik terus (aritmetika serial mod 2³²). Disarankan: jam milidetik `u32`, supaya aman saat emulator di-restart di sesi yang sama |
| 36 | 8 B | `MAC = HMAC-SHA256(sessionKey, bytes[0..36])[0..8]` |

Server → Klien, 32 byte, hanya untuk pemain di **map yang sama**, tidak pernah dikembalikan ke pengirim: layout byte 0..28 sama, `type = 0x03`, dan byte 28..32 = `tamerId` pengirim (diisi server).

Paket ditolak diam-diam bila: panjang/type salah, sessionId tidak dikenal, MAC salah, `seq` tidak lebih baru, rate > 40 paket/detik (burst 60), atau mapId `0xFF`.
Pemain yang tidak mengirim UDP selama 10 detik dikeluarkan dari map-nya (sesi WS tetap hidup).

## 3. Catatan keamanan

- Password di-hash bcrypt (cost 12). Klien tidak pernah memilih `tamerId`; server menentukannya dari akun.
- `sessionKey` dan password lewat WebSocket **tanpa enkripsi**. Untuk server publik, pasang reverse proxy TLS (nginx/Caddy) di depan `:8080` dan pakai `wss://`.
- UDP diautentikasi (MAC) tetapi tidak dienkripsi: posisi pemain terlihat oleh penyadap, namun tidak bisa dipalsukan.
- Bits / Prosperity / Mentor Points hanya diubah server; klien belum punya jalur untuk menulisnya.

## 4. Implementasi klien (Rust, crate `dwrc-net`)

- `client::NetClient::{login, register}` — membuka WebSocket (`tungstenite`) dan **mempertahankannya** di thread `mmo-ws`.
  Server mengakhiri sesi begitu koneksi putus, jadi `NetClient` harus tetap hidup selama online (disimpan di `App.mmo_net`).
  Meng-drop `NetClient` = logout.
- `state::NetShared` — state bersama (peers dari event WS, posisi pemain lain dari UDP, alasan sesi berakhir).
- `proto` — `pack_sync` (paket 44 byte + MAC), `parse_relay`, `SeqGen`, `drain_relays`.
- Thread emulasi (`apps/dwrc/src/emu.rs`) membaca RAM tiap 3 frame, mengirim UDP v2, dan menguras relay masuk.
- Minimap HUD menggambar pemain lain (hijau). Pemain lain **belum dirender di dalam game** (tahap berikutnya).

Tes: `cargo test -p dwrc-net` (unit, termasuk vektor paket yang dicocokkan byte-per-byte dengan `game-server/src/protocol.js`) dan
`cargo test -p dwrc-net --test live -- --ignored` (end-to-end melawan server yang sedang berjalan).
