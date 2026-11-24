//! Four corners drawn by blade-graphics into an offscreen target.

use std::collections::HashMap;
use std::f32::consts::PI;

use blade_graphics as gpu;
use blade_graphics::ShaderData;

use crate::art::{load_frm, select_direction_art};
use crate::camera::{self, PITCH_DEG};
use crate::frm::{decode_palette, Frame, Palette};
use crate::hexgrid::{self, hex_to_world};
use crate::play::Game;
use crate::scene::Scene;

pub const FRAME_WIDTH: u32 = 640;
pub const FRAME_HEIGHT: u32 = 360;
const FOV_DEG: f32 = 42.0;
const ATLAS_SCALE: f32 = 32.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SpriteVertex {
    pos: [f32; 3],
    uv: [f32; 2],
}

impl gpu::Vertex for SpriteVertex {
    fn layout() -> gpu::VertexLayout {
        gpu::VertexLayout {
            attributes: vec![
                (
                    "pos",
                    gpu::VertexAttribute {
                        offset: 0,
                        format: gpu::VertexFormat::F32Vec3,
                    },
                ),
                (
                    "uv",
                    gpu::VertexAttribute {
                        offset: 12,
                        format: gpu::VertexFormat::F32Vec2,
                    },
                ),
            ],
            stride: 20,
        }
    }
}

struct DrawData {
    camera: [[f32; 4]; 4],
    sprite_texture: gpu::TextureView,
    sprite_sampler: gpu::Sampler,
}

impl gpu::ShaderData for DrawData {
    fn layout() -> gpu::ShaderDataLayout {
        gpu::ShaderDataLayout {
            bindings: vec![
                ("camera", gpu::ShaderBinding::Plain { size: 64 }),
                ("sprite_texture", gpu::ShaderBinding::Texture),
                ("sprite_sampler", gpu::ShaderBinding::Sampler),
            ],
        }
    }

    fn fill(&self, mut ctx: gpu::PipelineContext) {
        use gpu::ShaderBindable as _;
        self.camera.bind_to(&mut ctx, 0);
        self.sprite_texture.bind_to(&mut ctx, 1);
        self.sprite_sampler.bind_to(&mut ctx, 2);
    }
}

const SHADER: &str = r#"
struct Camera {
    clip: mat4x4<f32>,
}

struct VertexIn {
    pos: vec3<f32>,
    uv: vec2<f32>,
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(vertex: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.position = camera.clip * vec4<f32>(vertex.pos, 1.0);
    out.uv = vertex.uv;
    return out;
}

@fragment
fn fs_main(fragment: VertexOut) -> @location(0) vec4<f32> {
    let color = textureSample(sprite_texture, sprite_sampler, fragment.uv);
    if (color.a < 0.5) {
        discard;
    }
    return color;
}

var<uniform> camera: Camera;
var sprite_texture: texture_2d<f32>;
var sprite_sampler: sampler;
"#;

#[derive(Clone)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub struct Rendered {
    pub device_name: String,
    pub frames: Vec<RgbaImage>,
}

struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

struct Atlas {
    width: u32,
    height: u32,
    min_x: f32,
    max_z: f32,
    pixels: Vec<u8>,
}

struct Quad {
    corners: [[f32; 3]; 4],
    anchor: [f32; 3],
    key: (String, usize),
}

pub fn render_corners(game: &mut Game) -> Result<Rendered, String> {
    game.sync_scene_positions();
    let palette = decode_palette(&game.files.read("color.pal")?)?;
    let (player_x, player_z) = hex_to_world(game.player_tile());
    let bounds = content_bounds(&game.scene, player_x, player_z);
    let atlas = build_atlas(game, &palette, bounds)?;
    let mut images: HashMap<(String, usize), Image> = HashMap::new();
    remember_image(
        &mut images,
        game,
        &palette,
        "__ground__",
        &atlas_image(&atlas),
    );
    let context = unsafe {
        gpu::Context::init(gpu::ContextDesc {
            presentation: false,
            validation: false,
            timing: false,
            capture: false,
            overlay: false,
            ray_tracing: false,
            xr: None,
            device_id: None,
        })
    }
    .map_err(|err| format!("blade context failed: {err}"))?;
    let device_name = context.device_information().device_name.clone();

    let shader = context.create_shader(gpu::ShaderDesc {
        source: SHADER,
        naga_module: None,
    });
    let data_layout = DrawData::layout();
    let vertex_layout = <SpriteVertex as gpu::Vertex>::layout();
    let mut pipeline = context.create_render_pipeline(gpu::RenderPipelineDesc {
        name: "fallout-sprite",
        data_layouts: &[&data_layout],
        vertex: shader.at("vs_main"),
        vertex_fetches: &[gpu::VertexFetchState {
            layout: &vertex_layout,
            instanced: false,
        }],
        primitive: gpu::PrimitiveState {
            topology: gpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(gpu::DepthStencilState {
            format: gpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: gpu::CompareFunction::Less,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        fragment: Some(shader.at("fs_main")),
        color_targets: &[gpu::ColorTargetState {
            format: gpu::TextureFormat::Rgba8Unorm,
            blend: Some(gpu::BlendState::ALPHA_BLENDING),
            write_mask: gpu::ColorWrites::ALL,
        }],
        multisample_state: gpu::MultisampleState::default(),
    });
    let sampler = context.create_sampler(gpu::SamplerDesc {
        name: "nearest",
        address_modes: [gpu::AddressMode::ClampToEdge; 3],
        mag_filter: gpu::FilterMode::Nearest,
        min_filter: gpu::FilterMode::Nearest,
        mipmap_filter: gpu::FilterMode::Nearest,
        ..Default::default()
    });

    let target_size = gpu::Extent {
        width: FRAME_WIDTH,
        height: FRAME_HEIGHT,
        depth: 1,
    };
    let color = context.create_texture(gpu::TextureDesc {
        name: "color",
        format: gpu::TextureFormat::Rgba8Unorm,
        size: target_size,
        array_layer_count: 1,
        mip_level_count: 1,
        sample_count: 1,
        dimension: gpu::TextureDimension::D2,
        usage: gpu::TextureUsage::TARGET | gpu::TextureUsage::COPY,
        external: None,
    });
    let color_view = context.create_texture_view(
        color,
        gpu::TextureViewDesc {
            name: "color",
            format: gpu::TextureFormat::Rgba8Unorm,
            dimension: gpu::ViewDimension::D2,
            subresources: &Default::default(),
        },
    );
    let depth = context.create_texture(gpu::TextureDesc {
        name: "depth",
        format: gpu::TextureFormat::Depth32Float,
        size: target_size,
        array_layer_count: 1,
        mip_level_count: 1,
        sample_count: 1,
        dimension: gpu::TextureDimension::D2,
        usage: gpu::TextureUsage::TARGET,
        external: None,
    });
    let depth_view = context.create_texture_view(
        depth,
        gpu::TextureViewDesc {
            name: "depth",
            format: gpu::TextureFormat::Depth32Float,
            dimension: gpu::ViewDimension::D2,
            subresources: &Default::default(),
        },
    );
    let row_bytes = FRAME_WIDTH * 4;
    let readback = context.create_buffer(gpu::BufferDesc {
        name: "readback",
        size: (row_bytes * FRAME_HEIGHT) as u64,
        memory: gpu::Memory::Shared,
    });

    let mut encoder = context.create_command_encoder(gpu::CommandEncoderDesc {
        name: "fallout",
        buffer_count: 1,
        manual_barriers: false,
    });
    encoder.start();
    encoder.init_texture(color);
    encoder.init_texture(depth);

    let mut gpu_textures: HashMap<(String, usize), (gpu::Texture, gpu::TextureView)> = HashMap::new();
    let mut staging = Vec::new();
    let ground_key = ("__ground__".to_string(), 0usize);
    stage_image(
        &context,
        &mut encoder,
        &mut gpu_textures,
        &mut staging,
        ground_key.clone(),
        images.get(&ground_key).expect("ground"),
    );

    let mut frames = Vec::with_capacity(4);
    for corner in 0..4 {
        let quads = sprites_for_corner(game, &palette, corner, &mut images)?;
        for quad in &quads {
            if gpu_textures.contains_key(&quad.key) {
                continue;
            }
            let image = images.get(&quad.key).expect("sprite image");
            stage_image(
                &context,
                &mut encoder,
                &mut gpu_textures,
                &mut staging,
                quad.key.clone(),
                image,
            );
        }
        let (forward, _right, _up) = camera::camera_axes(corner, PITCH_DEG)?;
        let distance = view_distance(corner, bounds);
        let target = [
            (bounds.0 + bounds.1) * 0.5,
            0.0,
            (bounds.2 + bounds.3) * 0.5,
        ];
        let eye = camera::camera_position(corner, target, distance, PITCH_DEG)?;
        let clip = view_clip(corner, bounds);
        let mut vertices = Vec::new();
        push_quad(
            &mut vertices,
            &[
                [atlas.min_x, 0.0, bounds.2 - 24.0],
                [atlas.min_x + atlas.width as f32 / ATLAS_SCALE, 0.0, bounds.2 - 24.0],
                [atlas.min_x + atlas.width as f32 / ATLAS_SCALE, 0.0, atlas.max_z],
                [atlas.min_x, 0.0, atlas.max_z],
            ],
        );
        let mut draw_ranges = vec![(ground_key.clone(), 0u32, 6u32)];
        let mut ordered = quads;
        ordered.sort_by(|a, b| {
            let da = camera::dot(camera::sub(a.anchor, eye), forward);
            let db = camera::dot(camera::sub(b.anchor, eye), forward);
            db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
        });
        for quad in &ordered {
            let start = vertices.len() as u32;
            if draw_ranges.last().is_some_and(|(key, _, _)| key == &quad.key) {
                draw_ranges.last_mut().unwrap().2 += 6;
            } else {
                draw_ranges.push((quad.key.clone(), start, 6));
            }
            push_quad(&mut vertices, &quad.corners);
        }
        let vertex_bytes = (vertices.len() * std::mem::size_of::<SpriteVertex>()) as u64;
        let vertex_buf = context.create_buffer(gpu::BufferDesc {
            name: "vertices",
            size: vertex_bytes.max(4),
            memory: gpu::Memory::Shared,
        });
        unsafe {
            std::ptr::copy_nonoverlapping(
                vertices.as_ptr() as *const u8,
                vertex_buf.data(),
                vertex_bytes as usize,
            );
        }
        context.sync_buffer(vertex_buf.into(), vertex_bytes, gpu::BufferTarget::Data);

        {
            let mut pass = encoder.render(
                "village",
                gpu::RenderTargetSet {
                    colors: &[gpu::RenderTarget {
                        view: color_view,
                        init_op: gpu::InitOp::Clear(gpu::TextureColor::OpaqueBlack),
                        finish_op: gpu::FinishOp::Store,
                    }],
                    depth_stencil: Some(gpu::RenderTarget {
                        view: depth_view,
                        init_op: gpu::InitOp::Clear(gpu::TextureColor::White),
                        finish_op: gpu::FinishOp::Store,
                    }),
                },
            );
            let mut pipe = pass.with(&pipeline);
            pipe.bind_vertex(0, vertex_buf.into());
            for (key, start, count) in &draw_ranges {
                let view = gpu_textures.get(key).expect("texture").1;
                pipe.bind(
                    0,
                    &DrawData {
                        camera: clip,
                        sprite_texture: view,
                        sprite_sampler: sampler,
                    },
                );
                pipe.draw(*start, *count, 0, 1);
            }
        }
        {
            let mut transfer = encoder.transfer("readback");
            transfer.copy_texture_to_buffer(color.into(), readback.into(), row_bytes, target_size);
        }
        let sync = context.submit(&mut encoder);
        context
            .wait_for(&sync, u32::MAX)
            .map_err(|err| format!("blade readback wait failed: {err}"))?;
        let byte_count = (row_bytes * FRAME_HEIGHT) as usize;
        let mut pixels = vec![0u8; byte_count];
        unsafe {
            std::ptr::copy_nonoverlapping(readback.data(), pixels.as_mut_ptr(), byte_count);
        }
        frames.push(RgbaImage {
            width: FRAME_WIDTH,
            height: FRAME_HEIGHT,
            pixels,
        });
        context.destroy_buffer(vertex_buf);
        if corner + 1 < 4 {
            encoder.start();
        }
    }

    for buffer in staging {
        context.destroy_buffer(buffer);
    }
    for (_, (texture, view)) in gpu_textures {
        context.destroy_texture_view(view);
        context.destroy_texture(texture);
    }
    context.destroy_buffer(readback);
    context.destroy_texture_view(depth_view);
    context.destroy_texture(depth);
    context.destroy_texture_view(color_view);
    context.destroy_texture(color);
    context.destroy_sampler(sampler);
    context.destroy_render_pipeline(&mut pipeline);
    context.destroy_command_encoder(&mut encoder);
    Ok(Rendered { device_name, frames })
}

/// Project a world point with the same camera `render_corners` uses for `corner`.
/// Image y grows downward. NDC +Y is up.
pub fn project_point(game: &Game, corner: i32, world: [f32; 3]) -> (f32, f32) {
    let (player_x, player_z) = hex_to_world(game.player_tile());
    let bounds = content_bounds(&game.scene, player_x, player_z);
    let clip = view_clip(corner, bounds);
    let clip_pos = transform_point(clip, world);
    let ndc_x = clip_pos[0] / clip_pos[3];
    let ndc_y = clip_pos[1] / clip_pos[3];
    let pixel_x = (ndc_x * 0.5 + 0.5) * FRAME_WIDTH as f32;
    let pixel_y = (0.5 - ndc_y * 0.5) * FRAME_HEIGHT as f32;
    (pixel_x, pixel_y)
}

fn view_clip(corner: i32, bounds: (f32, f32, f32, f32)) -> [[f32; 4]; 4] {
    let (forward, right, up) = camera::camera_axes(corner, PITCH_DEG).expect("pitch");
    let distance = view_distance(corner, bounds);
    let target = [
        (bounds.0 + bounds.1) * 0.5,
        0.0,
        (bounds.2 + bounds.3) * 0.5,
    ];
    let eye = camera::camera_position(corner, target, distance, PITCH_DEG).expect("camera");
    clip_matrix(
        forward,
        right,
        up,
        eye,
        FOV_DEG,
        FRAME_WIDTH as f32 / FRAME_HEIGHT as f32,
    )
}

fn transform_point(clip: [[f32; 4]; 4], point: [f32; 3]) -> [f32; 4] {
    let sample = [point[0], point[1], point[2], 1.0];
    let mut out = [0.0f32; 4];
    for row in 0..4 {
        for col in 0..4 {
            out[row] += clip[col][row] * sample[col];
        }
    }
    out
}

pub fn write_png(path: &std::path::Path, image: &RgbaImage) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let file = std::fs::File::create(path).map_err(|err| format!("create {}: {err}", path.display()))?;
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|err| format!("png header: {err}"))?;
    writer
        .write_image_data(&image.pixels)
        .map_err(|err| format!("png data: {err}"))?;
    Ok(())
}

fn remember_image(
    images: &mut HashMap<(String, usize), Image>,
    _game: &Game,
    _palette: &Palette,
    name: &str,
    image: &Image,
) {
    images.insert((name.to_string(), 0), clone_image(image));
}

fn clone_image(image: &Image) -> Image {
    Image {
        width: image.width,
        height: image.height,
        rgba: image.rgba.clone(),
    }
}

fn atlas_image(atlas: &Atlas) -> Image {
    Image {
        width: atlas.width,
        height: atlas.height,
        rgba: atlas.pixels.clone(),
    }
}

fn stage_image(
    context: &gpu::Context,
    encoder: &mut gpu::CommandEncoder,
    gpu_textures: &mut HashMap<(String, usize), (gpu::Texture, gpu::TextureView)>,
    staging: &mut Vec<gpu::Buffer>,
    key: (String, usize),
    image: &Image,
) {
    let texture = context.create_texture(gpu::TextureDesc {
        name: "sprite",
        format: gpu::TextureFormat::Rgba8Unorm,
        size: gpu::Extent {
            width: image.width.max(1),
            height: image.height.max(1),
            depth: 1,
        },
        array_layer_count: 1,
        mip_level_count: 1,
        sample_count: 1,
        dimension: gpu::TextureDimension::D2,
        usage: gpu::TextureUsage::RESOURCE | gpu::TextureUsage::COPY,
        external: None,
    });
    encoder.init_texture(texture);
    let view = context.create_texture_view(
        texture,
        gpu::TextureViewDesc {
            name: "sprite",
            format: gpu::TextureFormat::Rgba8Unorm,
            dimension: gpu::ViewDimension::D2,
            subresources: &Default::default(),
        },
    );
    let bytes = image.rgba.len().max(4) as u64;
    let buffer = context.create_buffer(gpu::BufferDesc {
        name: "stage",
        size: bytes,
        memory: gpu::Memory::Upload,
    });
    unsafe {
        std::ptr::copy_nonoverlapping(image.rgba.as_ptr(), buffer.data(), image.rgba.len());
    }
    context.sync_buffer(buffer.into(), bytes, gpu::BufferTarget::Data);
    {
        let mut transfer = encoder.transfer("upload");
        transfer.copy_buffer_to_texture(
            buffer.into(),
            image.width.max(1) * 4,
            texture.into(),
            gpu::Extent {
                width: image.width.max(1),
                height: image.height.max(1),
                depth: 1,
            },
        );
    }
    staging.push(buffer);
    gpu_textures.insert(key, (texture, view));
}

fn content_bounds(scene: &Scene, player_x: f32, player_z: f32) -> (f32, f32, f32, f32) {
    let mut min_x = player_x;
    let mut max_x = player_x;
    let mut min_z = player_z;
    let mut max_z = player_z;
    for object in &scene.objects {
        if !matches!(object.kind, "wall" | "scenery" | "critter" | "item") {
            continue;
        }
        if object.flat && object.kind != "item" {
            continue;
        }
        min_x = min_x.min(object.x);
        max_x = max_x.max(object.x);
        min_z = min_z.min(object.z);
        max_z = max_z.max(object.z);
    }
    const PAD: f32 = 6.0;
    (min_x - PAD, max_x + PAD, min_z - PAD, max_z + PAD)
}

fn build_atlas(game: &Game, palette: &Palette, bounds: (f32, f32, f32, f32)) -> Result<Atlas, String> {
    let (mut min_x, mut max_x, mut min_z, mut max_z) = bounds;
    min_x -= 24.0;
    max_x += 24.0;
    min_z -= 24.0;
    max_z += 24.0;
    let width = ((max_x - min_x) * ATLAS_SCALE).ceil().max(1.0) as u32;
    let height = ((max_z - min_z) * ATLAS_SCALE).ceil().max(1.0) as u32;
    if width > 8192 || height > 8192 {
        return Err(format!("ground atlas {width}x{height} exceeds 8192"));
    }
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let mut tiles: HashMap<String, Image> = HashMap::new();
    let mut stamped = 0u32;
    for floor in &game.scene.floors {
        let (sx, sy) = hexgrid::square_blit(floor.square);
        let origin = hexgrid::screen_to_world(sx as f32 + 40.0, sy as f32 + 18.0);
        if origin.0 < min_x - 4.0 || origin.0 > max_x + 4.0 || origin.1 < min_z - 4.0 || origin.1 > max_z + 4.0 {
            continue;
        }
        let image = if let Some(image) = tiles.get(&floor.path) {
            image
        } else {
            let frame = load_frm(&game.files, &floor.path)?.direction_frame(0)?.clone();
            let image = frame_image(&frame, palette);
            tiles.insert(floor.path.clone(), image);
            tiles.get(&floor.path).expect("just inserted")
        };
        let mut wrote = false;
        for y in 0..image.height {
            for x in 0..image.width {
                let src = ((y * image.width + x) * 4) as usize;
                if image.rgba[src + 3] == 0 {
                    continue;
                }
                let (wx, wz) = hexgrid::screen_to_world(sx as f32 + x as f32, sy as f32 + y as f32);
                let ax = ((wx - min_x) * ATLAS_SCALE).floor() as i32;
                let az = ((max_z - wz) * ATLAS_SCALE).floor() as i32;
                if ax < 0 || az < 0 || ax >= width as i32 || az >= height as i32 {
                    continue;
                }
                let dst = ((az as u32 * width + ax as u32) * 4) as usize;
                pixels[dst..dst + 4].copy_from_slice(&image.rgba[src..src + 4]);
                wrote = true;
            }
        }
        if wrote {
            stamped += 1;
        }
    }
    if stamped == 0 {
        return Err("no floor tiles fell inside the village bounds".into());
    }
    fill_cracks(&mut pixels, width, height);
    Ok(Atlas {
        width,
        height,
        min_x,
        max_z,
        pixels,
    })
}

fn fill_cracks(pixels: &mut [u8], width: u32, height: u32) {
    let width = width as i32;
    let height = height as i32;
    for _ in 0..2 {
        let snapshot = pixels.to_vec();
        for y in 0..height {
            for x in 0..width {
                let dest = ((y * width + x) * 4) as usize;
                if pixels[dest + 3] != 0 {
                    continue;
                }
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx < 0 || ny < 0 || nx >= width || ny >= height {
                        continue;
                    }
                    let src = ((ny * width + nx) * 4) as usize;
                    if snapshot[src + 3] > 0 {
                        pixels[dest..dest + 4].copy_from_slice(&snapshot[src..src + 4]);
                        break;
                    }
                }
            }
        }
    }
}

fn sprites_for_corner(
    game: &Game,
    palette: &Palette,
    corner: i32,
    images: &mut HashMap<(String, usize), Image>,
) -> Result<Vec<Quad>, String> {
    let (_, mut horizontal, _) = camera::camera_axes(corner, PITCH_DEG)?;
    horizontal[1] = 0.0;
    horizontal = camera::normalize(horizontal);
    let mut quads = Vec::new();
    for object in &game.scene.objects {
        if object.kind == "critter" && critter_is_dead(game, object.id) {
            continue;
        }
        if let Some(quad) = sprite_quad(
            game,
            palette,
            images,
            &object.path,
            object.facing,
            corner,
            object.x,
            object.z,
            object.flat,
            horizontal,
        )? {
            quads.push(quad);
        }
    }
    let (player_x, player_z) = hex_to_world(game.player_tile());
    if let Some(quad) = sprite_quad(
        game,
        palette,
        images,
        "art\\critters\\hmjmpsaa.frm",
        game.player_facing(),
        corner,
        player_x,
        player_z,
        false,
        horizontal,
    )? {
        quads.push(quad);
    }
    Ok(quads)
}

fn critter_is_dead(game: &Game, map_id: i32) -> bool {
    game.env.actors.iter().any(|actor| {
        actor.map_id == map_id && actor.handle > 1 && actor.kind == "critter" && !actor.alive
    })
}

fn sprite_quad(
    game: &Game,
    palette: &Palette,
    images: &mut HashMap<(String, usize), Image>,
    path: &str,
    facing: i32,
    corner: i32,
    x: f32,
    z: f32,
    flat: bool,
    horizontal_right: [f32; 3],
) -> Result<Option<Quad>, String> {
    let (art_path, direction, multi) = select_direction_art(&game.files, path, facing, corner)?;
    let key = (art_path.clone(), direction);
    if !images.contains_key(&key) {
        let frame = load_frm(&game.files, &art_path)?.direction_frame(direction)?.clone();
        images.insert(key.clone(), frame_image(&frame, palette));
    }
    let image = images.get(&key).expect("image");
    if image.width * image.height <= 4 {
        return Ok(None);
    }
    let width = image.width as f32 / 32.0;
    let height = image.height as f32 / 32.0;
    let anchor = [x, 0.02, z];
    let corners = if multi {
        upright_quad(anchor, width, height, horizontal_right)
    } else if flat {
        flat_quad(x, z, image.width as f32 / 32.0, image.height as f32 / 24.0)
    } else {
        let (_, mut right, _) = camera::camera_axes(0, PITCH_DEG)?;
        right[1] = 0.0;
        right = camera::normalize(right);
        upright_quad(anchor, width, height, right)
    };
    Ok(Some(Quad { corners, anchor, key }))
}

fn upright_quad(anchor: [f32; 3], width: f32, height: f32, right: [f32; 3]) -> [[f32; 3]; 4] {
    let half = camera::scale(right, width * 0.5);
    let base = [anchor[0], 0.02, anchor[2]];
    let left = camera::sub(base, half);
    let right_pt = camera::add(base, half);
    let up = [0.0, height, 0.0];
    [
        left,
        right_pt,
        camera::add(right_pt, up),
        camera::add(left, up),
    ]
}

fn flat_quad(x: f32, z: f32, width: f32, depth: f32) -> [[f32; 3]; 4] {
    let y = 0.03;
    [
        [x - width * 0.5, y, z],
        [x + width * 0.5, y, z],
        [x + width * 0.5, y, z + depth],
        [x - width * 0.5, y, z + depth],
    ]
}

fn frame_image(frame: &Frame, palette: &Palette) -> Image {
    Image {
        width: frame.width as u32,
        height: frame.height as u32,
        rgba: frame.rgba(palette),
    }
}

fn view_distance(corner: i32, bounds: (f32, f32, f32, f32)) -> f32 {
    let (min_x, max_x, min_z, max_z) = bounds;
    let target = [(min_x + max_x) * 0.5, 0.0, (min_z + max_z) * 0.5];
    let corners = [
        [min_x, 0.0, min_z],
        [max_x, 0.0, min_z],
        [max_x, 0.0, max_z],
        [min_x, 0.0, max_z],
    ];
    let (_, right, _) = camera::camera_axes(corner, PITCH_DEG).expect("pitch");
    let mut half = 0.0f32;
    for corner_point in corners {
        half = half.max(camera::dot(camera::sub(corner_point, target), right).abs());
    }
    let tan_half = (FOV_DEG * 0.5 * PI / 180.0).tan();
    (half / (0.90 * tan_half)).max(12.0)
}

fn clip_matrix(
    forward: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    eye: [f32; 3],
    fov_deg: f32,
    aspect: f32,
) -> [[f32; 4]; 4] {
    let focal = 1.0 / (fov_deg * 0.5 * PI / 180.0).tan();
    let near = 0.05;
    let far = 10000.0;
    let a = far / (far - near);
    let b = -far * near / (far - near);
    let right_eye = camera::dot(right, eye);
    let up_eye = camera::dot(up, eye);
    let forward_eye = camera::dot(forward, eye);
    [
        [focal * right[0], focal * aspect * up[0], a * forward[0], forward[0]],
        [focal * right[1], focal * aspect * up[1], a * forward[1], forward[1]],
        [focal * right[2], focal * aspect * up[2], a * forward[2], forward[2]],
        [
            focal * -right_eye,
            focal * aspect * -up_eye,
            a * -forward_eye + b,
            -forward_eye,
        ],
    ]
}

fn push_quad(vertices: &mut Vec<SpriteVertex>, corners: &[[f32; 3]; 4]) {
    let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    for index in [0, 1, 2, 0, 2, 3] {
        vertices.push(SpriteVertex {
            pos: corners[index],
            uv: uvs[index],
        });
    }
}
