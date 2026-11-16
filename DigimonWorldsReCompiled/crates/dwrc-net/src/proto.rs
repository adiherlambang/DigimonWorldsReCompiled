//! UDP protocol v2 (lihat docs/PROTOCOL.md di root repo).
//!
//! Klien -> server: 44 byte, ditandatangani HMAC-SHA256 dengan kunci sesi.
//! Server -> klien: 32 byte relay dari pemain lain di map yang sama.

use crate::state::NetShared;
use crate::PlayerTransform;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const UDP_IN_TYPE: u8 = 0x02;
pub const UDP_IN_LEN: usize = 44;
pub const UDP_OUT_TYPE: u8 = 0x03;
pub const UDP_OUT_LEN: usize = 32;
pub const MAC_LEN: usize = 8;
pub const DEFAULT_UDP_PORT: u16 = 7777;

/// Keadaan pemain lain yang diterima lewat UDP.
#[derive(Debug, Clone)]
pub struct RemotePlayer {
    pub tamer_id: u32,
    pub map_id: u8,
    pub species: u16,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub yaw: i16,
    pub anim: u8,
    pub digimon_anim: u8,
    pub digimon_x: i32,
    pub digimon_z: i32,
    pub last_update: Instant,
}

/// MAC 8 byte = HMAC-SHA256(key, data)[0..8].
pub fn mac8(key: &[u8], data: &[u8]) -> [u8; MAC_LEN] {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("HMAC menerima kunci panjang berapa pun");
    m.update(data);
    let full = m.finalize().into_bytes();
    let mut out = [0u8; MAC_LEN];
    out.copy_from_slice(&full[..MAC_LEN]);
    out
}

/// Paket sinkronisasi posisi (klien -> server).
pub fn pack_sync(tf: &PlayerTransform, session_id: u32, key: &[u8], seq: u32) -> [u8; UDP_IN_LEN] {
    let mut b = [0u8; UDP_IN_LEN];
    b[0] = UDP_IN_TYPE;
    b[1] = (tf.map_id & 0xFF) as u8;
    b[2..4].copy_from_slice(&tf.digimon_id.to_le_bytes());
    b[4..8].copy_from_slice(&tf.x.to_le_bytes());
    b[8..12].copy_from_slice(&tf.y.to_le_bytes());
    b[12..16].copy_from_slice(&tf.z.to_le_bytes());
    b[16..18].copy_from_slice(&tf.yaw.to_le_bytes());
    b[18] = tf.anim_id;
    b[19] = tf.digimon_anim;
    b[20..24].copy_from_slice(&tf.digimon_x.to_le_bytes());
    b[24..28].copy_from_slice(&tf.digimon_z.to_le_bytes());
    b[28..32].copy_from_slice(&session_id.to_le_bytes());
    b[32..36].copy_from_slice(&seq.to_le_bytes());
    let mac = mac8(key, &b[..36]);
    b[36..44].copy_from_slice(&mac);
    b
}

/// Urai paket relay dari server. `None` bila bukan paket relay yang valid.
pub fn parse_relay(buf: &[u8]) -> Option<RemotePlayer> {
    if buf.len() != UDP_OUT_LEN || buf[0] != UDP_OUT_TYPE {
        return None;
    }
    let i32_at = |o: usize| i32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
    Some(RemotePlayer {
        tamer_id: u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]),
        map_id: buf[1],
        species: u16::from_le_bytes([buf[2], buf[3]]),
        x: i32_at(4),
        y: i32_at(8),
        z: i32_at(12),
        yaw: i16::from_le_bytes([buf[16], buf[17]]),
        anim: buf[18],
        digimon_anim: buf[19],
        digimon_x: i32_at(20),
        digimon_z: i32_at(24),
        last_update: Instant::now(),
    })
}

/// Penghasil `seq` yang selalu naik, berbasis jam milidetik. Aman saat emulator
/// di-restart di dalam sesi login yang sama (nilai baru tetap lebih besar).
#[derive(Debug)]
pub struct SeqGen {
    last: u32,
}

impl SeqGen {
    pub fn new() -> Self {
        Self { last: 0 }
    }
    pub fn next(&mut self) -> u32 {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u32).unwrap_or(0);
        // Aritmetika serial: pilih yang "lebih baru" dari last, minimal last+1.
        let candidate = if now.wrapping_sub(self.last) < 0x8000_0000 && now != self.last { now } else { self.last.wrapping_add(1) };
        self.last = candidate;
        candidate
    }
}

impl Default for SeqGen {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolusi sekali (bukan tiap paket) alamat UDP server.
pub fn resolve_udp(host: &str, port: u16) -> Option<SocketAddr> {
    let host = if host.trim().is_empty() { "127.0.0.1" } else { host.trim() };
    (host, port).to_socket_addrs().ok()?.find(|a| a.is_ipv4())
}

/// Kuras semua paket relay yang menunggu di socket (non-blocking) ke `net.remote`.
/// Hanya menerima paket dari IP server, sisanya dibuang.
pub fn drain_relays(sock: &UdpSocket, server_ip: IpAddr, net: &NetShared) -> usize {
    let mut buf = [0u8; 128];
    let mut n = 0;
    for _ in 0..128 {
        match sock.recv_from(&mut buf) {
            Ok((len, src)) => {
                if src.ip() != server_ip {
                    continue;
                }
                if let Some(rp) = parse_relay(&buf[..len]) {
                    if let Ok(mut g) = net.remote.lock() {
                        g.insert(rp.tamer_id, rp);
                    }
                    n += 1;
                }
            }
            Err(_) => break, // WouldBlock (tidak ada lagi) atau error socket
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tf() -> PlayerTransform {
        PlayerTransform {
            player_id: 0, tamer_name: String::new(), map_id: 0xDA,
            x: 100, y: -5, z: 200, yaw: -300, anim_id: 4,
            digimon_id: 7, digimon_x: 9, digimon_y: 0, digimon_z: 8, digimon_anim: 2,
        }
    }

    // Vektor ini dihasilkan oleh game-server/src/protocol.js (buildClientPacket)
    // dengan sessionId=0x11223344, key=bytes 0..31, seq=0xAABBCCDD.
    // Jika test ini gagal, klien Rust dan server Node tidak lagi sepakat soal format.
    const NODE_VECTOR: &str = "02da070064000000fbffffffc8000000d4fe0402090000000800000044332211ddccbbaab40d2c5db7320930";

    #[test]
    fn pack_sync_identik_dengan_server_node() {
        let key: Vec<u8> = (0u8..32).collect();
        let pkt = pack_sync(&tf(), 0x1122_3344, &key, 0xAABB_CCDD);
        assert_eq!(hex::encode(pkt), NODE_VECTOR);
    }

    #[test]
    fn parse_relay_membaca_layout_server() {
        // Dihasilkan buildRelay() di Node dari paket di atas, tamerId=42.
        let mut raw = hex::decode(NODE_VECTOR).unwrap();
        raw.truncate(32);
        raw[0] = UDP_OUT_TYPE;
        raw[28..32].copy_from_slice(&42u32.to_le_bytes());
        let rp = parse_relay(&raw).expect("valid");
        assert_eq!((rp.tamer_id, rp.map_id, rp.species), (42, 0xDA, 7));
        assert_eq!((rp.x, rp.y, rp.z, rp.yaw), (100, -5, 200, -300));
        assert_eq!((rp.anim, rp.digimon_anim, rp.digimon_x, rp.digimon_z), (4, 2, 9, 8));
    }

    #[test]
    fn parse_relay_menolak_paket_rusak() {
        assert!(parse_relay(&[0u8; 31]).is_none());
        let mut p = [0u8; 32];
        p[0] = UDP_IN_TYPE; // type salah
        assert!(parse_relay(&p).is_none());
    }

    #[test]
    fn seq_selalu_naik_walau_jam_macet() {
        let mut g = SeqGen::new();
        let mut prev = g.next();
        for _ in 0..1000 {
            let n = g.next();
            assert!(n.wrapping_sub(prev) < 0x8000_0000 && n != prev, "{prev} -> {n}");
            prev = n;
        }
    }
}
