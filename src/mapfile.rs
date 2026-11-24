//! Fallout 2 MAP loader. Integers are big-endian. Version 20 is 0x00000014.

use crate::dat2::{GameFiles, normalize_path};

pub const HEADER_SIZE: usize = 0xEC;
const SQUARE_COUNT: usize = 100 * 100;
const ELEVATIONS: usize = 3;
const ELEVATION_ABSENT: [i32; 3] = [0x2, 0x4, 0x8];

const PROTO_LST: [&str; 6] = [
    "proto\\items\\items.lst",
    "proto\\critters\\critters.lst",
    "proto\\scenery\\scenery.lst",
    "proto\\walls\\walls.lst",
    "proto\\tiles\\tiles.lst",
    "proto\\misc\\misc.lst",
];
const PROTO_DIR: [&str; 6] = [
    "proto\\items",
    "proto\\critters",
    "proto\\scenery",
    "proto\\walls",
    "proto\\tiles",
    "proto\\misc",
];

const EXIT_GRID_LO: u32 = 0x0500_0010;
const EXIT_GRID_HI: u32 = 0x0500_0017;

pub const OBJECT_HIDDEN: i32 = 0x0000_0001;
pub const OBJECT_FLAT: i32 = 0x0000_0008;
pub const OBJECT_NO_BLOCK: i32 = 0x0000_0010;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn i32(&mut self) -> Result<i32, String> {
        if self.pos + 4 > self.data.len() {
            return Err(format!("MAP ended at {}, need 4 more bytes", self.pos));
        }
        let value = i32::from_be_bytes(self.data[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(value)
    }

    fn raw(&mut self, size: usize) -> Result<&'a [u8], String> {
        if self.pos + size > self.data.len() {
            return Err(format!("MAP ended at {}, need {size} more bytes", self.pos));
        }
        let chunk = &self.data[self.pos..self.pos + size];
        self.pos += size;
        Ok(chunk)
    }
}

#[derive(Clone, Debug)]
pub struct FloorCell {
    pub square: i32,
    pub art_index: i32,
}

#[derive(Clone, Debug)]
pub struct PlacedObject {
    pub id: i32,
    pub hex: i32,
    pub x: i32,
    pub y: i32,
    pub facing: i32,
    pub fid: u32,
    pub flags: i32,
    pub elevation: i32,
    pub pid: u32,
    pub sid: i32,
    pub script_index: i32,
    pub kind: &'static str,
    pub flat: bool,
    pub blocks: bool,
}

#[derive(Clone, Debug)]
pub struct ScriptRecord {
    pub sid: i32,
    pub script_index: i32,
    pub owner_id: i32,
    pub local_vars_count: i32,
}

#[derive(Clone, Debug)]
pub struct MapLevel {
    pub elevation: i32,
    pub floors: Vec<FloorCell>,
    pub objects: Vec<PlacedObject>,
}

#[derive(Clone, Debug)]
pub struct GameMap {
    pub version: i32,
    pub name: String,
    pub entering_tile: i32,
    pub entering_elevation: i32,
    pub entering_rotation: i32,
    pub map_script_index: i32,
    pub flags: i32,
    pub map_vars: Vec<i32>,
    pub saved_globals: Vec<i32>,
    pub scripts: Vec<ScriptRecord>,
    pub levels: Vec<MapLevel>,
    pub populated_elevation: usize,
    pub bytes_consumed: usize,
    pub file_size: usize,
}

impl GameMap {
    pub fn populated(&self) -> &MapLevel {
        &self.levels[self.populated_elevation]
    }
}

struct ProtoDb<'a> {
    files: &'a GameFiles,
    lists: Vec<Vec<String>>,
    subtypes: std::collections::HashMap<u32, i32>,
}

impl<'a> ProtoDb<'a> {
    fn load(files: &'a GameFiles) -> Result<Self, String> {
        let mut lists = Vec::with_capacity(PROTO_LST.len());
        for path in PROTO_LST {
            let text = String::from_utf8_lossy(&files.read(path)?).into_owned();
            let mut entries = Vec::new();
            for line in text.split('\n') {
                let line = line.trim_end_matches('\r');
                let token = line
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                entries.push(token.to_string());
            }
            if entries.last().is_some_and(|entry| entry.is_empty()) && text.ends_with('\n') {
                // Keep the same physical lines a splitlines() call would, including a missing trailing blank.
            }
            if text.ends_with('\n') || text.ends_with("\r\n") {
                if entries.last().is_some_and(|entry| entry.is_empty()) {
                    entries.pop();
                }
            }
            lists.push(entries);
        }
        Ok(Self {
            files,
            lists,
            subtypes: std::collections::HashMap::new(),
        })
    }

    fn subtype(&mut self, pid: u32) -> Result<Option<i32>, String> {
        if let Some(value) = self.subtypes.get(&pid) {
            return Ok(Some(*value));
        }
        let typ = (pid >> 24) & 0xFF;
        if typ != 0 && typ != 2 {
            return Ok(None);
        }
        let proto_id = (pid & 0x00FF_FFFF) as usize;
        let entries = &self.lists[typ as usize];
        if proto_id == 0 || proto_id > entries.len() || entries[proto_id - 1].is_empty() {
            return Err(format!("PID {pid:#x} is outside {}", PROTO_LST[typ as usize]));
        }
        let name = &entries[proto_id - 1];
        let path = format!("{}\\{name}", PROTO_DIR[typ as usize]);
        let data = self.files.read(&path)?;
        if data.len() < 0x24 {
            return Err(format!("{path} is too small to hold a subtype"));
        }
        let subtype = i32::from_be_bytes(data[0x20..0x24].try_into().unwrap());
        self.subtypes.insert(pid, subtype);
        Ok(Some(subtype))
    }
}

fn payload_ints(pid: u32, protos: &mut ProtoDb, version: i32) -> Result<usize, String> {
    let typ = (pid >> 24) & 0xFF;
    if typ == 1 {
        return Ok(11);
    }
    if typ == 0 {
        let subtype = protos.subtype(pid)?.ok_or_else(|| format!("item PID {pid:#x} has no subtype"))?;
        return match subtype {
            0 | 1 | 2 => Ok(1),
            3 => Ok(3),
            4 | 5 | 6 => Ok(2),
            other => Err(format!("unknown item subtype {other} on PID {pid:#x}")),
        };
    }
    if typ == 2 {
        let subtype = protos.subtype(pid)?.ok_or_else(|| format!("scenery PID {pid:#x} has no subtype"))?;
        return match subtype {
            0 => Ok(2),
            1 | 2 => Ok(3),
            3 | 4 => Ok(if version == 19 { 2 } else { 3 }),
            5 => Ok(1),
            other => Err(format!("unknown scenery subtype {other} on PID {pid:#x}")),
        };
    }
    if typ == 5 && (EXIT_GRID_LO..=EXIT_GRID_HI).contains(&pid) {
        return Ok(5);
    }
    if typ == 3 || typ == 4 || typ == 5 {
        return Ok(1);
    }
    Err(format!("unknown object type {typ} on PID {pid:#x}"))
}

fn kind_of(pid: u32) -> &'static str {
    match (pid >> 24) & 0xFF {
        0 => "item",
        1 => "critter",
        2 => "scenery",
        3 => "wall",
        4 => "tile",
        _ => "misc",
    }
}

fn read_object(
    reader: &mut Reader,
    protos: &mut ProtoDb,
    elevation: i32,
    version: i32,
    depth: i32,
) -> Result<Option<PlacedObject>, String> {
    if depth > 8 {
        return Err("inventory nesting is too deep".into());
    }
    let mut base = [0i32; 18];
    for slot in &mut base {
        *slot = reader.i32()?;
    }
    let inventory_length = reader.i32()?;
    let capacity = reader.i32()?;
    let _pointer = reader.i32()?;
    if inventory_length < 0 || capacity < inventory_length || inventory_length > 512 {
        return Err(format!(
            "inventory length {inventory_length}/{capacity} at offset {}",
            reader.pos
        ));
    }
    let pid = base[11] as u32;
    let extra = payload_ints(pid, protos, version)?;
    for _ in 0..extra {
        reader.i32()?;
    }
    for _ in 0..inventory_length {
        reader.i32()?;
        read_object(reader, protos, elevation, version, depth + 1)?;
    }
    let tile = base[1];
    if !(0..=39999).contains(&tile) {
        return Ok(None);
    }
    let flags = base[9];
    if flags & OBJECT_HIDDEN != 0 {
        return Ok(None);
    }
    let kind = kind_of(pid);
    let flat = flags & OBJECT_FLAT != 0;
    let blocks = matches!(kind, "wall" | "scenery") && !flat && flags & OBJECT_NO_BLOCK == 0;
    Ok(Some(PlacedObject {
        id: base[0],
        hex: tile,
        x: base[2],
        y: base[3],
        facing: base[7].rem_euclid(6),
        fid: base[8] as u32,
        flags,
        elevation,
        pid,
        sid: base[16],
        script_index: base[17],
        kind,
        flat,
        blocks,
    }))
}

fn read_script(reader: &mut Reader) -> Result<ScriptRecord, String> {
    let sid = reader.i32()?;
    let _next = reader.i32()?;
    let kind = sid >> 24;
    let extra = if kind == 1 { 2 } else if kind == 2 { 1 } else { 0 };
    let mut fields = Vec::with_capacity(extra + 14);
    for _ in 0..(extra + 14) {
        fields.push(reader.i32()?);
    }
    let body = &fields[extra..];
    Ok(ScriptRecord {
        sid,
        script_index: body[1],
        owner_id: body[3],
        local_vars_count: body[5],
    })
}

fn read_scripts(reader: &mut Reader) -> Result<Vec<ScriptRecord>, String> {
    let mut scripts = Vec::new();
    for _list in 0..5 {
        let count = reader.i32()?;
        if count < 0 {
            return Err(format!("script list count {count}"));
        }
        if count == 0 {
            continue;
        }
        // Extents hold 16 records. Only `count` of them are live; the tail is padding.
        let extents = (count + 15) / 16;
        let mut live = 0;
        for _ in 0..extents {
            for _ in 0..16 {
                let record = read_script(reader)?;
                if live < count {
                    scripts.push(record);
                    live += 1;
                }
            }
            reader.i32()?;
            reader.i32()?;
        }
    }
    Ok(scripts)
}

pub fn load_map(data: &[u8], files: &GameFiles) -> Result<GameMap, String> {
    if data.len() < HEADER_SIZE {
        return Err(format!("MAP is {} bytes, header is {HEADER_SIZE}", data.len()));
    }
    let mut protos = ProtoDb::load(files)?;
    let mut reader = Reader { data, pos: 0 };
    let version = reader.i32()?;
    if version != 19 && version != 20 {
        let little = i32::from_le_bytes(data[0..4].try_into().unwrap());
        return Err(format!(
            "MAP version {version} is not 19 or 20 (a little-endian misread is {little})"
        ));
    }
    let name_bytes = reader.raw(16)?;
    let name_end = name_bytes.iter().position(|byte| *byte == 0).unwrap_or(name_bytes.len());
    let name = String::from_utf8_lossy(&name_bytes[..name_end]).into_owned();
    let entering_tile = reader.i32()?;
    let entering_elevation = reader.i32()?;
    let entering_rotation = reader.i32()?;
    let mut local_vars = reader.i32()?;
    let map_script_index = reader.i32()?;
    let flags = reader.i32()?;
    let _darkness = reader.i32()?;
    let mut global_vars = reader.i32()?;
    let _map_index = reader.i32()?;
    let _last_visit = reader.i32()?;
    reader.raw(44 * 4)?;
    if reader.pos != HEADER_SIZE {
        return Err(format!("header consumed {} bytes, expected {HEADER_SIZE}", reader.pos));
    }
    if global_vars < 0 {
        global_vars = 0;
    }
    if local_vars < 0 {
        local_vars = 0;
    }
    let mut saved_globals = Vec::with_capacity(global_vars as usize);
    for _ in 0..global_vars {
        saved_globals.push(reader.i32()?);
    }
    let mut map_vars = Vec::with_capacity(local_vars as usize);
    for _ in 0..local_vars {
        map_vars.push(reader.i32()?);
    }

    let mut tile_words: Vec<Option<Vec<u32>>> = Vec::with_capacity(ELEVATIONS);
    for elevation in 0..ELEVATIONS {
        if flags & ELEVATION_ABSENT[elevation] != 0 {
            tile_words.push(None);
            continue;
        }
        let mut words = Vec::with_capacity(SQUARE_COUNT);
        for _ in 0..SQUARE_COUNT {
            words.push(reader.i32()? as u32);
        }
        tile_words.push(Some(words));
    }

    let scripts = read_scripts(&mut reader)?;
    let total_objects = reader.i32()?;
    if total_objects < 0 {
        return Err(format!("object count {total_objects}"));
    }
    let mut placed = Vec::with_capacity(ELEVATIONS);
    let mut seen = 0;
    for elevation in 0..ELEVATIONS {
        let count = reader.i32()?;
        if count < 0 || count > total_objects - seen {
            return Err(format!("elevation {elevation} object count {count} of {total_objects}"));
        }
        let mut level_objects = Vec::new();
        for _ in 0..count {
            if let Some(object) = read_object(&mut reader, &mut protos, elevation as i32, version, 0)? {
                level_objects.push(object);
            }
            seen += 1;
        }
        placed.push(level_objects);
    }
    if seen != total_objects {
        return Err(format!("read {seen} objects, header said {total_objects}"));
    }
    if reader.pos != data.len() {
        return Err(format!("MAP consumed {} of {} bytes", reader.pos, data.len()));
    }

    let mut levels = Vec::with_capacity(ELEVATIONS);
    for elevation in 0..ELEVATIONS {
        let mut floors = Vec::new();
        if let Some(words) = &tile_words[elevation] {
            for (square, word) in words.iter().enumerate() {
                floors.push(FloorCell {
                    square: square as i32,
                    art_index: (*word & 0x0FFF) as i32,
                });
            }
        }
        levels.push(MapLevel {
            elevation: elevation as i32,
            floors,
            objects: placed[elevation].clone(),
        });
    }
    let populated_elevation = levels
        .iter()
        .enumerate()
        .max_by_key(|(_, level)| (level.objects.len(), level.floors.len()))
        .map(|(index, _)| index)
        .unwrap_or(0);
    if levels[populated_elevation].floors.is_empty() && levels[populated_elevation].objects.is_empty() {
        return Err("map has no populated elevation".into());
    }
    Ok(GameMap {
        version,
        name,
        entering_tile,
        entering_elevation,
        entering_rotation,
        map_script_index,
        flags,
        map_vars,
        saved_globals,
        scripts,
        levels,
        populated_elevation,
        bytes_consumed: reader.pos,
        file_size: data.len(),
    })
}

pub fn load_game_map(files: &GameFiles, path: &str) -> Result<GameMap, String> {
    let data = files.read(&normalize_path(path))?;
    load_map(&data, files)
}
