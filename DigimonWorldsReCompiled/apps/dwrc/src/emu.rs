//! The emulation thread: owns the [`Session`], runs it at the console's
//! real frame rate and publishes frames and audio.

use crate::audio::AudioShared;
use dwrc_disc::GameId;
use dwrc_net::{dw1_ram, proto, AccountSession, MapTransitionFilter, PlayerTransform};
use std::net::UdpSocket;
use dwrc_mods::overlay::Overlay;
use dwrc_mods::EnabledMod;
use dwrc_psx::PadState;
use dwrc_runtime::config::Widescreen;
use dwrc_runtime::{Config, Library, Paths, Session};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct FrameBuf {
    pub pixels: Vec<u32>,
    pub width: u32,
    pub height: u32,
    pub seq: u64,
}

#[derive(Default, Clone)]
pub struct EmuStatus {
    pub running: bool,
    pub error: Option<String>,
    pub fps: f32,
    pub speed: f32,
    pub engine: String,
    pub title: String,
    pub mod_log: Vec<String>,
    pub pal: bool,
    pub bios: String,
    /// Speed a mod has set (Fast-Forward): 1 = real time.
    pub mod_speed: u32,
    /// How much wider than 4:3 to show the picture (widescreen; 1.0 = 4:3).
    pub stretch: f32,
    /// Widescreen UI is moving the game's UI (mods' images near an edge follow).
    pub ui_moved: bool,
}

pub struct Shared {
    pub frame: Mutex<FrameBuf>,
    pub pad: [AtomicU16; 2],
    pub paused: AtomicBool,
    pub fast_forward: AtomicBool,
    pub quit: AtomicBool,
    pub status: Mutex<EmuStatus>,
    /// Messages for the screen (mods, save states), taken by the UI.
    pub notices: Mutex<Vec<String>>,
    /// Images mods show over the picture, when they changed (taken by the UI).
    pub overlays: Mutex<Option<Vec<Overlay>>>,
    pub mmo_player: Mutex<Option<(PlayerTransform, String)>>,
    pub mmo_session: Mutex<AccountSession>,
    pub mmo_udp_ok: AtomicBool,
}

pub enum EmuCmd {
    Reset,
    /// A key pressed on the keyboard, for mods (`dwrc.on_key`).
    Key { key: String, shift: bool, ctrl: bool, alt: bool },
    /// The widescreen settings changed (shape, Widescreen UI).
    Widescreen(Widescreen, bool),
    /// Mouse support switched on or off.
    Mouse(bool),
    /// A click on the picture: `u`, `v` from 0 to 1 across the console's
    /// 4:3 picture.
    Click { right: bool, u: f32, v: f32 },
    /// Research: write the whole RAM and an entity summary into this folder.
    DumpRam(std::path::PathBuf),
}

pub struct EmuHandle {
    pub shared: Arc<Shared>,
    pub game: GameId,
    cmd: Sender<EmuCmd>,
    thread: Option<JoinHandle<()>>,
}

impl EmuHandle {
    pub fn start(paths: Paths, lib: Library, cfg: Config, game: GameId, mods: Vec<EnabledMod>, audio: Arc<AudioShared>) -> Self {
        let shared = Arc::new(Shared {
            frame: Mutex::new(FrameBuf::default()),
            pad: [AtomicU16::new(0), AtomicU16::new(0)],
            paused: AtomicBool::new(false),
            fast_forward: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            status: Mutex::new(EmuStatus::default()),
            notices: Mutex::new(Vec::new()),
            overlays: Mutex::new(None),
            mmo_player: Mutex::new(None),
            mmo_session: Mutex::new(AccountSession::default()),
            mmo_udp_ok: AtomicBool::new(false),
        });
        let (tx, rx) = channel();
        let sh = shared.clone();
        let thread = std::thread::Builder::new()
            .name("emulation".into())
            .stack_size(16 << 20)
            .spawn(move || run(paths, lib, cfg, game, mods, sh, rx, audio))
            .expect("spawn emulation thread");
        Self { shared, game, cmd: tx, thread: Some(thread) }
    }

    pub fn send(&self, c: EmuCmd) {
        let _ = self.cmd.send(c);
    }

    pub fn set_pad(&self, port: usize, s: PadState) {
        self.shared.pad[port].store(s.buttons, Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        self.shared.quit.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for EmuHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    paths: Paths,
    lib: Library,
    cfg: Config,
    game: GameId,
    mods: Vec<EnabledMod>,
    sh: Arc<Shared>,
    rx: Receiver<EmuCmd>,
    audio: Arc<AudioShared>,
) {
    let mut session = match Session::start(&paths, &lib, &cfg, game, mods) {
        Ok(s) => s,
        Err(e) => {
            sh.status.lock().unwrap().error = Some(format!("{e:#}"));
            return;
        }
    };
    {
        let mut st = sh.status.lock().unwrap();
        st.running = true;
        st.engine = session.psx.engine_name().to_string();
        st.title = session.ident.title.clone();
        st.mod_log = session.mods.log.clone();
        st.bios = session.bios_name.clone();
        st.stretch = session.picture_stretch();
        st.ui_moved = session.ui_moved();
    }
    audio.clear();
    let mut pixels = Vec::new();
    let mut samples = Vec::new();
    let mut target = Instant::now();
    let mut fps_t = Instant::now();
    let mut fps_n = 0u32;
    let mut emu_time = Duration::ZERO;
    let mut speed = 1u32;
    let mut frame_n = 0u64;
    let mut map_filter = MapTransitionFilter::new(8);
    // DWRC_MAP_DEBUG=1 logs every change of the raw map byte (to find what rewrites it).
    let map_debug = std::env::var_os("DWRC_MAP_DEBUG").is_some();
    let mut last_raw_map: u8 = 0xFF;
    let udp_sock = UdpSocket::bind("0.0.0.0:0").ok();
    if let Some(ref s) = udp_sock { let _ = s.set_nonblocking(true); }
    let mut seq_gen = proto::SeqGen::new();
    // Alamat UDP server di-resolve sekali per (host, port), bukan per paket.
    let mut udp_target: Option<(String, u16, std::net::SocketAddr)> = None;
    while !sh.quit.load(Ordering::Relaxed) {
        while let Ok(c) = rx.try_recv() {
            match c {
                EmuCmd::Reset => {
                    session.reset();
                    audio.clear();
                }
                EmuCmd::Key { key, shift, ctrl, alt } => session.key(&key, shift, ctrl, alt),
                EmuCmd::Mouse(on) => session.set_mouse(on),
                EmuCmd::Click { right, u, v } => session.click(right, u, v),
                EmuCmd::DumpRam(dir) => {
                    let msg = match research_dump(&session.psx.bus.ram, &dir) {
                        Ok(p) => format!("RAM dump saved: {}", p.display()),
                        Err(e) => format!("RAM dump failed: {e}"),
                    };
                    sh.notices.lock().unwrap().push(msg);
                }
                EmuCmd::Widescreen(w, ui) => {
                    session.set_widescreen(w, ui);
                    let mut st = sh.status.lock().unwrap();
                    st.stretch = session.picture_stretch();
                    st.ui_moved = session.ui_moved();
                }
            }
        }
        let notices = session.take_notices();
        if !notices.is_empty() {
            sh.notices.lock().unwrap().extend(notices);
        }
        if let Some(o) = session.take_overlays() {
            *sh.overlays.lock().unwrap() = Some(o);
        }
        if sh.paused.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
            target = Instant::now();
            continue;
        }
        let p0 = PadState { buttons: sh.pad[0].load(Ordering::Relaxed) };
        session.set_pad(0, p0);
        session.run_frame();
        frame_n += 1;
        if session.ident.serial == "SLUS-01032" && frame_n % 3 == 0 {
            let sess = sh.mmo_session.lock().unwrap().clone();
            if sess.is_online {
                let same = udp_target.as_ref().map(|(h, p, _)| (h.as_str(), *p)) == Some((sess.server_host.as_str(), sess.udp_port));
                if !same {
                    udp_target = proto::resolve_udp(&sess.server_host, sess.udp_port).map(|a| (sess.server_host.clone(), sess.udp_port, a));
                }
                // Terima posisi pemain lain (tidak perlu menunggu map stabil).
                if let (Some(sock), Some((_, _, addr)), Some(net)) = (&udp_sock, &udp_target, &sess.net) {
                    proto::drain_relays(sock, addr.ip(), net);
                    net.expire(Duration::from_secs(3));
                }
            }
            if map_debug {
                let raw = dw1_ram::read_u8(&session.psx.bus.ram, dw1_ram::slus_01032::CURRENT_MAP_ID);
                if raw != last_raw_map {
                    map_debug_log(&format!(
                        "frame {frame_n}: 0x{last_raw_map:02X} -> 0x{raw:02X} (calm={}, game_state={})",
                        dw1_ram::is_calm(&session.psx.bus.ram),
                        dw1_ram::read_u8(&session.psx.bus.ram, dw1_ram::slus_01032::GAME_STATE)
                    ));
                    last_raw_map = raw;
                }
            }
            let mut tf = dw1_ram::extract_player_transform(&session.psx.bus.ram, sess.tamer_id, &sess.tamer_name);
            let calm = dw1_ram::is_calm(&session.psx.bus.ram);
            if let Some(stable_map) = map_filter.update(&tf, calm) {
                tf.map_id = stable_map;
                let area = dw1_ram::area_name_for(&session.psx.bus.ram, stable_map);
                if sess.is_online {
                    if let (Some(sock), Some((_, _, addr))) = (&udp_sock, &udp_target) {
                        let pkt = proto::pack_sync(&tf, sess.session_id, &sess.session_key, seq_gen.next());
                        let sent = sock.send_to(&pkt, *addr).is_ok();
                        sh.mmo_udp_ok.store(sent, Ordering::Relaxed);
                    }
                }
                *sh.mmo_player.lock().unwrap() = Some((tf, area));
            } else if tf.map_id == 0x00FF {
                *sh.mmo_player.lock().unwrap() = None;
            }
        }
        if session.speed() != speed {
            speed = session.speed();
            sh.status.lock().unwrap().mod_speed = speed;
        }
        // Sped up, only every speed-th picture can be seen anyway.
        if speed == 1 || frame_n % speed as u64 == 0 {
            let (w, h) = session.frame_pixels(&mut pixels);
            let mut f = sh.frame.lock().unwrap();
            std::mem::swap(&mut f.pixels, &mut pixels);
            f.width = w;
            f.height = h;
            f.seq += 1;
        }
        // Sped up by a mod, the sound chip still delivers real-time audio.
        samples.clear();
        session.take_audio(&mut samples);
        let ff = sh.fast_forward.load(Ordering::Relaxed);
        if !ff {
            audio.push(&samples);
        }
        // Pace to the console's own frame timing.
        let ft = session.last_frame_time;
        emu_time += ft;
        fps_n += 1;
        if fps_t.elapsed() >= Duration::from_millis(500) {
            let el = fps_t.elapsed();
            let mut st = sh.status.lock().unwrap();
            st.fps = fps_n as f32 / el.as_secs_f32();
            st.speed = emu_time.as_secs_f32() / el.as_secs_f32();
            st.pal = session.is_pal();
            st.mod_log = session.mods.log.clone();
            fps_t = Instant::now();
            fps_n = 0;
            emu_time = Duration::ZERO;
        }
        if ff {
            target = Instant::now();
            continue;
        }
        target += ft / speed;
        let now = Instant::now();
        if target > now {
            std::thread::sleep(target - now);
        } else if now - target > Duration::from_millis(250) {
            target = now; // fell far behind (e.g. window dragged); resync
        }
    }
    session.shutdown();
    sh.status.lock().unwrap().running = false;
}

/// Appends a line to `dwrc-map-debug.log` in the working directory. The Windows
/// release build has no console, so `eprintln!` would be invisible there.
fn map_debug_log(line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("dwrc-map-debug.log") {
        let _ = writeln!(f, "[map] {line}");
    }
}

/// Writes `dump-<time>-map<XX>.ram` (all 2 MiB of console RAM) and `.txt` (decoded summary).
fn research_dump(ram: &[u8], dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let map = dw1_ram::read_u8(ram, dw1_ram::slus_01032::CURRENT_MAP_ID);
    let stem = dir.join(format!("dump-{ts}-map{map:02X}"));
    std::fs::write(stem.with_extension("ram"), ram)?;
    std::fs::write(stem.with_extension("txt"), dw1_ram::research_report(ram))?;
    Ok(stem.with_extension("ram"))
}
