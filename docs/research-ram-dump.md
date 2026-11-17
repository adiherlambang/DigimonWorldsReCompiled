# Alat riset: dump RAM (Digimon World 1)

Dipakai untuk mencari tahu bagaimana game menyimpan entitas dunia (NPC, Tamer, partner) di RAM,
sebagai dasar menampilkan pemain lain di dalam layar game. Alat ini hanya **membaca** RAM.

## Cara memakai

1. Mulai Digimon World 1 (online maupun lokal sama saja).
2. Tekan **F9** saat bermain, atau buka menu Pause (Esc) lalu pilih "Dump RAM for research".
3. Di layar muncul pesan `RAM dump saved: <lokasi>`. Dua berkas dibuat di folder `research` di
   dalam folder data launcher:
   - `dump-<waktu>-map<XX>.ram`: seluruh RAM konsol (2 MiB), untuk analisis offline.
   - `dump-<waktu>-map<XX>.txt`: ringkasan teks (map, status menu/dialog, posisi, objek Tamer/partner,
     siapa yang menunjuk ke objek itu, objek lain yang sejenis).
4. Tekan **F12** pada saat yang sama untuk mengambil screenshot; gambar itu membantu mencocokkan
   NPC yang terlihat dengan isi dump.

Dump tidak berisi disc, BIOS, atau berkas game; hanya isi RAM saat itu. RAM berisi data game yang sedang
berjalan, jadi jangan dibagikan secara publik.

## Dump yang berguna (tiga sampai lima)

| # | Situasi | Tujuan |
|---|---|---|
| A | File City, berdiri diam di satu titik, tanpa menu/dialog | Baseline entitas |
| B | File City, berjalan jauh ke titik lain (map sama), tanpa menu/dialog | Membedakan field posisi dari data lain |
| C | File City, dekat NPC yang terlihat di layar (screenshot F12 juga) | Mencocokkan NPC dengan entitas |
| D | Dalam rumah Jijimon (map interior) | Perilaku entitas antar map |
| E | (opsional) Tropical Jungle atau map luar lain | Pembanding |

Catat satu baris per dump: di mana kamu berdiri dan NPC apa yang tampak.
