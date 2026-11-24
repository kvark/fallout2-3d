//! World-fixed vertical sprites and whether they hide a point from a corner.

use crate::camera::{self, PITCH_DEG};

const EPS: f32 = 1e-6;

pub fn fixed_vertical_quad(center_xz: (f32, f32), width: f32, height: f32, pitch_deg: f32) -> [[f32; 3]; 4] {
    let (_forward, mut right, _up) = camera::camera_axes(0, pitch_deg).expect("pitch");
    right[1] = 0.0;
    right = camera::normalize(right);
    let base = [center_xz.0, 0.0, center_xz.1];
    let half = camera::scale(right, width * 0.5);
    let up = [0.0, height, 0.0];
    let left = camera::sub(base, half);
    let right_pt = camera::add(base, half);
    [
        left,
        right_pt,
        camera::add(right_pt, up),
        camera::add(left, up),
    ]
}

fn ray_triangle(origin: [f32; 3], direction: [f32; 3], triangle: [[f32; 3]; 3]) -> Option<f32> {
    let (v0, v1, v2) = (triangle[0], triangle[1], triangle[2]);
    let edge1 = camera::sub(v1, v0);
    let edge2 = camera::sub(v2, v0);
    let pvec = camera::cross(direction, edge2);
    let det = camera::dot(edge1, pvec);
    if det.abs() < EPS {
        return None;
    }
    let inv = 1.0 / det;
    let tvec = camera::sub(origin, v0);
    let u = camera::dot(tvec, pvec) * inv;
    if !(-1e-5..=1.0 + 1e-5).contains(&u) {
        return None;
    }
    let qvec = camera::cross(tvec, edge1);
    let v = camera::dot(direction, qvec) * inv;
    if v < -1e-5 || u + v > 1.0 + 1e-5 {
        return None;
    }
    let t = camera::dot(edge2, qvec) * inv;
    if t <= EPS {
        return None;
    }
    Some(t)
}

pub fn ray_hits_quad(origin: [f32; 3], target: [f32; 3], quad: [[f32; 3]; 4]) -> bool {
    let direction = camera::sub(target, origin);
    let length = (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2]).sqrt();
    if length < EPS {
        return false;
    }
    let triangles = [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]];
    triangles.into_iter().any(|triangle| {
        ray_triangle(origin, direction, triangle).is_some_and(|t| t < 1.0 - 1e-4)
    })
}

pub fn point_hidden(corner: i32, point: [f32; 3]) -> bool {
    point_hidden_at(corner, point, (0.0, 0.0), 2.0, 2.0, PITCH_DEG, 8.0)
}

pub fn point_hidden_at(
    corner: i32,
    point: [f32; 3],
    center_xz: (f32, f32),
    width: f32,
    height: f32,
    pitch_deg: f32,
    distance: f32,
) -> bool {
    let target = [center_xz.0, 0.0, center_xz.1];
    let origin = camera::camera_position(corner, target, distance, pitch_deg).expect("pitch");
    let quad = fixed_vertical_quad(center_xz, width, height, pitch_deg);
    ray_hits_quad(origin, point, quad)
}
