//! Fallout's 200-wide hex grid. Directions are NE, E, SE, SW, W, NW.

pub const HEX_WIDTH: i32 = 200;
pub const HEX_HEIGHT: i32 = 200;
pub const HEX_COUNT: i32 = HEX_WIDTH * HEX_HEIGHT;
pub const DIRECTION_NAMES: [&str; 6] = ["NE", "E", "SE", "SW", "W", "NW"];

const DIR_DELTA: [[i32; 6]; 2] = [
    [-1, HEX_WIDTH - 1, HEX_WIDTH, HEX_WIDTH + 1, 1, -HEX_WIDTH],
    [
        -HEX_WIDTH - 1,
        -1,
        HEX_WIDTH,
        1,
        1 - HEX_WIDTH,
        -HEX_WIDTH,
    ],
];

pub fn in_bounds(tile: i32) -> bool {
    (0..HEX_COUNT).contains(&tile)
}

/// One hex step. This is the shipped step used by play commands.
pub fn step(tile: i32, facing: i32) -> Result<i32, String> {
    if !(0..6).contains(&facing) {
        return Err(format!("facing {facing} is outside 0..5"));
    }
    let parity = (tile.rem_euclid(HEX_WIDTH) & 1) as usize;
    Ok(tile + DIR_DELTA[parity][facing as usize])
}

pub fn hex_screen(tile: i32) -> (i32, i32) {
    let col = tile.rem_euclid(HEX_WIDTH);
    let row = tile.div_euclid(HEX_WIDTH);
    let mirrored = (HEX_WIDTH - 1) - col;
    let mut sx = 48 * (mirrored / 2);
    let mut sy = 12 * (-(mirrored / 2));
    if mirrored & 1 != 0 {
        sx += 32;
    }
    sx += 16 * row;
    sy += 12 * row;
    (sx, sy)
}

pub fn hex_to_world(tile: i32) -> (f32, f32) {
    let (sx, sy) = hex_screen(tile);
    ((sx + 16) as f32 / 32.0, -((sy + 8) as f32) / 24.0)
}

pub fn square_of_hex(tile: i32) -> Result<i32, String> {
    if !in_bounds(tile) {
        return Err(format!("hex {tile} is outside 0..{}", HEX_COUNT - 1));
    }
    let tile_col = (HEX_WIDTH - 1) - tile.rem_euclid(HEX_WIDTH);
    let tile_row = tile.div_euclid(HEX_WIDTH);
    let square_col = tile_col / 2;
    let square_row = tile_row / 2;
    Ok((99 - square_col) + 100 * square_row)
}

/// Engine `squareTileToScreenXY` with the map origin at square 0.
pub fn square_blit(square: i32) -> (i32, i32) {
    let tile_col = 99 - (square.rem_euclid(100));
    let tile_row = square.div_euclid(100);
    let sx = -16 + 48 * tile_col + 32 * tile_row;
    let sy = -2 - 12 * tile_col + 24 * tile_row;
    (sx, sy)
}

pub fn screen_to_world(px: f32, py: f32) -> (f32, f32) {
    (px / 32.0, -(py / 24.0))
}

/// Screen-space direction from `from` toward `to`, matching the engine's
/// `tileGetRotationTo` (atan2 of the hex blit positions, 60° buckets).
pub fn rotation_to(from: i32, to: i32) -> i32 {
    let (x1, y1) = hex_screen(from);
    let (x2, y2) = hex_screen(to);
    let dx = x2 - x1;
    let dy = y2 - y1;
    if dx != 0 {
        let degrees = (f64::from(-dy)).atan2(f64::from(dx)).to_degrees();
        let mut bucket = 360 - (degrees.trunc() as i32 + 180) - 90;
        if bucket < 0 {
            bucket += 360;
        }
        bucket /= 60;
        if bucket >= 6 {
            5
        } else {
            bucket
        }
    } else if dy < 0 {
        0
    } else {
        2
    }
}

/// Hex steps between two tiles. Invalid tiles use the engine's no-path value.
pub fn distance(from: i32, to: i32) -> i32 {
    if !in_bounds(from) || !in_bounds(to) {
        return 9999;
    }
    let mut tile = from;
    for steps in 0..400 {
        if tile == to {
            return steps;
        }
        let facing = rotation_to(tile, to);
        match step(tile, facing) {
            Ok(next) if in_bounds(next) => tile = next,
            _ => return 9999,
        }
    }
    9999
}
