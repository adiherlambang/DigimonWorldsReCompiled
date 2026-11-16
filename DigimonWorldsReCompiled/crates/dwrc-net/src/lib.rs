pub mod client;
pub mod dw1_ram;
pub mod proto;
pub mod state;

use std::sync::Arc;
use state::NetShared;

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
    /// Dari LOGIN_SUCCESS; dipakai menandatangani paket UDP.
    pub session_id: u32,
    pub session_key: Vec<u8>,
    pub udp_port: u16,
    /// State jaringan bersama (pemain lain, status koneksi). `None` saat offline.
    pub net: Option<Arc<NetShared>>,
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
            session_id: 0,
            session_key: Vec::new(),
            udp_port: proto::DEFAULT_UDP_PORT,
            net: None,
        }
    }
}

/// Decides which map the player is really in.
///
/// The map byte in RAM is only meaningful while the player walks around. Talking
/// to an NPC, a menu, a battle and the loading screens all reuse or scramble it,
/// so a new value is believed only when
///   * the game is calm (no menu, text box or battle),
///   * a load boundary was crossed since the last accepted map (the player
///     disappeared from the world, or jumped to a far-away spot), and
///   * it then stays the same for `required_ticks` ticks in a row.
/// Anything else keeps the last accepted map.
pub struct MapTransitionFilter {
    stable_map_id: u16,
    pending_map_id: u16,
    stable_ticks: u8,
    required_ticks: u8,
    armed: bool,
    last_pos: Option<(i32, i32, i32)>,
}

/// A step between two ticks (3 frames) longer than this is a teleport, not walking.
const TELEPORT_DISTANCE: i64 = 400;

impl MapTransitionFilter {
    pub fn new(required_ticks: u8) -> Self {
        Self {
            stable_map_id: 0x00FF,
            pending_map_id: 0x00FF,
            stable_ticks: 0,
            required_ticks,
            armed: false,
            last_pos: None,
        }
    }

    fn current(&self) -> Option<u16> {
        (self.stable_map_id != 0x00FF).then_some(self.stable_map_id)
    }

    /// `Some(map)` is the map to use this tick; `None` means the player is not
    /// in the world (title screen, loading) or no map has been established yet.
    pub fn update(&mut self, current: &PlayerTransform, calm: bool) -> Option<u16> {
        let in_world = current.map_id != 0x00FF && !(current.x == 0 && current.y == 0 && current.z == 0);
        if !in_world {
            // The player left the world: this is a load boundary.
            self.armed = true;
            self.pending_map_id = 0x00FF;
            self.stable_ticks = 0;
            self.last_pos = None;
            return None;
        }
        let pos = (current.x, current.y, current.z);
        if let Some(p) = self.last_pos {
            let d = (pos.0 as i64 - p.0 as i64).abs() + (pos.2 as i64 - p.2 as i64).abs();
            if d > TELEPORT_DISTANCE {
                self.armed = true;
            }
        }
        self.last_pos = Some(pos);

        if !calm {
            self.pending_map_id = 0x00FF;
            self.stable_ticks = 0;
            return self.current();
        }

        let first = self.stable_map_id == 0x00FF;
        if !first && current.map_id == self.stable_map_id {
            // Same map as before: once it has held, the boundary is settled.
            self.pending_map_id = 0x00FF;
            self.stable_ticks = self.stable_ticks.saturating_add(1);
            if self.stable_ticks >= self.required_ticks {
                self.armed = false;
            }
            return self.current();
        }
        if !first && !self.armed {
            // A different value with no load in between: the game reused the byte.
            self.pending_map_id = 0x00FF;
            self.stable_ticks = 0;
            return self.current();
        }
        if current.map_id == self.pending_map_id {
            self.stable_ticks = self.stable_ticks.saturating_add(1);
        } else {
            self.pending_map_id = current.map_id;
            self.stable_ticks = 1;
        }
        if self.stable_ticks >= self.required_ticks {
            self.stable_map_id = self.pending_map_id;
            self.armed = false;
        }
        self.current()
    }
}

#[cfg(test)]
mod map_filter_tests {
    use super::*;

    fn tf(map: u16, x: i32) -> PlayerTransform {
        PlayerTransform {
            player_id: 1,
            tamer_name: String::new(),
            map_id: map,
            x,
            y: 0,
            z: 100,
            yaw: 0,
            anim_id: 0,
            digimon_id: 0,
            digimon_x: 0,
            digimon_y: 0,
            digimon_z: 0,
            digimon_anim: 0,
        }
    }

    fn feed(f: &mut MapTransitionFilter, map: u16, x: i32, calm: bool, n: usize) -> Option<u16> {
        let mut last = None;
        for _ in 0..n {
            last = f.update(&tf(map, x), calm);
        }
        last
    }

    #[test]
    fn first_map_needs_calm_stable_ticks() {
        let mut f = MapTransitionFilter::new(8);
        assert_eq!(feed(&mut f, 0xCC, 500, true, 7), None);
        assert_eq!(feed(&mut f, 0xCC, 500, true, 1), Some(0xCC));
    }

    #[test]
    fn byte_changing_without_a_load_is_ignored() {
        let mut f = MapTransitionFilter::new(8);
        feed(&mut f, 0xCC, 500, true, 20);
        // Talking to an NPC (text box open) and the byte becomes 0x0B.
        assert_eq!(feed(&mut f, 0x0B, 502, false, 50), Some(0xCC));
        // Even once calm again, nothing crossed a load boundary.
        assert_eq!(feed(&mut f, 0x0B, 502, true, 50), Some(0xCC));
    }

    #[test]
    fn leaving_the_world_arms_a_real_change() {
        let mut f = MapTransitionFilter::new(8);
        feed(&mut f, 0xCC, 500, true, 20);
        // Loading screen: the player is not in the world (map 0xFF).
        assert_eq!(f.update(&tf(0xFF, 0), true), None);
        assert_eq!(feed(&mut f, 0xDA, 300, true, 7), Some(0xCC));
        assert_eq!(feed(&mut f, 0xDA, 300, true, 1), Some(0xDA));
        // The boundary is spent: a stray value afterwards is ignored again.
        assert_eq!(feed(&mut f, 0x0B, 300, true, 30), Some(0xDA));
    }

    #[test]
    fn a_teleport_also_arms_a_change() {
        let mut f = MapTransitionFilter::new(8);
        feed(&mut f, 0xCC, 500, true, 20);
        f.update(&tf(0xDA, 5000), true);
        assert_eq!(feed(&mut f, 0xDA, 5000, true, 8), Some(0xDA));
    }

    #[test]
    fn garbage_during_loading_never_sticks() {
        let mut f = MapTransitionFilter::new(8);
        feed(&mut f, 0xCC, 500, true, 20);
        f.update(&tf(0xFF, 0), true);
        // Flickering values while loading: no value holds 8 ticks in a row.
        for i in 0..40u16 {
            f.update(&tf(if i % 2 == 0 { 0x0B } else { 0x33 }, 300), true);
        }
        assert_eq!(feed(&mut f, 0xCC, 300, true, 8), Some(0xCC));
    }
}
