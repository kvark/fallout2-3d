//! Placed floors and objects for one elevation.

use crate::art::ArtIndex;
use crate::dat2::GameFiles;
use crate::hexgrid::hex_to_world;
use crate::mapfile::{GameMap, PlacedObject};

#[derive(Clone, Debug)]
pub struct SceneObject {
    pub hex: i32,
    pub facing: i32,
    pub fid: u32,
    pub kind: &'static str,
    pub flat: bool,
    pub blocks: bool,
    pub x: f32,
    pub z: f32,
    pub path: String,
    pub id: i32,
    pub pid: u32,
    pub sid: i32,
    pub script_index: i32,
    pub handle: i32,
}

#[derive(Clone, Debug)]
pub struct SceneFloor {
    pub square: i32,
    pub art_index: i32,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub version: i32,
    pub elevation: i32,
    pub floors: Vec<SceneFloor>,
    pub objects: Vec<SceneObject>,
}

impl Scene {
    pub fn critters(&self) -> impl Iterator<Item = &SceneObject> {
        self.objects.iter().filter(|object| object.kind == "critter")
    }
}

fn blank_tile(name: &str, art_index: i32) -> bool {
    if art_index <= 0 {
        return true;
    }
    let stem = name.rsplit('\\').next().unwrap_or(name).to_ascii_lowercase();
    stem.starts_with("grid") || stem.starts_with("blank")
}

fn object_path(art: &ArtIndex, files: &GameFiles, object: &PlacedObject) -> Option<String> {
    let path = art.resolve(object.fid).ok()?;
    if files.contains(&path) {
        return Some(path);
    }
    if object.kind == "critter" {
        let base = path.rsplit('\\').next().unwrap_or(&path);
        let stem = base.split('.').next().unwrap_or(base);
        if stem.len() >= 2 {
            let fallback = format!("art\\critters\\{}aa.frm", &stem[..stem.len() - 2]);
            if files.contains(&fallback) {
                return Some(fallback);
            }
        }
    }
    None
}

pub fn build_scene(files: &GameFiles, art: &ArtIndex, game_map: &GameMap) -> Result<Scene, String> {
    let level = game_map.populated();
    let mut floors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for cell in &level.floors {
        if !seen.insert(cell.square) {
            continue;
        }
        let Ok(path) = art.resolve_tile(cell.art_index) else {
            continue;
        };
        if blank_tile(&path, cell.art_index) || !files.contains(&path) {
            continue;
        }
        floors.push(SceneFloor {
            square: cell.square,
            art_index: cell.art_index,
            path,
        });
    }
    let mut objects = Vec::new();
    let mut handle = 2;
    for object in &level.objects {
        let Some(path) = object_path(art, files, object) else {
            continue;
        };
        let (x, z) = hex_to_world(object.hex);
        objects.push(SceneObject {
            hex: object.hex,
            facing: object.facing,
            fid: object.fid,
            kind: object.kind,
            flat: object.flat,
            blocks: object.blocks,
            x,
            z,
            path,
            id: object.id,
            pid: object.pid,
            sid: object.sid,
            script_index: object.script_index,
            handle,
        });
        handle += 1;
    }
    Ok(Scene {
        version: game_map.version,
        elevation: level.elevation,
        floors,
        objects,
    })
}
