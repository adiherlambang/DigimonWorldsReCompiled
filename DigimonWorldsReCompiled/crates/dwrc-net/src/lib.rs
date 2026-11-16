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
