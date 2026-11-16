//! State jaringan yang dibagi antara thread WebSocket, thread emulasi, dan UI.

use crate::proto::RemotePlayer;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

/// Pemain lain di map yang sama, dari event WebSocket.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerInfo {
    pub name: String,
    pub species: u16,
}

/// Satu permintaan WebSocket yang menunggu balasan dengan `reqId` yang sama.
#[derive(Debug)]
pub struct Outgoing {
    pub req_id: u64,
    pub text: String,
    pub reply: mpsc::Sender<Value>,
}

#[derive(Debug, Default)]
pub struct NetShared {
    /// Antrean permintaan ke thread WebSocket (diisi saat koneksi terbentuk).
    pub outbox: Mutex<Option<mpsc::Sender<Outgoing>>>,
    next_req: AtomicU64,
    /// WebSocket masih tersambung.
    pub connected: AtomicBool,
    /// Alasan sesi berakhir (mis. login dari tempat lain). `None` = masih hidup.
    pub ended: Mutex<Option<String>>,
    /// tamer_id -> nama/species, hanya untuk map pemain saat ini.
    pub peers: Mutex<HashMap<u32, PeerInfo>>,
    /// tamer_id -> posisi terbaru dari UDP.
    pub remote: Mutex<HashMap<u32, RemotePlayer>>,
}

impl NetShared {
    /// Kirim satu pesan dan tunggu balasan yang membawa `reqId` yang sama.
    /// Dipanggil dari thread mana pun; WebSocket tetap hanya dipegang thread jaringan.
    pub fn request(&self, mut msg: Value, timeout: Duration) -> Result<Value, String> {
        let req_id = self.next_req.fetch_add(1, Ordering::Relaxed) + 1;
        msg["reqId"] = Value::from(req_id);
        let tx = self
            .outbox
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .ok_or_else(|| "Tidak terhubung ke server.".to_string())?;
        let (reply, rx) = mpsc::channel();
        tx.send(Outgoing { req_id, text: msg.to_string(), reply })
            .map_err(|_| "Koneksi ke server terputus.".to_string())?;
        rx.recv_timeout(timeout).map_err(|e| match e {
            mpsc::RecvTimeoutError::Timeout => "Server tidak membalas (waktu habis).".to_string(),
            mpsc::RecvTimeoutError::Disconnected => "Koneksi ke server terputus.".to_string(),
        })
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn end_reason(&self) -> Option<String> {
        self.ended.lock().ok().and_then(|g| g.clone())
    }

    pub fn set_ended(&self, reason: &str) {
        if let Ok(mut g) = self.ended.lock() {
            if g.is_none() {
                *g = Some(reason.to_string());
            }
        }
    }

    /// Jumlah pemain lain di map saat ini.
    pub fn peer_count(&self) -> usize {
        self.peers.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Salinan pemain lain beserta namanya (untuk digambar UI).
    pub fn snapshot(&self) -> Vec<(Option<PeerInfo>, RemotePlayer)> {
        let peers = self.peers.lock().map(|g| g.clone()).unwrap_or_default();
        let remote = self.remote.lock().map(|g| g.values().cloned().collect::<Vec<_>>()).unwrap_or_default();
        remote.into_iter().map(|r| (peers.get(&r.tamer_id).cloned(), r)).collect()
    }

    /// Buang pemain yang sudah lama tidak mengirim UDP.
    pub fn expire(&self, max_age: Duration) {
        if let Ok(mut g) = self.remote.lock() {
            g.retain(|_, r| r.last_update.elapsed() <= max_age);
        }
    }

    /// Terapkan satu pesan WebSocket dari server (JSON teks).
    pub fn apply_event(&self, text: &str) {
        let Ok(v) = serde_json::from_str::<Value>(text) else { return };
        match v["type"].as_str() {
            Some("MAP_ROSTER") => {
                let mut map = HashMap::new();
                if let Some(arr) = v["players"].as_array() {
                    for p in arr {
                        if let Some(id) = p["tamerId"].as_u64() {
                            map.insert(id as u32, peer_from(p));
                        }
                    }
                }
                if let Ok(mut g) = self.peers.lock() {
                    *g = map;
                }
                // Pindah map: posisi lama dari map sebelumnya tidak berlaku lagi.
                if let Ok(mut g) = self.remote.lock() {
                    g.clear();
                }
            }
            Some("PLAYER_JOIN") => {
                if let Some(id) = v["tamerId"].as_u64() {
                    if let Ok(mut g) = self.peers.lock() {
                        g.insert(id as u32, peer_from(&v));
                    }
                }
            }
            Some("PLAYER_LEAVE") => {
                if let Some(id) = v["tamerId"].as_u64() {
                    let id = id as u32;
                    if let Ok(mut g) = self.peers.lock() {
                        g.remove(&id);
                    }
                    if let Ok(mut g) = self.remote.lock() {
                        g.remove(&id);
                    }
                }
            }
            Some("SESSION_END") => {
                self.set_ended(v["reason"].as_str().unwrap_or("Sesi diakhiri server"));
            }
            _ => {}
        }
    }
}

fn peer_from(v: &Value) -> PeerInfo {
    PeerInfo {
        name: v["tamerName"].as_str().unwrap_or("?").to_string(),
        species: v["species"].as_u64().unwrap_or(0) as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::RemotePlayer;
    use std::time::Instant;

    fn rp(id: u32) -> RemotePlayer {
        RemotePlayer {
            tamer_id: id, map_id: 1, species: 3, x: 0, y: 0, z: 0, yaw: 0,
            anim: 0, digimon_anim: 0, digimon_x: 0, digimon_z: 0, last_update: Instant::now(),
        }
    }

    #[test]
    fn roster_join_leave() {
        let n = NetShared::default();
        n.remote.lock().unwrap().insert(99, rp(99));
        n.apply_event(r#"{"type":"MAP_ROSTER","mapId":218,"players":[{"tamerId":5,"tamerName":"Ann","species":3}]}"#);
        assert_eq!(n.peer_count(), 1);
        assert!(n.remote.lock().unwrap().is_empty(), "roster baru membersihkan posisi map lama");

        n.apply_event(r#"{"type":"PLAYER_JOIN","mapId":218,"tamerId":6,"tamerName":"Ben","species":9}"#);
        assert_eq!(n.peers.lock().unwrap()[&6], PeerInfo { name: "Ben".into(), species: 9 });

        n.remote.lock().unwrap().insert(6, rp(6));
        n.apply_event(r#"{"type":"PLAYER_LEAVE","mapId":218,"tamerId":6}"#);
        assert_eq!(n.peer_count(), 1);
        assert!(n.remote.lock().unwrap().is_empty());
    }

    #[test]
    fn session_end_dan_sampah() {
        let n = NetShared::default();
        n.apply_event("bukan json");
        n.apply_event(r#"{"type":"TIDAK_DIKENAL"}"#);
        assert!(n.end_reason().is_none());
        n.apply_event(r#"{"type":"SESSION_END","reason":"login dari tempat lain"}"#);
        assert_eq!(n.end_reason().as_deref(), Some("login dari tempat lain"));
    }

    #[test]
    fn expire_membuang_yang_basi() {
        let n = NetShared::default();
        let mut old = rp(1);
        old.last_update = Instant::now() - Duration::from_secs(10);
        n.remote.lock().unwrap().insert(1, old);
        n.remote.lock().unwrap().insert(2, rp(2));
        n.expire(Duration::from_secs(3));
        let g = n.remote.lock().unwrap();
        assert!(!g.contains_key(&1) && g.contains_key(&2));
    }
}
