//! Four corners. Corner 0 sits on the southwest side, looking northeast.

use std::f32::consts::PI;

pub const PITCH_DEG: f32 = 58.0;

const HORIZON: [(f32, f32); 4] = [(1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)];

pub fn horizontal_look(corner: i32) -> [f32; 3] {
    let (east, north) = HORIZON[corner.rem_euclid(4) as usize];
    let length = east.hypot(north);
    [east / length, 0.0, north / length]
}

pub fn view_forward(corner: i32, pitch_deg: f32) -> Result<[f32; 3], String> {
    if !(0.0 < pitch_deg && pitch_deg < 90.0) {
        return Err(format!(
            "pitch {pitch_deg} is not strictly between the horizon and straight down"
        ));
    }
    let pitch = pitch_deg * PI / 180.0;
    let look = horizontal_look(corner);
    let forward = [
        pitch.cos() * look[0],
        -pitch.sin(),
        pitch.cos() * look[2],
    ];
    Ok(normalize(forward))
}

pub fn camera_axes(corner: i32, pitch_deg: f32) -> Result<([f32; 3], [f32; 3], [f32; 3]), String> {
    let forward = view_forward(corner, pitch_deg)?;
    let world_up = [0.0, 1.0, 0.0];
    let right = normalize(cross(world_up, forward));
    let up = normalize(cross(forward, right));
    Ok((forward, right, up))
}

pub fn yaw_degrees(corner: i32) -> f32 {
    let look = horizontal_look(corner);
    look[0].atan2(look[2]).to_degrees().rem_euclid(360.0)
}

pub fn pitch_degrees(corner: i32, pitch_deg: f32) -> Result<f32, String> {
    let forward = view_forward(corner, pitch_deg)?;
    let horizontal = forward[0].hypot(forward[2]);
    Ok((-forward[1]).atan2(horizontal).to_degrees())
}

pub fn camera_position(corner: i32, target: [f32; 3], distance: f32, pitch_deg: f32) -> Result<[f32; 3], String> {
    let forward = view_forward(corner, pitch_deg)?;
    Ok([
        target[0] - forward[0] * distance,
        target[1] - forward[1] * distance,
        target[2] - forward[2] * distance,
    ])
}

#[derive(Clone, Debug)]
pub struct CornerCamera {
    pub corner: i32,
    pub pitch_deg: f32,
}

impl Default for CornerCamera {
    fn default() -> Self {
        Self {
            corner: 0,
            pitch_deg: PITCH_DEG,
        }
    }
}

impl CornerCamera {
    pub fn new(corner: i32) -> Self {
        Self {
            corner: corner.rem_euclid(4),
            pitch_deg: PITCH_DEG,
        }
    }

    pub fn advance(&mut self) -> i32 {
        self.corner = (self.corner + 1).rem_euclid(4);
        self.corner
    }

    pub fn back(&mut self) -> i32 {
        self.corner = (self.corner - 1).rem_euclid(4);
        self.corner
    }

    pub fn yaw(&self) -> f32 {
        yaw_degrees(self.corner)
    }

    pub fn pitch(&self) -> Result<f32, String> {
        pitch_degrees(self.corner, self.pitch_deg)
    }
}

pub fn normalize(value: [f32; 3]) -> [f32; 3] {
    let length = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt();
    if length == 0.0 {
        return value;
    }
    [value[0] / length, value[1] / length, value[2] / length]
}

pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn scale(a: [f32; 3], factor: f32) -> [f32; 3] {
    [a[0] * factor, a[1] * factor, a[2] * factor]
}
