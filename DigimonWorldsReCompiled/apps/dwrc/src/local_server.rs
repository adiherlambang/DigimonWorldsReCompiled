//! Starts the bundled Node.js game server (`game-server/`) when the launcher
//! opens, in a console window of its own so its log is visible.
//!
//! Nothing is started when something already answers on the WebSocket port
//! (a server you started yourself), when `game-server/server.js` cannot be
//! found next to the program, or when `DWRC_NO_SERVER` is set.

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

const WS_PORT: u16 = 8080;

#[derive(Clone, PartialEq, Eq)]
pub enum ServerState {
    /// Started by the launcher; its console window shows the log.
    Started,
    /// Something was already listening on the port.
    AlreadyRunning,
    /// `game-server/server.js` was not found.
    NotFound,
    /// Switched off with `DWRC_NO_SERVER`.
    Disabled,
    Failed(String),
}

pub struct LocalServer {
    child: Option<Child>,
    pub state: ServerState,
}

fn port_open() -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], WS_PORT));
    TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
}

/// Looks for `game-server/server.js` in the program's folder and the working
/// directory, then in their parents (the repository keeps it beside the
/// workspace folder).
fn find_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("DWRC_SERVER_DIR") {
        let d = PathBuf::from(d);
        return d.join("server.js").is_file().then_some(d);
    }
    let mut starts: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            starts.push(p.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    for start in starts {
        for dir in start.ancestors().take(6) {
            let cand = dir.join("game-server");
            if cand.join("server.js").is_file() {
                return Some(cand);
            }
        }
    }
    None
}

impl LocalServer {
    pub fn start() -> Self {
        if std::env::var_os("DWRC_NO_SERVER").is_some() {
            return Self { child: None, state: ServerState::Disabled };
        }
        if port_open() {
            return Self { child: None, state: ServerState::AlreadyRunning };
        }
        let Some(dir) = find_dir() else {
            return Self { child: None, state: ServerState::NotFound };
        };
        match spawn(&dir) {
            Ok(child) => Self { child: Some(child), state: ServerState::Started },
            Err(e) => Self { child: None, state: ServerState::Failed(e.to_string()) },
        }
    }

    pub fn stop(&mut self) {
        let Some(mut child) = self.child.take() else { return };
        #[cfg(windows)]
        {
            // `cmd` is the child; take its whole tree (node) down with it.
            let _ = Command::new("taskkill").args(["/PID", &child.id().to_string(), "/T", "/F"]).output();
        }
        #[cfg(not(windows))]
        {
            let _ = child.kill();
        }
        let _ = child.wait();
    }

    pub fn status_text(&self) -> String {
        match &self.state {
            ServerState::Started => "Server lokal dijalankan (lihat jendela “DWRC Game Server”).".into(),
            ServerState::AlreadyRunning => "Server sudah berjalan di port 8080.".into(),
            ServerState::NotFound => "Folder game-server tidak ditemukan; jalankan server secara manual.".into(),
            ServerState::Disabled => "Server lokal dimatikan (DWRC_NO_SERVER).".into(),
            ServerState::Failed(e) => format!("Server lokal gagal dijalankan: {e}"),
        }
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The command that runs in the server's console: install the dependencies the
/// first time, then start the server.
fn script(dir: &Path) -> &'static str {
    if dir.join("node_modules").is_dir() {
        "node server.js"
    } else {
        "npm install --no-audit --no-fund && node server.js"
    }
}

#[cfg(windows)]
fn spawn(dir: &Path) -> std::io::Result<Child> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    // /K keeps the window open if the server stops, so an error stays readable.
    Command::new("cmd")
        .current_dir(dir)
        .arg("/K")
        .raw_arg(format!("\"title DWRC Game Server && {}\"", script(dir)))
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
}

#[cfg(not(windows))]
fn spawn(dir: &Path) -> std::io::Result<Child> {
    // The log goes to the terminal the launcher was started from.
    Command::new("sh").current_dir(dir).args(["-c", script(dir)]).spawn()
}
