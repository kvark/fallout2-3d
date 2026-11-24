//! Stored sprite direction for an object facing and a camera corner.

pub fn desired_angle(facing: i32, corner: i32) -> i32 {
    (facing * 60 - corner * 90).rem_euclid(360)
}

pub fn pick_direction(available: &[i32], facing: i32, corner: i32) -> Result<i32, String> {
    if available.is_empty() {
        return Err("no stored directions".into());
    }
    if available.len() == 1 {
        return Ok(available[0]);
    }
    let desired = desired_angle(facing, corner);
    let mut best = available[0];
    let mut best_key: Option<(i32, i32)> = None;
    for &direction in available {
        let angle = (direction * 60).rem_euclid(360);
        let clockwise = (angle - desired).rem_euclid(360);
        let distance = if clockwise <= 180 { clockwise } else { 360 - clockwise };
        let key = (distance, clockwise);
        if best_key.is_none() || key < best_key.unwrap() {
            best_key = Some(key);
            best = direction;
        }
    }
    Ok(best)
}
