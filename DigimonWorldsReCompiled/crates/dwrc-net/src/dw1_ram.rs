pub const PS1_RAM_MASK: u32 = 0x001F_FFFF;

pub mod slus_01032 {
    pub const GP_REG: u32 = 0x8013_BD68;
    pub const CURRENT_MAP_ID: u32 = 0x8013_4FE4; // gp - 28036 (0xDA = Jijimon's house, 0xCC = File City)
    pub const MAP_DATA_TABLE: u32 = 0x8012_92D4; // 16 bytes per map entry; byte +15 = area name index
    pub const AREA_NAME_PTRS: u32 = 0x8012_91BC; // u32* array of C-strings in PS1 RAM

    // Same variables the mouse support reads (`dwrc-runtime/src/mouse/dw1.rs`).
    pub const GAME_STATE: u32 = 0x8013_4F0A; // u8: 1 = battle, 4 = tournament / Vs fight
    pub const IS_IN_MENU: u32 = 0x8013_4EF0; // u32, non-zero while a menu is open
    pub const UI_BOX_DATA: u32 = 0x8013_D390; // 4 boxes, 0x24 bytes each; s16 state at +0x12
    pub const UI_BOX_SIZE: u32 = 0x24;

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

/// True while the player is simply walking around: no battle, no menu and no
/// text box (an NPC conversation opens one). While it is false the game is
/// free to reuse variables such as the map byte, so the map is not trusted.
pub fn is_calm(ram: &[u8]) -> bool {
    use slus_01032 as a;
    if matches!(read_u8(ram, a::GAME_STATE), 1 | 4) {
        return false;
    }
    if read_u32(ram, a::IS_IN_MENU) != 0 {
        return false;
    }
    !(0..4).any(|i| read_u16(ram, a::UI_BOX_DATA + i * a::UI_BOX_SIZE + 0x12) != 0)
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

/// Name of the area a map ID belongs to, read from the game's own tables in
/// `ram` and formatted as `"Name (0xID)"`. It is computed from the ID it is
/// given (the debounced one), never from whatever the game wrote last, so the
/// label and the ID can no longer disagree during a transition.
pub fn area_name_for(ram: &[u8], map_id: u16) -> String {
    use slus_01032 as a;
    let entry_addr = a::MAP_DATA_TABLE + (map_id as u32) * 16;
    let area_idx = read_u8(ram, entry_addr + 15) as u32;
    let str_ptr = read_u32(ram, a::AREA_NAME_PTRS + area_idx * 4);
    match read_cstr(ram, str_ptr, 40) {
        Some(name) => format!("{name} (0x{map_id:02X})"),
        None => format!("Unknown Area (0x{map_id:02X})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::slus_01032 as a;

    /// A fake RAM with one area name, and two map IDs that point to different areas.
    fn fake_ram() -> Vec<u8> {
        let mut ram = vec![0u8; 0x20_0000];
        let put_u32 = |ram: &mut Vec<u8>, addr: u32, v: u32| {
            let o = (addr & PS1_RAM_MASK) as usize;
            ram[o..o + 4].copy_from_slice(&v.to_le_bytes());
        };
        let name_a = 0x8010_0000u32;
        let name_b = 0x8010_0100u32;
        for (addr, text) in [(name_a, b"File City\0".as_slice()), (name_b, b"Tropical Jungle\0".as_slice())] {
            let o = (addr & PS1_RAM_MASK) as usize;
            ram[o..o + text.len()].copy_from_slice(text);
        }
        put_u32(&mut ram, a::AREA_NAME_PTRS, name_a);
        put_u32(&mut ram, a::AREA_NAME_PTRS + 4, name_b);
        ram[(a::MAP_DATA_TABLE & PS1_RAM_MASK) as usize + 0xCC * 16 + 15] = 0;
        ram[(a::MAP_DATA_TABLE & PS1_RAM_MASK) as usize + 0x0B * 16 + 15] = 1;
        ram
    }

    #[test]
    fn calm_is_false_for_menus_boxes_and_battles() {
        let mut ram = vec![0u8; 0x20_0000];
        assert!(is_calm(&ram));
        ram[(a::GAME_STATE & PS1_RAM_MASK) as usize] = 1;
        assert!(!is_calm(&ram));
        ram[(a::GAME_STATE & PS1_RAM_MASK) as usize] = 0;
        ram[(a::IS_IN_MENU & PS1_RAM_MASK) as usize] = 1;
        assert!(!is_calm(&ram));
        ram[(a::IS_IN_MENU & PS1_RAM_MASK) as usize] = 0;
        ram[((a::UI_BOX_DATA + 2 * a::UI_BOX_SIZE + 0x12) & PS1_RAM_MASK) as usize] = 3;
        assert!(!is_calm(&ram));
    }

    #[test]
    fn area_name_follows_the_id_it_is_given() {
        let ram = fake_ram();
        assert_eq!(area_name_for(&ram, 0xCC), "File City (0xCC)");
        assert_eq!(area_name_for(&ram, 0x0B), "Tropical Jungle (0x0B)");
    }

    #[test]
    fn unreadable_name_is_reported_not_guessed() {
        let ram = vec![0u8; 0x20_0000];
        assert_eq!(area_name_for(&ram, 0x42), "Unknown Area (0x42)");
    }
}
