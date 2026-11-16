//! Klien WebSocket ke game-server (port 8080).
//!
//! Server menutup sesi begitu WebSocket putus, jadi koneksi HARUS dipertahankan
//! selama pemain online. `NetClient` memegangnya di thread latar dan menerapkan
//! event (roster/join/leave/session_end) ke `NetShared`.

use crate::proto::DEFAULT_UDP_PORT;
use crate::state::NetShared;
use crate::AccountSession;
use serde_json::{json, Value};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tungstenite::{client::client, Error as WsError, Message, WebSocket};

pub const WS_PORT: u16 = 8080;

pub struct NetClient {
    pub shared: Arc<NetShared>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl NetClient {
    pub fn login(host: &str, username: &str, password: &str) -> Result<(AccountSession, NetClient), String> {
        connect_auth(host, json!({ "type": "LOGIN", "username": username.trim(), "password": password }))
    }

    pub fn register(host: &str, username: &str, password: &str, tamer_name: &str) -> Result<(AccountSession, NetClient), String> {
        connect_auth(
            host,
            json!({ "type": "REGISTER", "username": username.trim(), "password": password, "tamerName": tamer_name.trim() }),
        )
    }
}

impl Drop for NetClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn connect_auth(host: &str, request: Value) -> Result<(AccountSession, NetClient), String> {
    let host = if host.trim().is_empty() { "127.0.0.1" } else { host.trim() };
    let addr_str = format!("{host}:{WS_PORT}");
    let addr = addr_str
        .to_socket_addrs()
        .map_err(|e| format!("Alamat server tidak valid: {e}"))?
        .next()
        .ok_or_else(|| "Host tidak ditemukan".to_string())?;

    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|e| format!("Gagal terhubung ke {addr_str} ({e}). Pastikan server berjalan."))?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));

    let (mut ws, _resp) = client(format!("ws://{addr_str}/"), stream).map_err(|e| format!("Handshake WebSocket gagal: {e}"))?;
    ws.send(Message::Text(request.to_string().into())).map_err(|e| format!("Gagal mengirim permintaan: {e}"))?;

    let reply = loop {
        match ws.read() {
            Ok(Message::Text(t)) => break t.to_string(),
            Ok(Message::Close(_)) => return Err("Server menutup koneksi.".to_string()),
            Ok(_) => continue,
            Err(e) => return Err(format!("Tidak ada balasan dari server: {e}")),
        }
    };
    let v: Value = serde_json::from_str(&reply).map_err(|_| "Balasan server tidak valid.".to_string())?;
    match v["type"].as_str() {
        Some("LOGIN_SUCCESS") => {}
        Some("ERROR") => return Err(v["message"].as_str().unwrap_or("Ditolak oleh server.").to_string()),
        _ => return Err("Balasan server tidak dikenal.".to_string()),
    }

    let key = hex::decode(v["sessionKey"].as_str().unwrap_or("")).map_err(|_| "Kunci sesi tidak valid.".to_string())?;
    if key.len() != 32 {
        return Err("Kunci sesi tidak valid.".to_string());
    }
    let t = &v["tamer"];
    let shared = Arc::new(NetShared::default());
    let session = AccountSession {
        server_host: host.to_string(),
        username: request["username"].as_str().unwrap_or("").to_string(),
        tamer_id: t["TamerID"].as_u64().unwrap_or(0) as u32,
        tamer_name: t["TamerName"].as_str().unwrap_or("Tamer").to_string(),
        bits: t["Bits"].as_u64().unwrap_or(0) as u32,
        prosperity: t["ProsperityPoints"].as_u64().unwrap_or(0) as u32,
        is_online: true,
        session_id: v["sessionId"].as_u64().ok_or("sessionId hilang.")? as u32,
        session_key: key,
        udp_port: v["udpPort"].as_u64().map(|p| p as u16).unwrap_or(DEFAULT_UDP_PORT),
        net: Some(shared.clone()),
    };

    // Setelah handshake, baca dengan timeout pendek supaya thread bisa dihentikan.
    let _ = ws.get_ref().set_read_timeout(Some(Duration::from_millis(100)));
    let stop = Arc::new(AtomicBool::new(false));
    let (sh, st) = (shared.clone(), stop.clone());
    let thread = std::thread::Builder::new()
        .name("mmo-ws".into())
        .spawn(move || run(ws, sh, st))
        .map_err(|e| format!("Gagal memulai thread jaringan: {e}"))?;

    Ok((session, NetClient { shared, stop, thread: Some(thread) }))
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
}

fn run(mut ws: WebSocket<TcpStream>, shared: Arc<NetShared>, stop: Arc<AtomicBool>) {
    shared.connected.store(true, Ordering::Relaxed);
    while !stop.load(Ordering::Relaxed) {
        match ws.read() {
            Ok(Message::Text(t)) => shared.apply_event(&t.to_string()),
            Ok(Message::Close(_)) => {
                shared.set_ended("Server menutup koneksi.");
                break;
            }
            Ok(_) => {} // Ping/Pong ditangani tungstenite
            Err(WsError::Io(e)) if is_timeout(&e) => {}
            Err(WsError::ConnectionClosed) | Err(WsError::AlreadyClosed) => {
                shared.set_ended("Koneksi ke server terputus.");
                break;
            }
            Err(e) => {
                shared.set_ended(&format!("Koneksi ke server terputus: {e}"));
                break;
            }
        }
        // Kirim Pong yang tertunda; timeout di sini tidak berbahaya.
        match ws.flush() {
            Ok(()) => {}
            Err(WsError::Io(e)) if is_timeout(&e) => {}
            Err(_) => {}
        }
    }
    let _ = ws.close(None);
    let _ = ws.flush();
    shared.connected.store(false, Ordering::Relaxed);
}
