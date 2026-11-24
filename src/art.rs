//! Art LST tables, FID paths, and the direction-frame selector.

use crate::dat2::GameFiles;
use crate::direction::pick_direction;
use crate::frm::{Frm, decode_frm};

pub const ART_DIR: [&str; 11] = [
    "art\\items",
    "art\\critters",
    "art\\scenery",
    "art\\walls",
    "art\\tiles",
    "art\\misc",
    "art\\intrface",
    "art\\inven",
    "art\\heads",
    "art\\backgrnd",
    "art\\skilldex",
];

const WEAPON_LETTER: [&str; 11] = ["a", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m"];

pub fn parse_lst(data: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(data);
    text.split(['\n', '\r'])
        .filter(|line| !line.is_empty() || text.contains('\n'))
        .map(|line| {
            line.split(|ch: char| ch.is_whitespace() || ch == ',' || ch == ';')
                .next()
                .unwrap_or("")
                .to_string()
        })
        .filter(|line| {
            // splitlines drops a trailing empty line; keep physical lines that had content markers
            true || !line.is_empty()
        })
        .collect::<Vec<_>>()
        .into_iter()
        .collect()
}

pub fn parse_lst_lines(data: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(data);
    let mut lines = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        let token = line
            .split(|ch: char| ch.is_whitespace() || ch == ',' || ch == ';')
            .next()
            .unwrap_or("");
        lines.push(token.to_string());
    }
    if lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
}

pub fn fid_type(fid: u32) -> u32 {
    (fid >> 24) & 0xF
}

pub fn fid_index(fid: u32) -> u32 {
    fid & 0xFFF
}

pub fn fid_animation(fid: u32) -> u32 {
    (fid >> 16) & 0xFF
}

pub fn fid_weapon(fid: u32) -> u32 {
    (fid >> 12) & 0xF
}

pub fn fid_rotation(fid: u32) -> u32 {
    (fid >> 28) & 0x7
}

pub fn critter_suffix(animation: u32, weapon: u32) -> String {
    if (20..=35).contains(&animation) {
        return format!("b{}", char::from(b'a' + (animation - 20) as u8));
    }
    if (48..=63).contains(&animation) {
        return format!("r{}", char::from(b'a' + (animation - 48) as u8));
    }
    let letter = WEAPON_LETTER.get(weapon as usize).copied().unwrap_or("a");
    if weapon == 0 {
        if animation <= 25 {
            return format!("a{}", char::from(b'a' + animation as u8));
        }
        return "aa".into();
    }
    if animation == 0 {
        return format!("{letter}a");
    }
    if animation == 1 {
        return format!("{letter}b");
    }
    if (2..=19).contains(&animation) {
        return format!("{letter}{}", char::from(b'c' + (animation - 2) as u8));
    }
    format!("{letter}a")
}

pub fn critter_extension(rotation_field: u32) -> String {
    if rotation_field == 0 {
        ".frm".into()
    } else {
        format!(".fr{}", rotation_field - 1)
    }
}

pub fn direction_stem(path: &str) -> String {
    match path.rfind('.') {
        Some(dot) => {
            let ext = path[dot..].to_ascii_lowercase();
            let split = ext == ".frm"
                || (ext.len() == 4 && ext.starts_with(".fr") && ext.as_bytes()[3].is_ascii_digit() && ext.as_bytes()[3] <= b'5');
            if split { path[..dot].to_string() } else { path.to_string() }
        }
        None => path.to_string(),
    }
}

pub fn select_direction_art(
    files: &GameFiles,
    path: &str,
    facing: i32,
    corner: i32,
) -> Result<(String, usize, bool), String> {
    let stem = direction_stem(path);
    let frm_path = format!("{stem}.frm");
    if files.contains(&frm_path) {
        let frm = decode_frm(&files.read(&frm_path)?)?;
        let stored: Vec<i32> = frm.stored_directions.iter().map(|value| *value as i32).collect();
        if stored.len() > 1 {
            let chosen = pick_direction(&stored, facing, corner)? as usize;
            return Ok((frm_path, chosen, true));
        }
        if let Some(&only) = stored.first() {
            return Ok((frm_path, only as usize, false));
        }
    }
    let available: Vec<i32> = (0..6).filter(|index| files.contains(&format!("{stem}.fr{index}"))).collect();
    if !available.is_empty() {
        let chosen = pick_direction(&available, facing, corner)?;
        return Ok((format!("{stem}.fr{chosen}"), 0, available.len() > 1));
    }
    Err(format!("no art for {path}"))
}

pub struct ArtIndex {
    lists: Vec<Vec<String>>,
}

impl ArtIndex {
    pub fn load(files: &GameFiles) -> Result<Self, String> {
        let mut lists = Vec::with_capacity(ART_DIR.len());
        for folder in ART_DIR {
            let leaf = folder.rsplit('\\').next().unwrap_or(folder);
            let path = format!("{folder}\\{leaf}.lst");
            lists.push(parse_lst_lines(&files.read(&path)?));
        }
        Ok(Self { lists })
    }

    pub fn resolve(&self, fid: u32) -> Result<String, String> {
        let typ = fid_type(fid) as usize;
        let entries = self.lists.get(typ).ok_or_else(|| format!("FID type {typ} is not an art list"))?;
        let index = fid_index(fid) as usize;
        let name = entries.get(index).filter(|name| !name.is_empty()).ok_or_else(|| {
            format!("FID {fid:#x} index {index} is outside the art list")
        })?;
        if typ == 1 {
            let suffix = critter_suffix(fid_animation(fid), fid_weapon(fid));
            let ext = critter_extension(fid_rotation(fid));
            return Ok(format!("art\\critters\\{name}{suffix}{ext}"));
        }
        let file_name = if name.contains('.') { name.clone() } else { format!("{name}.frm") };
        Ok(format!("{}\\{file_name}", ART_DIR[typ]))
    }

    pub fn resolve_tile(&self, art_index: i32) -> Result<String, String> {
        let entries = &self.lists[4];
        if art_index < 0 {
            return Err(format!("tile art {art_index}"));
        }
        let name = entries
            .get(art_index as usize)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("tile art {art_index}"))?;
        let file_name = if name.contains('.') { name.clone() } else { format!("{name}.frm") };
        Ok(format!("art\\tiles\\{file_name}"))
    }
}

pub fn load_frm(files: &GameFiles, path: &str) -> Result<Frm, String> {
    decode_frm(&files.read(path)?)
}
