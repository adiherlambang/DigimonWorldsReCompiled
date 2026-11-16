# Save online (memory card di server)

Game berjalan di komputer pemain dan menyimpan ke `saves/dw1/card1.mcd` (dan `card2.mcd`).
Saat bermain online, server menyimpan salinan tiap kartu per Tamer, sehingga progres mengikuti
akun dan bisa dilanjutkan dari komputer lain. Server **tidak** menjalankan logika game; ini
penyimpanan dan pengaman, bukan anti-cheat penuh.

## Alur

1. **Login** (layar login launcher) lalu pilih Digimon World 1.
2. **Sebelum game membaca kartu**, launcher menyamakan kartu lokal dengan server:

   | Keadaan | Hasil |
   |---|---|
   | Server kosong, kartu lokal berisi | Kartu lokal diunggah (versi 1) |
   | Kartu lokal kosong / tidak ada, server berisi | Server ditarik |
   | Sama persis | Tidak ada yang dilakukan |
   | Kartu berasal dari versi server sekarang lalu diubah | Kartu lokal diunggah |
   | Kartu tidak disentuh, server sudah maju (main di komputer lain) | Server ditarik |
   | Dua-duanya berubah, atau tidak ada catatan | **Server menang**; kartu lokal dicadangkan |

3. **Selama bermain**, perubahan kartu diunggah otomatis setelah stabil (dicek tiap 3 detik).
   Saat game berhenti, ada satu unggahan terakhir.
4. Status terakhir tampil di menu Pause (Esc).

## Pengaman

- Kartu lokal yang akan ditimpa **selalu** disalin ke `saves/dw1/backups/card<slot>-<waktu>.mcd`
  (20 terakhir per slot).
- Server menolak unggahan yang berangkat dari versi lama (`CONFLICT`); versi server tidak pernah
  tertimpa diam-diam. Setelah konflik, unggahan sesi itu dihentikan sampai game dimulai ulang.
- Server menolak kartu kosong yang akan menimpa kartu berisi (`EMPTY_OVERWRITE`).
- Server memeriksa ukuran (131072 byte), header `MC` + checksum, dan sha256 isi.
- Server menyimpan 5 versi terakhir per Tamer dan slot (`saveVersionsKept`), untuk pemulihan manual.
- Catatan sinkron (`card<slot>.sync.json`) terikat ke alamat server dan Tamer; akun lain di
  komputer yang sama tidak dianggap sinkron.

## Protokol WebSocket (setelah login; balasan membawa `reqId` yang sama)

| Klien -> Server | Server -> Klien |
|---|---|
| `SAVE_LIST {reqId}` | `SAVE_LIST_RESULT {saves:[{slot,version,sha256,usedBlocks,hasGameSave,uploadedAt}]}` |
| `SAVE_GET {reqId, slot}` | `SAVE_DATA {slot,version,sha256,data(base64)}` atau `SAVE_REJECTED NO_SAVE` |
| `SAVE_PUT {reqId, slot, baseVersion, sha256, data(base64)}` | `SAVE_OK {slot,version,sha256,unchanged}` atau `SAVE_REJECTED {code,...}` |

Kode penolakan: `BAD_REQUEST`, `BAD_SIZE`, `BAD_FORMAT`, `BAD_HASH`, `CONFLICT`, `EMPTY_OVERWRITE`,
`NO_SAVE`, `RATE_LIMITED` (unggahan ~1 per 5 detik, burst 4), `INTERNAL`.

## Yang belum dicakup

- Pemulihan versi lama dari server (data tersimpan, belum ada perintah/menu untuk memilihnya).
- Pemeriksaan kewajaran isi save (mis. lonjakan Bits yang tidak mungkin).
- Hanya Digimon World 1; DW2/DW3 tidak disinkronkan.
