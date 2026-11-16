pub mod dw1_ram;

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct PlayerTransform {
    pub player_id: u32,
    pub tamer_name: String,
    pub map_id: u16,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub yaw: i16,
    pub anim_id: u8,
    pub digimon_id: u16,
    pub digimon_x: i32,
    pub digimon_y: i32,
    pub digimon_z: i32,
    pub digimon_anim: u8,
}

#[derive(Debug, Clone)]
pub struct AccountSession {
    pub server_host: String,
    pub username: String,
    pub tamer_id: u32,
    pub tamer_name: String,
    pub bits: u32,
    pub prosperity: u32,
    pub is_online: bool,
}

impl Default for AccountSession {
    fn default() -> Self {
        Self {
            server_host: "127.0.0.1".to_string(),
            username: "adi_tamer".to_string(),
            tamer_id: 1,
            tamer_name: "Adi".to_string(),
            bits: 500,
            prosperity: 1,
            is_online: false,
        }
    }
}

pub struct MapTransitionFilter {
    stable_map_id: u16,
    pending_map_id: u16,
    stable_ticks: u8,
    required_ticks: u8,
}

impl MapTransitionFilter {
    pub fn new(required_ticks: u8) -> Self {
        Self {
            stable_map_id: 0x00FF,
            pending_map_id: 0x00FF,
            stable_ticks: 0,
            required_ticks,
        }
    }

    pub fn update(&mut self, current: &PlayerTransform) -> Option<u16> {
        if current.map_id == 0x00FF || (current.x == 0 && current.y == 0 && current.z == 0) {
            self.stable_ticks = 0;
            return None;
        }
        if current.map_id == self.pending_map_id {
            self.stable_ticks = self.stable_ticks.saturating_add(1);
        } else {
            self.pending_map_id = current.map_id;
            self.stable_ticks = 1;
        }
        if self.stable_ticks >= self.required_ticks {
            self.stable_map_id = self.pending_map_id;
            Some(self.stable_map_id)
        } else {
            None
        }
    }
}

fn extract_json_str(src: &str, key: &str) -> Option<String> {
    let pat = format!("\"{}\":\"", key);
    let idx = src.find(&pat)? + pat.len();
    let end = src[idx..].find('"')?;
    Some(src[idx..idx + end].to_string())
}

fn extract_json_u32(src: &str, key: &str) -> Option<u32> {
    let pat = format!("\"{}\":", key);
    let idx = src.find(&pat)? + pat.len();
    let rest = src[idx..].trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Melakukan autentikasi WebSocket RFC 6455 ke game-server/server.js (port 8080)
pub fn authenticate_ws(server_host: &str, username: &str, password: &str) -> Result<AccountSession, String> {
    let host = if server_host.trim().is_empty() { "127.0.0.1" } else { server_host.trim() };
    let addr_str = format!("{host}:8080");
    let addr = addr_str
        .to_socket_addrs()
        .map_err(|e| format!("Alamat server tidak valid: {e}"))?
        .next()
        .ok_or_else(|| "Host tidak ditemukan".to_string())?;

    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|e| format!("Gagal terhubung ke {addr_str} ({e}). Pastikan node server.js berjalan."))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));

    let handshake = format!(
        "GET / HTTP/1.1\r\n         Host: {addr_str}\r\n         Upgrade: websocket\r\n         Connection: Upgrade\r\n         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n         Sec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(handshake.as_bytes()).map_err(|e| format!("Handshake gagal: {e}"))?;

    let mut hdr_buf = [0u8; 1024];
    let n = stream.read(&mut hdr_buf).map_err(|e| format!("Tidak ada respon handshake: {e}"))?;
    let hdr = String::from_utf8_lossy(&hdr_buf[..n]);
    if !hdr.contains("101") {
        return Err("Server menolak koneksi WebSocket".to_string());
    }

    let safe_user = username.trim().replace('"', "");
    let safe_pass = password.replace('"', "");
    let payload = format!(
        "{{\"type\":\"LOGIN\",\"username\":\"{safe_user}\",\"password\":\"{safe_pass}\",\"tamerId\":1}}"
    );
    let raw = payload.as_bytes();
    let mask = [0x12u8, 0x34, 0x56, 0x78];
    let mut frame = Vec::with_capacity(raw.len() + 8);
    frame.push(0x81); // FIN + Text frame
    if raw.len() < 126 {
        frame.push(0x80 | (raw.len() as u8));
    } else {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(raw.len() as u16).to_be_bytes());
    }
    frame.extend_from_slice(&mask);
    for (i, b) in raw.iter().enumerate() {
        frame.push(b ^ mask[i % 4]);
    }
    stream.write_all(&frame).map_err(|e| format!("Gagal mengirim paket LOGIN: {e}"))?;

    let mut resp_buf = [0u8; 2048];
    let rn = stream.read(&mut resp_buf).map_err(|e| format!("Gagal membaca balasan LOGIN: {e}"))?;
    if rn < 2 {
        return Err("Balasan server kosong".to_string());
    }
    let len_byte = resp_buf[1] & 0x7F;
    let offset = if len_byte == 126 { 4 } else if len_byte == 127 { 10 } else { 2 };
    let json = String::from_utf8_lossy(&resp_buf[offset.min(rn)..rn]);

    if !json.contains("LOGIN_SUCCESS") {
        return Err("Login ditolak oleh server".to_string());
    }

    let tamer_name = extract_json_str(&json, "TamerName").unwrap_or_else(|| {
        if safe_user.is_empty() { "Adi".to_string() } else { safe_user.clone() }
    });
    let tamer_id = extract_json_u32(&json, "TamerID").unwrap_or(1);
    let bits = extract_json_u32(&json, "Bits").unwrap_or(500);
    let prosperity = extract_json_u32(&json, "ProsperityPoints").unwrap_or(1);

    Ok(AccountSession {
        server_host: host.to_string(),
        username: if safe_user.is_empty() { "adi_tamer".to_string() } else { safe_user },
        tamer_id,
        tamer_name,
        bits,
        prosperity,
        is_online: true,
    })
}
