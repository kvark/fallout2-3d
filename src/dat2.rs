//! Fallout 2 DAT2 archives. The directory is little-endian at the end of the file.
//! Compressed payloads are zlib.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::ZlibDecoder;

pub const DEFAULT_GOG: &str = "/x/GOG/fallout_2_game";
pub const DEFAULT_MAP: &str = "maps\\arvillag.map";

pub fn normalize_path(path: &str) -> String {
    let mut text = path.replace('/', "\\").trim().to_ascii_lowercase();
    while let Some(rest) = text.strip_prefix(".\\") {
        text = rest.to_string();
    }
    text
}

#[derive(Clone, Debug)]
pub struct DatEntry {
    pub path: String,
    pub compressed: bool,
    pub uncompressed_size: u32,
    pub data_size: u32,
    pub data_offset: u32,
}

pub struct Dat2 {
    pub path: PathBuf,
    data: Vec<u8>,
    pub data_base: usize,
    entries: HashMap<String, DatEntry>,
    pub order: Vec<String>,
}

impl Dat2 {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let mut file = File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|err| format!("read {}: {err}", path.display()))?;
        let size = data.len();
        if size < 8 {
            return Err(format!("{} is too small to be a DAT2 archive", path.display()));
        }
        let tree_size = u32::from_le_bytes(data[size - 8..size - 4].try_into().unwrap()) as usize;
        let data_size = u32::from_le_bytes(data[size - 4..].try_into().unwrap()) as usize;
        if data_size > size || tree_size < 4 || tree_size + 8 > data_size {
            return Err(format!("{} has an invalid DAT2 footer", path.display()));
        }
        let data_base = size - data_size;
        let tree_offset = size - tree_size - 8;
        if tree_offset < data_base || tree_offset + tree_size > size - 8 {
            return Err(format!("{} directory tree is out of range", path.display()));
        }
        let count = u32::from_le_bytes(data[tree_offset..tree_offset + 4].try_into().unwrap()) as usize;
        let mut pos = tree_offset + 4;
        let end = tree_offset + tree_size;
        let mut entries = HashMap::new();
        let mut order = Vec::with_capacity(count);
        for _ in 0..count {
            if pos + 4 > end {
                return Err(format!("{} directory ended early", path.display()));
            }
            let path_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            if pos + path_len + 1 + 12 > end {
                return Err(format!("{} directory entry overruns the tree", path.display()));
            }
            let raw = &data[pos..pos + path_len];
            pos += path_len;
            let rel = String::from_utf8_lossy(raw).into_owned();
            let compressed = data[pos];
            pos += 1;
            if compressed > 1 {
                return Err(format!(
                    "{} entry {rel:?} has compression flag {compressed}",
                    path.display()
                ));
            }
            let uncompressed_size = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
            let data_len = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap());
            let data_offset = u32::from_le_bytes(data[pos + 8..pos + 12].try_into().unwrap());
            pos += 12;
            let key = normalize_path(&rel);
            entries.insert(
                key.clone(),
                DatEntry {
                    path: rel,
                    compressed: compressed == 1,
                    uncompressed_size,
                    data_size: data_len,
                    data_offset,
                },
            );
            order.push(key);
        }
        if pos != end {
            return Err(format!(
                "{} directory parsed {} of {tree_size} bytes",
                path.display(),
                pos - tree_offset
            ));
        }
        Ok(Self {
            path: path.to_path_buf(),
            data,
            data_base,
            entries,
            order,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(&normalize_path(path))
    }

    pub fn entry(&self, path: &str) -> Result<&DatEntry, String> {
        self.entries
            .get(&normalize_path(path))
            .ok_or_else(|| format!("missing archive path {path}"))
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        let entry = self.entry(path)?;
        let start = self.data_base + entry.data_offset as usize;
        let end = start + entry.data_size as usize;
        if end > self.data.len() {
            return Err(format!("{} payload is outside the archive", entry.path));
        }
        let blob = &self.data[start..end];
        if entry.compressed {
            let mut decoder = ZlibDecoder::new(blob);
            let mut out = Vec::with_capacity(entry.uncompressed_size as usize);
            decoder
                .read_to_end(&mut out)
                .map_err(|err| format!("{} inflate failed: {err}", entry.path))?;
            if out.len() != entry.uncompressed_size as usize {
                return Err(format!(
                    "{} inflated to {} bytes, directory says {}",
                    entry.path,
                    out.len(),
                    entry.uncompressed_size
                ));
            }
            Ok(out)
        } else if blob.len() != entry.uncompressed_size as usize {
            Err(format!(
                "{} stored {} bytes, directory says {}",
                entry.path,
                blob.len(),
                entry.uncompressed_size
            ))
        } else {
            Ok(blob.to_vec())
        }
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(String::as_str)
    }
}

pub struct GameFiles {
    pub root: PathBuf,
    pub master: Dat2,
    pub critter: Dat2,
    pub patch: Dat2,
}

impl GameFiles {
    pub fn open(gog: impl AsRef<Path>) -> Result<Self, String> {
        let root = gog.as_ref();
        Ok(Self {
            master: Dat2::open(root.join("master.dat"))?,
            critter: Dat2::open(root.join("critter.dat"))?,
            patch: Dat2::open(root.join("patch000.dat"))?,
            root: root.to_path_buf(),
        })
    }

    pub fn open_default() -> Result<Self, String> {
        Self::open(DEFAULT_GOG)
    }

    fn archives(&self) -> [&Dat2; 3] {
        [&self.patch, &self.critter, &self.master]
    }

    pub fn locate<'a>(&'a self, path: &str) -> Result<&'a Dat2, String> {
        let key = normalize_path(path);
        for archive in self.archives() {
            if archive.entries.contains_key(&key) {
                return Ok(archive);
            }
        }
        Err(format!("missing game file {path}"))
    }

    pub fn contains(&self, path: &str) -> bool {
        let key = normalize_path(path);
        self.archives().iter().any(|archive| archive.entries.contains_key(&key))
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        self.locate(path)?.read(path)
    }
}
