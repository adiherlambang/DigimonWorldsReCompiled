use std::sync::Mutex;

pub const PS1_RAM_MASK: u32 = 0x001F_FFFF;

pub mod slus_01032 {
    pub const GP_REG: u32 = 0x8013_BD68;
    pub const CURRENT_MAP_ID: u32 = 0x8013_4FE4; // gp - 28036 (0xDA = Jijimon's house, 0xCC = File City)
    pub const MAP_DATA_TABLE: u32 = 0x8012_92D4; // 16 bytes per map entry; byte +15 = area name index
    pub const AREA_NAME_PTRS: u32 = 0x8012_91BC; // u32* array of C-strings in PS1 RAM

    pub const TAMER_ENTITY: u32 = 0x8015_576C;
    pub const PARTNER_ENTITY: u32 = 0x8015_57A8;
    pub const TAMER_POS_STRUCT_PTR: u32 = 0x8015_5770;
    pub const PARTNER_POS_STRUCT_PTR: u32 = 0x8015_57AC;

    pub const TRANSFORM_POS_X: u32 = 0x78;
    pub const TRANSFORM_POS_Y: u32 = 0x7C;
    pub const TRANSFORM_POS_Z: u32 = 0x80;
    pub const TRANSFORM_YAW: u32 = 0x8A;

    pub const ENT_ANIM_ID: u32 = 0x0E;
    pub const PARTNER_SPECIES_ID: u32 = 0x00;
}

static NATIVE_MAP_NAME: Mutex<String> = Mutex::new(String::new());

#[inline]
pub fn is_valid_ps1_ptr(vaddr: u32) -> bool {
    let top = vaddr & 0xFF00_0000;
    let off = vaddr & 0x00FF_FFFF;
    (top == 0x8000_0000 || top == 0x0000_0000 || top == 0xA000_0000)
        && off >= 0x0001_0000
        && off < 0x001F_FE00
}

#[inline]
pub fn read_u8(ram: &[u8], vaddr: u32) -> u8 {
    let off = (vaddr & PS1_RAM_MASK) as usize;
    ram.get(off).copied().unwrap_or(0)
}

#[inline]
pub fn read_u16(ram: &[u8], vaddr: u32) -> u16 {
    let off = (vaddr & PS1_RAM_MASK) as usize;
    if off + 2 > ram.len() { return 0; }
    u16::from_le_bytes([ram[off], ram[off + 1]])
}

#[inline]
pub fn read_i16(ram: &[u8], vaddr: u32) -> i16 {
    read_u16(ram, vaddr) as i16
}

#[inline]
pub fn read_i32(ram: &[u8], vaddr: u32) -> i32 {
    let off = (vaddr & PS1_RAM_MASK) as usize;
    if off + 4 > ram.len() { return 0; }
    i32::from_le_bytes([ram[off], ram[off + 1], ram[off + 2], ram[off + 3]])
}

#[inline]
pub fn read_u32(ram: &[u8], vaddr: u32) -> u32 {
    read_i32(ram, vaddr) as u32
}

pub fn read_cstr(ram: &[u8], vaddr: u32, max_len: usize) -> Option<String> {
    if !is_valid_ps1_ptr(vaddr) {
        return None;
    }
    let start = (vaddr & PS1_RAM_MASK) as usize;
    let mut bytes = Vec::new();
    for i in 0..max_len {
        let b = *ram.get(start + i)?;
        if b == 0 {
            break;
        }
        if (0x20..=0x7E).contains(&b) {
            bytes.push(b);
        } else {
            return None;
        }
    }
    if bytes.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(&bytes).trim().to_string())
    }
}

pub fn extract_player_transform(ram: &[u8], player_id: u32, tamer_name: &str) -> crate::PlayerTransform {
    use slus_01032 as a;

    let tamer_ptr = read_u32(ram, a::TAMER_POS_STRUCT_PTR);
    let (x, y, z, yaw) = if is_valid_ps1_ptr(tamer_ptr) {
        (
            read_i32(ram, tamer_ptr + a::TRANSFORM_POS_X),
            read_i32(ram, tamer_ptr + a::TRANSFORM_POS_Y),
            read_i32(ram, tamer_ptr + a::TRANSFORM_POS_Z),
            read_i16(ram, tamer_ptr + a::TRANSFORM_YAW),
        )
    } else {
        (0, 0, 0, 0)
    };

    let partner_ptr = read_u32(ram, a::PARTNER_POS_STRUCT_PTR);
    let (digimon_x, digimon_y, digimon_z) = if is_valid_ps1_ptr(partner_ptr) {
        (
            read_i32(ram, partner_ptr + a::TRANSFORM_POS_X),
            read_i32(ram, partner_ptr + a::TRANSFORM_POS_Y),
            read_i32(ram, partner_ptr + a::TRANSFORM_POS_Z),
        )
    } else {
        (x, y, z)
    };

    let anim_id = read_u8(ram, a::TAMER_ENTITY + a::ENT_ANIM_ID);
    let digimon_anim = read_u8(ram, a::PARTNER_ENTITY + a::ENT_ANIM_ID);
    let sp_raw = read_u16(ram, a::PARTNER_ENTITY + a::PARTNER_SPECIES_ID);
    let digimon_id = if sp_raw > 0 && sp_raw < 256 {
        sp_raw
    } else {
        read_u8(ram, a::PARTNER_ENTITY + a::PARTNER_SPECIES_ID) as u16
    };

    let raw_map_id = read_u8(ram, a::CURRENT_MAP_ID) as u16;
    let in_game = is_valid_ps1_ptr(tamer_ptr) && (x != 0 || y != 0 || z != 0);
    let map_id = if in_game { raw_map_id } else { 0x00FF };

    let native_name = if !in_game {
        "Title / Menu".to_string()
    } else {
        let entry_addr = a::MAP_DATA_TABLE + (map_id as u32) * 16;
        let area_idx = read_u8(ram, entry_addr + 15) as u32;
        let str_ptr = read_u32(ram, a::AREA_NAME_PTRS + area_idx * 4);
        read_cstr(ram, str_ptr, 40).unwrap_or_default()
    };

    if let Ok(mut guard) = NATIVE_MAP_NAME.lock() {
        *guard = native_name;
    }

    crate::PlayerTransform {
        player_id,
        tamer_name: tamer_name.to_string(),
        map_id,
        x,
        y,
        z,
        yaw,
        anim_id,
        digimon_id,
        digimon_x,
        digimon_y,
        digimon_z,
        digimon_anim,
    }
}

pub fn map_id_to_name(map_id: u16) -> String {
    if let Ok(guard) = NATIVE_MAP_NAME.lock() {
        if !guard.is_empty() {
            return format!("{} (0x{:02X})", *guard, map_id);
        }
    }
    format!("Unknown Area (0x{:02X})", map_id)
}


pub fn pack_udp_sync(tf: &crate::PlayerTransform) -> [u8; 32] {
    let mut buf = [0u8; 32];
    buf[0] = 0x01;
    buf[1] = (tf.map_id & 0xFF) as u8;
    buf[2..4].copy_from_slice(&tf.digimon_id.to_le_bytes());
    buf[4..8].copy_from_slice(&tf.x.to_le_bytes());
    buf[8..12].copy_from_slice(&tf.y.to_le_bytes());
    buf[12..16].copy_from_slice(&tf.z.to_le_bytes());
    buf[16..18].copy_from_slice(&tf.yaw.to_le_bytes());
    buf[18] = tf.anim_id;
    buf[19] = tf.digimon_anim;
    buf[20..24].copy_from_slice(&tf.digimon_x.to_le_bytes());
    buf[24..28].copy_from_slice(&tf.digimon_z.to_le_bytes());
    buf[28..32].copy_from_slice(&tf.player_id.to_le_bytes());
    buf
}
