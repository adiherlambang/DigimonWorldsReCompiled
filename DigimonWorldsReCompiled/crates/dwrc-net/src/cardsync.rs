//! Sinkronisasi memory card dengan server.
//!
//! Game berjalan di komputer pemain dan menyimpan ke `card1.mcd` / `card2.mcd`.
//! Server menyimpan salinan tiap kartu per Tamer (berversi), sehingga progres
//! mengikuti akun dan bisa dilanjutkan dari komputer lain.
//!
//! * Saat game dimulai: `sync_on_login` membandingkan kartu lokal dengan server.
//!   Server yang lebih baru ditarik; kartu lokal yang lebih baru diunggah.
//!   Kartu lokal yang akan ditimpa SELALU dicadangkan dulu ke `backups/`.
//! * Selama bermain: `CardUploader` mengunggah kartu yang berubah.
//!
//! Server tidak pernah menimpa versi yang lebih baru: unggahan membawa versi
//! dasarnya, dan bila server sudah maju ia menolak (`CONFLICT`).

use crate::state::NetShared;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const CARD_SIZE: usize = 128 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_EVERY: Duration = Duration::from_secs(3);
const BACKUPS_KEPT: usize = 20;

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

// ---------------------------------------------------------------------------
// Memory card
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardInfo {
    pub used_blocks: u32,
    pub has_game_save: bool,
}

/// `None` bila data bukan memory card PS1 yang utuh (ukuran atau header salah).
pub fn inspect(data: &[u8]) -> Option<CardInfo> {
    if data.len() != CARD_SIZE || data[0] != b'M' || data[1] != b'C' {
        return None;
    }
    if data[..127].iter().fold(0u8, |a, b| a ^ b) != data[127] {
        return None;
    }
    let mut used_blocks = 0;
    let mut has_game_save = false;
    for i in 1..16 {
        let f = &data[i * 128..(i + 1) * 128];
        if (0x51..=0x53).contains(&f[0]) {
            used_blocks += 1;
        }
        if f[0] == 0x51 {
            let name = &f[0x0A..0x1E];
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            if name[..end].starts_with(b"BASLUS-01032") {
                has_game_save = true;
            }
        }
    }
    Some(CardInfo { used_blocks, has_game_save })
}

// ---------------------------------------------------------------------------
// Server (diabstraksikan supaya logikanya bisa diuji tanpa jaringan)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSlot {
    pub slot: u8,
    pub version: u32,
    pub sha: String,
    pub used_blocks: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCard {
    pub version: u32,
    pub sha: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutResult {
    Ok { version: u32, sha: String },
    /// Server sudah punya versi yang lebih baru dari `base`.
    Conflict { version: u32 },
    Rejected { code: String, message: String },
}

pub trait CardServer: Send + Sync {
    fn list(&self) -> Result<Vec<ServerSlot>, String>;
    fn get(&self, slot: u8) -> Result<Option<ServerCard>, String>;
    fn put(&self, slot: u8, base: u32, data: &[u8]) -> Result<PutResult, String>;
}

impl CardServer for NetShared {
    fn list(&self) -> Result<Vec<ServerSlot>, String> {
        let r = self.request(json!({ "type": "SAVE_LIST" }), REQUEST_TIMEOUT)?;
        if r["type"] != "SAVE_LIST_RESULT" {
            return Err(reject_text(&r));
        }
        Ok(r["saves"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| {
                        Some(ServerSlot {
                            slot: s["slot"].as_u64()? as u8,
                            version: s["version"].as_u64()? as u32,
                            sha: s["sha256"].as_str()?.to_string(),
                            used_blocks: s["usedBlocks"].as_u64().unwrap_or(0) as u32,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn get(&self, slot: u8) -> Result<Option<ServerCard>, String> {
        let r = self.request(json!({ "type": "SAVE_GET", "slot": slot }), REQUEST_TIMEOUT)?;
        match r["type"].as_str() {
            Some("SAVE_DATA") => {
                let data = B64
                    .decode(r["data"].as_str().unwrap_or(""))
                    .map_err(|_| "Data save dari server tidak valid.".to_string())?;
                Ok(Some(ServerCard {
                    version: r["version"].as_u64().unwrap_or(0) as u32,
                    sha: r["sha256"].as_str().unwrap_or("").to_string(),
                    data,
                }))
            }
            Some("SAVE_REJECTED") if r["code"] == "NO_SAVE" => Ok(None),
            _ => Err(reject_text(&r)),
        }
    }

    fn put(&self, slot: u8, base: u32, data: &[u8]) -> Result<PutResult, String> {
        let msg = json!({
            "type": "SAVE_PUT", "slot": slot, "baseVersion": base,
            "sha256": sha256_hex(data), "data": B64.encode(data),
        });
        let r = self.request(msg, REQUEST_TIMEOUT)?;
        match r["type"].as_str() {
            Some("SAVE_OK") => Ok(PutResult::Ok {
                version: r["version"].as_u64().unwrap_or(0) as u32,
                sha: r["sha256"].as_str().unwrap_or("").to_string(),
            }),
            Some("SAVE_REJECTED") if r["code"] == "CONFLICT" => {
                Ok(PutResult::Conflict { version: r["version"].as_u64().unwrap_or(0) as u32 })
            }
            Some("SAVE_REJECTED") => Ok(PutResult::Rejected {
                code: r["code"].as_str().unwrap_or("?").to_string(),
                message: r["message"].as_str().unwrap_or("Ditolak server.").to_string(),
            }),
            _ => Err(reject_text(&r)),
        }
    }
}

fn reject_text(r: &Value) -> String {
    r["message"].as_str().unwrap_or("Balasan server tidak dikenal.").to_string()
}

// ---------------------------------------------------------------------------
// Berkas lokal
// ---------------------------------------------------------------------------

/// Identitas akun; catatan sinkronisasi hanya berlaku untuk server + Tamer yang sama.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardIdentity {
    pub server: String,
    pub tamer_id: u32,
}

/// Versi server terakhir yang cocok dengan kartu lokal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub version: u32,
    pub sha: String,
}

pub fn card_path(dir: &Path, slot: u8) -> PathBuf {
    dir.join(format!("card{slot}.mcd"))
}

fn sidecar_path(dir: &Path, slot: u8) -> PathBuf {
    dir.join(format!("card{slot}.sync.json"))
}

fn read_sidecar(dir: &Path, slot: u8, id: &CardIdentity) -> Option<Known> {
    let v: Value = serde_json::from_slice(&std::fs::read(sidecar_path(dir, slot)).ok()?).ok()?;
    if v["server"].as_str()? != id.server || v["tamerId"].as_u64()? != id.tamer_id as u64 {
        return None;
    }
    Some(Known { version: v["version"].as_u64()? as u32, sha: v["sha256"].as_str()?.to_string() })
}

fn write_sidecar(dir: &Path, slot: u8, id: &CardIdentity, k: &Known) {
    let v = json!({ "server": id.server, "tamerId": id.tamer_id, "version": k.version, "sha256": k.sha });
    let _ = write_atomic(&sidecar_path(dir, slot), v.to_string().as_bytes());
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Kartu lokal yang utuh; `None` bila berkas tidak ada atau bukan memory card.
fn read_local(path: &Path) -> Option<(Vec<u8>, CardInfo)> {
    let data = std::fs::read(path).ok()?;
    let info = inspect(&data)?;
    Some((data, info))
}

/// Salin kartu lokal ke `backups/` sebelum ditimpa. Mengembalikan lokasi cadangan.
fn backup(dir: &Path, slot: u8) -> Option<PathBuf> {
    let src = card_path(dir, slot);
    if !src.is_file() {
        return None;
    }
    let bdir = dir.join("backups");
    std::fs::create_dir_all(&bdir).ok()?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut dst = bdir.join(format!("card{slot}-{ts}.mcd"));
    let mut n = 1;
    while dst.exists() {
        dst = bdir.join(format!("card{slot}-{ts}-{n}.mcd"));
        n += 1;
    }
    std::fs::copy(&src, &dst).ok()?;
    // Simpan hanya yang terbaru.
    let prefix = format!("card{slot}-");
    let mut all: Vec<PathBuf> = std::fs::read_dir(&bdir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix)))
        .collect();
    all.sort();
    while all.len() > BACKUPS_KEPT {
        let _ = std::fs::remove_file(all.remove(0));
    }
    Some(dst)
}

// ---------------------------------------------------------------------------
// Keputusan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCard {
    pub sha: String,
    pub used_blocks: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Nothing,
    /// Kartu lokal sama dengan server.
    InSync,
    /// Kartu lokal lebih baru: unggah di atas `base`.
    Push { base: u32 },
    /// Ambil dari server. `conflict` = lokal dan server sama-sama berubah (server menang, lokal dicadangkan).
    Pull { conflict: bool },
}

/// Menentukan arah sinkronisasi satu slot saat game dimulai.
///
/// `sidecar` = versi server terakhir yang pernah cocok dengan kartu ini di komputer ini.
pub fn plan(local: Option<&LocalCard>, sidecar: Option<&Known>, server: Option<&ServerSlot>) -> Plan {
    let local = local.filter(|l| l.used_blocks > 0); // kartu kosong tidak punya apa-apa untuk dilindungi
    let Some(sv) = server else {
        return if local.is_some() { Plan::Push { base: 0 } } else { Plan::Nothing };
    };
    let Some(l) = local else { return Plan::Pull { conflict: false } };
    if l.sha == sv.sha {
        return Plan::InSync;
    }
    match sidecar {
        // Kartu ini berasal dari versi server sekarang lalu diubah pemain: lokal lebih baru.
        Some(k) if k.version == sv.version && k.sha == sv.sha => Plan::Push { base: sv.version },
        // Kartu tidak disentuh sejak sinkronisasi lama; server sudah maju (mis. main di komputer lain).
        Some(k) if k.sha == l.sha => Plan::Pull { conflict: false },
        // Dua-duanya berubah, atau tidak ada catatan: akun adalah sumber kebenaran.
        _ => Plan::Pull { conflict: true },
    }
}

// ---------------------------------------------------------------------------
// Sinkronisasi saat game dimulai
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct SyncOutcome {
    /// Pesan untuk pemain (Indonesia), satu per kejadian penting.
    pub lines: Vec<String>,
    /// Keadaan per slot setelah sinkronisasi, dipakai `CardUploader`.
    pub known: Vec<(u8, Option<Known>)>,
    /// Ada slot yang kartu lokalnya digantikan versi server.
    pub pulled: bool,
}

pub fn sync_on_login(server: &dyn CardServer, dir: &Path, id: &CardIdentity, slots: &[u8]) -> Result<SyncOutcome, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Folder save tidak bisa dibuat: {e}"))?;
    let listing = server.list()?;
    let mut out = SyncOutcome::default();
    for &slot in slots {
        let path = card_path(dir, slot);
        let local = read_local(&path);
        let local_card = local.as_ref().map(|(d, i)| LocalCard { sha: sha256_hex(d), used_blocks: i.used_blocks });
        let sidecar = read_sidecar(dir, slot, id);
        let sv = listing.iter().find(|s| s.slot == slot);
        let mut known = sv.map(|s| Known { version: s.version, sha: s.sha.clone() });
        match plan(local_card.as_ref(), sidecar.as_ref(), sv) {
            Plan::Nothing => {}
            Plan::InSync => {
                if let Some(k) = &known {
                    write_sidecar(dir, slot, id, k);
                }
            }
            Plan::Push { base } => {
                let data = local.as_ref().map(|(d, _)| d.as_slice()).unwrap_or_default();
                match server.put(slot, base, data)? {
                    PutResult::Ok { version, sha } => {
                        let k = Known { version, sha };
                        write_sidecar(dir, slot, id, &k);
                        known = Some(k);
                        out.lines.push(format!("Kartu {slot}: save lokal diunggah ke server (versi {version})."));
                    }
                    PutResult::Conflict { version } => {
                        out.lines.push(format!("Kartu {slot}: server sudah punya versi {version}; save lokal tidak diunggah."));
                    }
                    PutResult::Rejected { message, .. } => {
                        out.lines.push(format!("Kartu {slot}: unggahan ditolak server ({message})."));
                    }
                }
            }
            Plan::Pull { conflict } => {
                let Some(card) = server.get(slot)? else { continue };
                if sha256_hex(&card.data) != card.sha || inspect(&card.data).is_none() {
                    out.lines.push(format!("Kartu {slot}: data dari server rusak; save lokal dipakai apa adanya."));
                    continue;
                }
                let bak = if local.is_some() { backup(dir, slot) } else { None };
                write_atomic(&path, &card.data).map_err(|e| format!("Kartu {slot} tidak bisa ditulis: {e}"))?;
                let k = Known { version: card.version, sha: card.sha };
                write_sidecar(dir, slot, id, &k);
                known = Some(k);
                out.pulled = true;
                let mut line = if conflict {
                    format!("Kartu {slot}: save lokal berbeda dari server; memakai versi server ({}).", card.version)
                } else {
                    format!("Kartu {slot}: save terbaru diambil dari server (versi {}).", card.version)
                };
                if let Some(b) = bak {
                    line.push_str(&format!(" Save lokal lama dicadangkan di {}.", b.display()));
                }
                out.lines.push(line);
            }
        }
        out.known.push((slot, known));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Unggah otomatis selama bermain
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct SyncStatus {
    last: Mutex<String>,
    /// Server punya versi lebih baru dari yang dipakai sesi ini: unggahan dihentikan.
    pub conflict: AtomicBool,
}

impl SyncStatus {
    pub fn text(&self) -> String {
        self.last.lock().map(|g| g.clone()).unwrap_or_default()
    }
    fn set(&self, s: impl Into<String>) {
        if let Ok(mut g) = self.last.lock() {
            *g = s.into();
        }
    }
}

/// Memantau satu kartu dan mengunggahnya setelah berubah dan stabil.
struct Watcher {
    slot: u8,
    known: Option<Known>,
    candidate: Option<String>,
    stable: u32,
    /// Isi yang sudah ditolak server; tidak dicoba lagi sampai isinya berubah.
    rejected: Option<String>,
}

impl Watcher {
    /// `force` = pemeriksaan terakhir saat game berhenti (tanpa menunggu stabil).
    fn poll(&mut self, server: &dyn CardServer, dir: &Path, id: &CardIdentity, force: bool, status: &SyncStatus) {
        if status.conflict.load(Ordering::Relaxed) {
            return;
        }
        let Some((data, info)) = read_local(&card_path(dir, self.slot)) else { return };
        let sha = sha256_hex(&data);
        if self.known.as_ref().is_some_and(|k| k.sha == sha) || self.rejected.as_deref() == Some(sha.as_str()) {
            self.candidate = None;
            self.stable = 0;
            return;
        }
        if self.candidate.as_deref() == Some(sha.as_str()) {
            self.stable += 1;
        } else {
            self.candidate = Some(sha.clone());
            self.stable = 1;
        }
        // Dua pembacaan sama berturut-turut: game selesai menulis kartu.
        if self.stable < 2 && !force {
            return;
        }
        if info.used_blocks == 0 && self.known.is_none() {
            return; // kartu kosong dan belum ada di server: tidak ada yang perlu disimpan
        }
        let base = self.known.as_ref().map(|k| k.version).unwrap_or(0);
        match server.put(self.slot, base, &data) {
            Ok(PutResult::Ok { version, sha: s }) => {
                let k = Known { version, sha: s };
                write_sidecar(dir, self.slot, id, &k);
                self.known = Some(k);
                self.candidate = None;
                self.stable = 0;
                status.set(format!("Save online: kartu {} tersimpan (versi {version})", self.slot));
            }
            Ok(PutResult::Conflict { version }) => {
                status.conflict.store(true, Ordering::Relaxed);
                status.set(format!(
                    "Save online: server sudah punya versi {version} yang lebih baru. Tutup game lalu mulai lagi untuk menariknya."
                ));
            }
            Ok(PutResult::Rejected { code, message }) => {
                self.rejected = Some(sha);
                status.set(format!("Save online ditolak ({code}): {message}"));
            }
            Err(e) => status.set(format!("Save online gagal terkirim, akan dicoba lagi: {e}")),
        }
    }
}

pub struct CardUploader {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub status: Arc<SyncStatus>,
}

impl CardUploader {
    pub fn start(
        server: Arc<dyn CardServer>,
        dir: PathBuf,
        id: CardIdentity,
        known: Vec<(u8, Option<Known>)>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(SyncStatus::default());
        let (st, ss) = (stop.clone(), status.clone());
        let mut watchers: Vec<Watcher> = known
            .into_iter()
            .map(|(slot, known)| Watcher { slot, known, candidate: None, stable: 0, rejected: None })
            .collect();
        let thread = std::thread::Builder::new()
            .name("mmo-cardsync".into())
            .spawn(move || {
                let mut waited = POLL_EVERY;
                loop {
                    let stopping = st.load(Ordering::Relaxed);
                    if stopping || waited >= POLL_EVERY {
                        for w in &mut watchers {
                            w.poll(server.as_ref(), &dir, &id, stopping, &ss);
                        }
                        waited = Duration::ZERO;
                    }
                    if stopping {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                    waited += Duration::from_millis(200);
                }
            })
            .ok();
        Self { stop, thread, status }
    }

    /// Menghentikan pemantauan setelah satu unggahan terakhir (kartu sudah ditulis game).
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for CardUploader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU32;

    fn xor(b: &[u8]) -> u8 {
        b.iter().fold(0, |a, x| a ^ x)
    }

    /// Kartu terformat seperti `MemoryCard::new_formatted()`; `game` mengisi blok 1 dengan save Digimon World.
    fn card(game: bool, marker: u8) -> Vec<u8> {
        let mut b = vec![0u8; CARD_SIZE];
        b[0] = b'M';
        b[1] = b'C';
        b[127] = xor(&b[..127]);
        for i in 1..16 {
            b[i * 128] = 0xA0;
            b[i * 128 + 8] = 0xFF;
            b[i * 128 + 9] = 0xFF;
            b[i * 128 + 127] = xor(&b[i * 128..i * 128 + 127]);
        }
        if game {
            b[128] = 0x51;
            b[128 + 0x0A..128 + 0x0A + 19].copy_from_slice(b"BASLUS-01032DIGIMON");
            b[128 + 127] = xor(&b[128..128 + 127]);
        }
        b[8192] = marker;
        b
    }

    fn tmp() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!("dwrc-cardsync-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn me() -> CardIdentity {
        CardIdentity { server: "127.0.0.1".into(), tamer_id: 7 }
    }

    /// Server palsu dengan aturan yang sama dengan game-server (versi dasar, data sama, kartu kosong).
    #[derive(Default)]
    struct Fake {
        slots: Mutex<HashMap<u8, (u32, Vec<u8>)>>,
        puts: Mutex<u32>,
        corrupt_get: AtomicBool,
    }
    impl Fake {
        fn with(slot: u8, version: u32, data: Vec<u8>) -> Self {
            let f = Fake::default();
            f.slots.lock().unwrap().insert(slot, (version, data));
            f
        }
    }
    impl CardServer for Fake {
        fn list(&self) -> Result<Vec<ServerSlot>, String> {
            Ok(self
                .slots
                .lock()
                .unwrap()
                .iter()
                .map(|(s, (v, d))| ServerSlot {
                    slot: *s,
                    version: *v,
                    sha: sha256_hex(d),
                    used_blocks: inspect(d).map(|i| i.used_blocks).unwrap_or(0),
                })
                .collect())
        }
        fn get(&self, slot: u8) -> Result<Option<ServerCard>, String> {
            Ok(self.slots.lock().unwrap().get(&slot).map(|(v, d)| {
                let mut data = d.clone();
                let sha = sha256_hex(&data);
                if self.corrupt_get.load(Ordering::Relaxed) {
                    data[9000] ^= 0xFF;
                }
                ServerCard { version: *v, sha, data }
            }))
        }
        fn put(&self, slot: u8, base: u32, data: &[u8]) -> Result<PutResult, String> {
            *self.puts.lock().unwrap() += 1;
            let mut g = self.slots.lock().unwrap();
            let cur = g.get(&slot).cloned();
            let cur_v = cur.as_ref().map(|c| c.0).unwrap_or(0);
            if let Some((v, d)) = &cur {
                if sha256_hex(d) == sha256_hex(data) {
                    return Ok(PutResult::Ok { version: *v, sha: sha256_hex(d) });
                }
            }
            if cur_v != base {
                return Ok(PutResult::Conflict { version: cur_v });
            }
            if cur.as_ref().is_some_and(|(_, d)| inspect(d).unwrap().used_blocks > 0) && inspect(data).unwrap().used_blocks == 0 {
                return Ok(PutResult::Rejected { code: "EMPTY_OVERWRITE".into(), message: "kosong".into() });
            }
            g.insert(slot, (cur_v + 1, data.to_vec()));
            Ok(PutResult::Ok { version: cur_v + 1, sha: sha256_hex(data) })
        }
    }

    // ---- inspect ----------------------------------------------------------

    #[test]
    fn inspect_membedakan_kartu_kosong_isi_dan_rusak() {
        let blank = inspect(&card(false, 0)).unwrap();
        assert_eq!((blank.used_blocks, blank.has_game_save), (0, false));
        let game = inspect(&card(true, 0)).unwrap();
        assert_eq!((game.used_blocks, game.has_game_save), (1, true));
        let mut bad = card(true, 0);
        bad[127] ^= 1;
        assert!(inspect(&bad).is_none(), "checksum header salah");
        assert!(inspect(&card(true, 0)[..1000]).is_none(), "ukuran salah");
    }

    // ---- keputusan --------------------------------------------------------

    fn lc(data: &[u8]) -> LocalCard {
        LocalCard { sha: sha256_hex(data), used_blocks: inspect(data).unwrap().used_blocks }
    }
    fn sv(version: u32, data: &[u8]) -> ServerSlot {
        ServerSlot { slot: 1, version, sha: sha256_hex(data), used_blocks: 1 }
    }
    fn kn(version: u32, data: &[u8]) -> Known {
        Known { version, sha: sha256_hex(data) }
    }

    #[test]
    fn plan_semua_kasus() {
        let (a, b, blank) = (card(true, 1), card(true, 2), card(false, 0));
        // Server kosong
        assert_eq!(plan(Some(&lc(&a)), None, None), Plan::Push { base: 0 });
        assert_eq!(plan(Some(&lc(&blank)), None, None), Plan::Nothing);
        assert_eq!(plan(None, None, None), Plan::Nothing);
        // Server berisi, lokal tidak ada / kosong: tarik tanpa konflik
        assert_eq!(plan(None, None, Some(&sv(3, &a))), Plan::Pull { conflict: false });
        assert_eq!(plan(Some(&lc(&blank)), None, Some(&sv(3, &a))), Plan::Pull { conflict: false });
        // Sama persis
        assert_eq!(plan(Some(&lc(&a)), None, Some(&sv(3, &a))), Plan::InSync);
        // Kartu berasal dari versi server sekarang lalu diubah pemain
        assert_eq!(plan(Some(&lc(&b)), Some(&kn(3, &a)), Some(&sv(3, &a))), Plan::Push { base: 3 });
        // Kartu tidak disentuh, server sudah maju
        assert_eq!(plan(Some(&lc(&a)), Some(&kn(2, &a)), Some(&sv(3, &b))), Plan::Pull { conflict: false });
        // Dua-duanya berubah, atau tanpa catatan: server menang, ditandai konflik
        assert_eq!(plan(Some(&lc(&b)), Some(&kn(2, &a)), Some(&sv(3, &card(true, 3)))), Plan::Pull { conflict: true });
        assert_eq!(plan(Some(&lc(&b)), None, Some(&sv(3, &a))), Plan::Pull { conflict: true });
    }

    // ---- sinkronisasi saat game dimulai -----------------------------------

    #[test]
    fn login_pertama_dengan_save_lokal_mengunggahnya() {
        let (dir, server) = (tmp(), Fake::default());
        let local = card(true, 5);
        std::fs::write(card_path(&dir, 1), &local).unwrap();
        let out = sync_on_login(&server, &dir, &me(), &[1, 2]).unwrap();
        assert_eq!(server.slots.lock().unwrap()[&1], (1, local.clone()));
        assert_eq!(out.known[0], (1, Some(kn(1, &local))));
        assert_eq!(out.known[1], (2, None), "slot 2 tidak ada di mana pun");
        assert!(!out.pulled && out.lines.len() == 1);
        assert_eq!(read_sidecar(&dir, 1, &me()), Some(kn(1, &local)));
    }

    #[test]
    fn komputer_kedua_menarik_save_tanpa_cadangan() {
        let dir = tmp();
        let remote = card(true, 9);
        let server = Fake::with(1, 4, remote.clone());
        let out = sync_on_login(&server, &dir, &me(), &[1]).unwrap();
        assert_eq!(std::fs::read(card_path(&dir, 1)).unwrap(), remote);
        assert!(out.pulled);
        assert!(!dir.join("backups").exists(), "tidak ada yang perlu dicadangkan");
        assert_eq!(out.known[0], (1, Some(kn(4, &remote))));
    }

    #[test]
    fn konflik_server_menang_dan_save_lokal_dicadangkan() {
        let dir = tmp();
        let (mine, theirs) = (card(true, 1), card(true, 2));
        std::fs::write(card_path(&dir, 1), &mine).unwrap();
        let server = Fake::with(1, 6, theirs.clone());
        let out = sync_on_login(&server, &dir, &me(), &[1]).unwrap();
        assert_eq!(std::fs::read(card_path(&dir, 1)).unwrap(), theirs);
        assert!(out.lines[0].contains("berbeda dari server") && out.lines[0].contains("dicadangkan"), "{:?}", out.lines);
        let backups: Vec<_> = std::fs::read_dir(dir.join("backups")).unwrap().collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(backups[0].as_ref().unwrap().path()).unwrap(), mine, "isi lokal lama utuh di cadangan");
    }

    #[test]
    fn main_di_dua_komputer_bergantian() {
        // Komputer A: unggah v1, lalu main dan unggah v2. Komputer B menarik v2.
        let (dir_a, dir_b, server) = (tmp(), tmp(), Fake::default());
        let v1 = card(true, 1);
        std::fs::write(card_path(&dir_a, 1), &v1).unwrap();
        sync_on_login(&server, &dir_a, &me(), &[1]).unwrap();

        let v2 = card(true, 2);
        std::fs::write(card_path(&dir_a, 1), &v2).unwrap();
        let out = sync_on_login(&server, &dir_a, &me(), &[1]).unwrap(); // login berikutnya melihat kartu berubah
        assert_eq!(server.slots.lock().unwrap()[&1].0, 2);
        assert!(out.lines[0].contains("diunggah"));

        let out_b = sync_on_login(&server, &dir_b, &me(), &[1]).unwrap();
        assert_eq!(std::fs::read(card_path(&dir_b, 1)).unwrap(), v2);
        assert!(out_b.pulled);

        // A tidak main, B main dan unggah v3; A login lagi: tarik tanpa konflik.
        let v3 = card(true, 3);
        std::fs::write(card_path(&dir_b, 1), &v3).unwrap();
        sync_on_login(&server, &dir_b, &me(), &[1]).unwrap();
        let out_a = sync_on_login(&server, &dir_a, &me(), &[1]).unwrap();
        assert_eq!(std::fs::read(card_path(&dir_a, 1)).unwrap(), v3);
        assert!(!out_a.lines[0].contains("berbeda dari server"), "bukan konflik: {:?}", out_a.lines);
    }

    #[test]
    fn catatan_sinkron_akun_lain_diabaikan() {
        let dir = tmp();
        let server = Fake::default();
        std::fs::write(card_path(&dir, 1), card(true, 1)).unwrap();
        sync_on_login(&server, &dir, &me(), &[1]).unwrap();
        let other = CardIdentity { server: "127.0.0.1".into(), tamer_id: 99 };
        assert!(read_sidecar(&dir, 1, &other).is_none());
        assert!(read_sidecar(&dir, 1, &CardIdentity { server: "10.0.0.5".into(), tamer_id: 7 }).is_none());
    }

    #[test]
    fn data_server_yang_rusak_tidak_menimpa_save_lokal() {
        let dir = tmp();
        let mine = card(true, 1);
        std::fs::write(card_path(&dir, 1), &mine).unwrap();
        let server = Fake::with(1, 3, card(true, 2));
        server.corrupt_get.store(true, Ordering::Relaxed);
        let out = sync_on_login(&server, &dir, &me(), &[1]).unwrap();
        assert_eq!(std::fs::read(card_path(&dir, 1)).unwrap(), mine);
        assert!(out.lines[0].contains("rusak"));
    }

    // ---- unggah selama bermain --------------------------------------------

    #[test]
    fn watcher_menunggu_stabil_lalu_mengunggah_sekali() {
        let (dir, server, status) = (tmp(), Fake::default(), SyncStatus::default());
        let mut w = Watcher { slot: 1, known: None, candidate: None, stable: 0, rejected: None };
        std::fs::write(card_path(&dir, 1), card(true, 1)).unwrap();
        w.poll(&server, &dir, &me(), false, &status);
        assert_eq!(*server.puts.lock().unwrap(), 0, "baru terlihat sekali: tunggu stabil");
        w.poll(&server, &dir, &me(), false, &status);
        assert_eq!(*server.puts.lock().unwrap(), 1);
        w.poll(&server, &dir, &me(), false, &status);
        w.poll(&server, &dir, &me(), false, &status);
        assert_eq!(*server.puts.lock().unwrap(), 1, "tidak berubah, tidak diunggah lagi");
        assert!(status.text().contains("versi 1"));

        std::fs::write(card_path(&dir, 1), card(true, 2)).unwrap();
        w.poll(&server, &dir, &me(), true, &status); // pemeriksaan terakhir tidak menunggu stabil
        assert_eq!(server.slots.lock().unwrap()[&1].0, 2);
    }

    #[test]
    fn watcher_berhenti_saat_server_sudah_lebih_baru() {
        let (dir, status) = (tmp(), SyncStatus::default());
        let server = Fake::with(1, 5, card(true, 8));
        let mut w = Watcher { slot: 1, known: Some(kn(2, &card(true, 1))), candidate: None, stable: 0, rejected: None };
        std::fs::write(card_path(&dir, 1), card(true, 3)).unwrap();
        w.poll(&server, &dir, &me(), true, &status);
        assert!(status.conflict.load(Ordering::Relaxed));
        assert_eq!(server.slots.lock().unwrap()[&1], (5, card(true, 8)), "versi server tidak tertimpa");
        let puts = *server.puts.lock().unwrap();
        w.poll(&server, &dir, &me(), true, &status);
        assert_eq!(*server.puts.lock().unwrap(), puts, "setelah konflik tidak mencoba lagi");
    }

    #[test]
    fn watcher_tidak_mengulang_isi_yang_ditolak() {
        let (dir, status) = (tmp(), SyncStatus::default());
        let server = Fake::with(1, 1, card(true, 1));
        let mut w = Watcher { slot: 1, known: Some(kn(1, &card(true, 1))), candidate: None, stable: 0, rejected: None };
        std::fs::write(card_path(&dir, 1), card(false, 0)).unwrap(); // kartu kosong menimpa kartu berisi
        w.poll(&server, &dir, &me(), true, &status);
        w.poll(&server, &dir, &me(), true, &status);
        assert_eq!(*server.puts.lock().unwrap(), 1, "isi yang sama tidak dikirim dua kali");
        assert!(status.text().contains("EMPTY_OVERWRITE"));
        assert_eq!(server.slots.lock().unwrap()[&1].1, card(true, 1));
    }
}
