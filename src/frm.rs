//! FRM sprites and the Fallout `color.pal` palette. The file is big-endian.

#[derive(Clone, Debug)]
pub struct Palette {
    pub rgb: [[u8; 3]; 256],
}

pub fn decode_palette(data: &[u8]) -> Result<Palette, String> {
    if data.len() < 768 {
        return Err(format!("palette is {} bytes, need at least 768", data.len()));
    }
    let mut rgb = [[0u8; 3]; 256];
    for index in 0..256 {
        for channel in 0..3 {
            let raw = data[index * 3 + channel];
            rgb[index][channel] = (raw << 2) | (raw >> 4);
        }
    }
    Ok(Palette { rgb })
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u16,
    pub height: u16,
    pub dx: i16,
    pub dy: i16,
    pub pixels: Vec<u8>,
}

impl Frame {
    pub const TRANSPARENT_INDEX: u8 = 0;

    pub fn transparent_index(&self) -> u8 {
        Self::TRANSPARENT_INDEX
    }

    pub fn rgba(&self, palette: &Palette) -> Vec<u8> {
        let mut image = Vec::with_capacity(self.pixels.len() * 4);
        for &index in &self.pixels {
            let color = palette.rgb[index as usize];
            image.push(color[0]);
            image.push(color[1]);
            image.push(color[2]);
            image.push(if index == Self::TRANSPARENT_INDEX { 0 } else { 255 });
        }
        image
    }
}

#[derive(Clone, Debug)]
pub struct Frm {
    pub version: u32,
    pub fps: u16,
    pub action_frame: u16,
    pub frames_per_direction: u16,
    pub direction_offsets: [u32; 6],
    pub frames: Vec<Vec<Frame>>,
    pub stored_directions: Vec<usize>,
}

impl Frm {
    pub fn direction_frame(&self, direction: usize) -> Result<&Frame, String> {
        let frames = self
            .frames
            .get(direction)
            .ok_or_else(|| format!("direction {direction} is missing"))?;
        frames
            .first()
            .ok_or_else(|| format!("direction {direction} has no frames"))
    }
}

fn be_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([data[offset], data[offset + 1]])
}

fn be_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn be_i16(data: &[u8], offset: usize) -> i16 {
    i16::from_be_bytes([data[offset], data[offset + 1]])
}

pub fn decode_frm(data: &[u8]) -> Result<Frm, String> {
    if data.len() < 0x3E {
        return Err(format!("FRM is {} bytes, header is 62", data.len()));
    }
    let version = be_u32(data, 0);
    let fps = be_u16(data, 0x04);
    let action_frame = be_u16(data, 0x06);
    let per_dir = be_u16(data, 0x08);
    if per_dir == 0 {
        return Err("FRM frames_per_direction is 0".into());
    }
    let mut offsets = [0u32; 6];
    for index in 0..6 {
        offsets[index] = be_u32(data, 0x22 + 4 * index);
    }
    let mut frames: Vec<Vec<Frame>> = Vec::with_capacity(6);
    let mut stored = Vec::new();
    for rotation in 0..6 {
        if rotation != 0 && offsets[rotation] == offsets[rotation - 1] {
            frames.push(frames[rotation - 1].clone());
            continue;
        }
        let mut pos = 0x3E + offsets[rotation] as usize;
        let mut direction_frames = Vec::with_capacity(per_dir as usize);
        for _ in 0..per_dir {
            if pos + 12 > data.len() {
                return Err(format!("FRM direction {rotation} frame header overruns the file"));
            }
            let width = be_u16(data, pos);
            let height = be_u16(data, pos + 2);
            let size = be_u32(data, pos + 4) as usize;
            let dx = be_i16(data, pos + 8);
            let dy = be_i16(data, pos + 10);
            let start = pos + 12;
            let end = start + size;
            if end > data.len() {
                return Err(format!("FRM direction {rotation} pixels overrun the file"));
            }
            let pixels = data[start..end].to_vec();
            if pixels.len() != width as usize * height as usize {
                return Err(format!(
                    "frame pixel count {} != {width} * {height}",
                    pixels.len()
                ));
            }
            direction_frames.push(Frame {
                width,
                height,
                dx,
                dy,
                pixels,
            });
            pos = end;
        }
        frames.push(direction_frames);
        stored.push(rotation);
    }
    Ok(Frm {
        version,
        fps,
        action_frame,
        frames_per_direction: per_dir,
        direction_offsets: offsets,
        frames,
        stored_directions: stored,
    })
}
