//! Tes end-to-end: klien Rust asli melawan game-server Node asli.
//!
//! Jalankan server dulu (`cd game-server && npm start`), lalu:
//!   cargo test -p dwrc-net --test live -- --ignored --nocapture
//! Host lain: set DW1_TEST_HOST. Tes membuat akun acak di database server.

use dwrc_net::client::NetClient;
use dwrc_net::proto::{self, SeqGen};
use dwrc_net::PlayerTransform;
use std::net::UdpSocket;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn host() -> String {
    std::env::var("DW1_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".to_string())
}

fn uniq() -> String {
    format!("{:x}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros() & 0xFFFFFFFF)
}

fn tf(map: u16, x: i32, species: u16) -> PlayerTransform {
    PlayerTransform {
        player_id: 0, tamer_name: String::new(), map_id: map, x, y: 0, z: 50, yaw: 10, anim_id: 1,
        digimon_id: species, digimon_x: x + 5, digimon_y: 0, digimon_z: 55, digimon_anim: 2,
    }
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(4) {
        if f() { return; }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timeout menunggu: {what}");
}

#[test]
#[ignore]
fn rust_client_vs_node_server() {
    let h = host();
    let u = uniq();
    let (ua, ub) = (format!("ra_{u}"), format!("rb_{u}"));

    // 1) register -> sesi dengan kunci 32 byte
    let (sa, ca) = NetClient::register(&h, &ua, "secret-pass-1", &format!("RA{}", &u[..4])).expect("register A");
    let (sb, cb) = NetClient::register(&h, &ub, "secret-pass-2", &format!("RB{}", &u[..4])).expect("register B");
    assert!(sa.is_online && sa.session_id != 0 && sa.session_key.len() == 32);
    assert_ne!(sa.tamer_id, sb.tamer_id);
    assert_eq!(sa.bits, 500);

    // 2) password salah -> pesan error dari server, bukan "balasan tidak valid"
    let err = NetClient::login(&h, &ua, "salah-total-1").err().expect("harus gagal");
    assert!(err.contains("salah"), "pesan: {err}");

    // 3) UDP v2: A dan B di map yang sama saling melihat
    let (sock_a, sock_b) = (UdpSocket::bind("0.0.0.0:0").unwrap(), UdpSocket::bind("0.0.0.0:0").unwrap());
    sock_a.set_nonblocking(true).unwrap();
    sock_b.set_nonblocking(true).unwrap();
    let addr = proto::resolve_udp(&h, sa.udp_port).expect("resolve udp");
    let (mut ga, mut gb) = (SeqGen::new(), SeqGen::new());

    let send = |s: &UdpSocket, sess: &dwrc_net::AccountSession, g: &mut SeqGen, t: &PlayerTransform| {
        s.send_to(&proto::pack_sync(t, sess.session_id, &sess.session_key, g.next()), addr).unwrap();
    };

    send(&sock_a, &sa, &mut ga, &tf(0xDA, 1000, 3));
    std::thread::sleep(Duration::from_millis(80));
    send(&sock_b, &sb, &mut gb, &tf(0xDA, 2000, 7));

    // WebSocket: A diberi tahu B masuk (nama + species), B menerima roster berisi A.
    wait_for("A menerima PLAYER_JOIN B", || ca.shared.peer_count() == 1);
    wait_for("B menerima MAP_ROSTER berisi A", || cb.shared.peer_count() == 1);
    let peer = ca.shared.peers.lock().unwrap().get(&sb.tamer_id).cloned().expect("B ada di roster A");
    assert!(peer.name.starts_with("RB") && peer.species == 7, "{peer:?}");

    // UDP: A mengirim lagi -> B menerima relay dengan tamer_id dari server.
    let ip = addr.ip();
    wait_for("B menerima posisi A lewat UDP", || {
        send(&sock_a, &sa, &mut ga, &tf(0xDA, 1111, 3));
        std::thread::sleep(Duration::from_millis(30));
        proto::drain_relays(&sock_b, ip, &cb.shared);
        cb.shared.remote.lock().unwrap().contains_key(&sa.tamer_id)
    });
    {
        let g = cb.shared.remote.lock().unwrap();
        let r = &g[&sa.tamer_id];
        assert_eq!((r.map_id, r.species, r.z), (0xDA, 3, 50));
        assert!(r.x >= 1000 && r.digimon_x == r.x + 5, "{r:?}");
    }

    // 4) pemain di map lain tidak menerima apa-apa (phasing)
    send(&sock_b, &sb, &mut gb, &tf(0xCC, 5, 7));
    wait_for("A menerima PLAYER_LEAVE B", || ca.shared.peer_count() == 0);
    assert!(ca.shared.remote.lock().unwrap().get(&sb.tamer_id).is_none());

    // 5) login kedua akun yang sama menendang sesi pertama
    let (_s2, c2) = NetClient::login(&h, &ua, "secret-pass-1").expect("login ulang A");
    wait_for("sesi lama A berakhir", || ca.shared.end_reason().is_some());
    assert!(c2.shared.end_reason().is_none());

    // 6) drop klien = sesi putus; login lagi berhasil
    drop(c2);
    drop(cb);
    std::thread::sleep(Duration::from_millis(200));
    let again = NetClient::login(&h, &ub, "secret-pass-2").expect("login B setelah disconnect");
    assert_eq!(again.0.tamer_id, sb.tamer_id);
}
