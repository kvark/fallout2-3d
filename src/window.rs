//! Present one rendered frame in an X11/Vulkan window.
//!
//! `fallout2-3d --output DIR --window` keeps the first corner on screen for a few
//! seconds so a virtual display can capture it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use blade_graphics as gpu;
use blade_graphics::ShaderData;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::play::{Command, Game};
use crate::render::{self, RgbaImage};

const SHADER: &str = r#"
struct VertexIn {
    pos: vec2<f32>,
    uv: vec2<f32>,
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(vertex: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.position = vec4<f32>(vertex.pos, 0.0, 1.0);
    out.uv = vertex.uv;
    return out;
}

@fragment
fn fs_main(fragment: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(sprite_texture, sprite_sampler, fragment.uv);
}

var sprite_texture: texture_2d<f32>;
var sprite_sampler: sampler;
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BlitVertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

impl gpu::Vertex for BlitVertex {
    fn layout() -> gpu::VertexLayout {
        gpu::VertexLayout {
            attributes: vec![
                (
                    "pos",
                    gpu::VertexAttribute {
                        offset: 0,
                        format: gpu::VertexFormat::F32Vec2,
                    },
                ),
                (
                    "uv",
                    gpu::VertexAttribute {
                        offset: 8,
                        format: gpu::VertexFormat::F32Vec2,
                    },
                ),
            ],
            stride: 16,
        }
    }
}

struct BlitData {
    sprite_texture: gpu::TextureView,
    sprite_sampler: gpu::Sampler,
}

impl gpu::ShaderData for BlitData {
    fn layout() -> gpu::ShaderDataLayout {
        gpu::ShaderDataLayout {
            bindings: vec![
                ("sprite_texture", gpu::ShaderBinding::Texture),
                ("sprite_sampler", gpu::ShaderBinding::Sampler),
            ],
        }
    }

    fn fill(&self, mut ctx: gpu::PipelineContext) {
        use gpu::ShaderBindable as _;
        self.sprite_texture.bind_to(&mut ctx, 0);
        self.sprite_sampler.bind_to(&mut ctx, 1);
    }
}

struct Gpu {
    context: gpu::Context,
    device_name: String,
    surface: gpu::Surface,
    pipeline: gpu::RenderPipeline,
    sampler: gpu::Sampler,
    texture: gpu::Texture,
    view: gpu::TextureView,
    vertex_buf: gpu::Buffer,
    encoder: gpu::CommandEncoder,
    staging: gpu::Buffer,
}

struct App<'a> {
    image: RgbaImage,
    hold: Duration,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    shown_at: Option<Instant>,
    announced: bool,
    error: Option<String>,
    game: Option<&'a mut Game>,
    commands: Vec<Command>,
    output: Option<PathBuf>,
    scripted: bool,
    needs_render: bool,
}

impl App<'_> {
    fn ensure_gpu(&mut self, window: &Arc<Window>) -> Result<(), String> {
        if self.gpu.is_some() {
            return Ok(());
        }
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let context = unsafe {
            gpu::Context::init(gpu::ContextDesc {
                presentation: true,
                validation: false,
                timing: false,
                capture: false,
                overlay: false,
                ray_tracing: false,
                xr: None,
                device_id: None,
            })
        }
        .map_err(|err| format!("blade window context failed: {err}"))?;
        let device_name = context.device_information().device_name.clone();
        let surface = context
            .create_surface_configured(
                window.as_ref(),
                gpu::SurfaceConfig {
                    size: gpu::Extent {
                        width: size.width,
                        height: size.height,
                        depth: 1,
                    },
                    usage: gpu::TextureUsage::TARGET,
                    display_sync: gpu::DisplaySync::Block,
                    color_space: gpu::ColorSpace::Srgb,
                    transparent: false,
                    allow_exclusive_full_screen: false,
                },
            )
            .map_err(|err| format!("window surface failed: {err}"))?;
        let format = surface.info().format;
        println!("surface format: {format:?}");
        println!("window inner {}x{}", size.width, size.height);

        let shader = context.create_shader(gpu::ShaderDesc {
            source: SHADER,
            naga_module: None,
        });
        let data_layout = BlitData::layout();
        let vertex_layout = <BlitVertex as gpu::Vertex>::layout();
        let pipeline = context.create_render_pipeline(gpu::RenderPipelineDesc {
            name: "present-blit",
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
            depth_stencil: None,
            fragment: Some(shader.at("fs_main")),
            color_targets: &[gpu::ColorTargetState {
                format,
                blend: None,
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
        let texture = context.create_texture(gpu::TextureDesc {
            name: "frame",
            format: gpu::TextureFormat::Rgba8Unorm,
            size: gpu::Extent {
                width: self.image.width.max(1),
                height: self.image.height.max(1),
                depth: 1,
            },
            array_layer_count: 1,
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            usage: gpu::TextureUsage::RESOURCE | gpu::TextureUsage::COPY,
            external: None,
        });
        let view = context.create_texture_view(
            texture,
            gpu::TextureViewDesc {
                name: "frame",
                format: gpu::TextureFormat::Rgba8Unorm,
                dimension: gpu::ViewDimension::D2,
                subresources: &Default::default(),
            },
        );
        // Blade flips the viewport, so clip +Y is the top of the window.
        let vertices: [BlitVertex; 6] = [
            BlitVertex { pos: [-1.0, 1.0], uv: [0.0, 0.0] },
            BlitVertex { pos: [1.0, 1.0], uv: [1.0, 0.0] },
            BlitVertex { pos: [1.0, -1.0], uv: [1.0, 1.0] },
            BlitVertex { pos: [-1.0, 1.0], uv: [0.0, 0.0] },
            BlitVertex { pos: [1.0, -1.0], uv: [1.0, 1.0] },
            BlitVertex { pos: [-1.0, -1.0], uv: [0.0, 1.0] },
        ];
        let vertex_bytes = std::mem::size_of_val(&vertices) as u64;
        let vertex_buf = context.create_buffer(gpu::BufferDesc {
            name: "blit-vertices",
            size: vertex_bytes,
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

        let bytes = self.image.pixels.len().max(4) as u64;
        let staging = context.create_buffer(gpu::BufferDesc {
            name: "frame-stage",
            size: bytes,
            memory: gpu::Memory::Upload,
        });
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.image.pixels.as_ptr(),
                staging.data(),
                self.image.pixels.len(),
            );
        }
        context.sync_buffer(staging.into(), bytes, gpu::BufferTarget::Data);

        let mut encoder = context.create_command_encoder(gpu::CommandEncoderDesc {
            name: "present",
            buffer_count: 2,
            manual_barriers: false,
        });
        encoder.start();
        encoder.init_texture(texture);
        {
            let mut transfer = encoder.transfer("upload-frame");
            transfer.copy_buffer_to_texture(
                staging.into(),
                self.image.width.max(1) * 4,
                texture.into(),
                gpu::Extent {
                    width: self.image.width.max(1),
                    height: self.image.height.max(1),
                    depth: 1,
                },
            );
        }

        self.gpu = Some(Gpu {
            context,
            device_name,
            surface,
            pipeline,
            sampler,
            texture,
            view,
            vertex_buf,
            encoder,
            staging,
        });
        Ok(())
    }

    fn play_script(&mut self) -> Result<(), String> {
        let commands = std::mem::take(&mut self.commands);
        let output = self.output.clone();
        let image = {
            let Some(game) = self.game.as_mut() else {
                return Ok(());
            };
            crate::play::run_commands(game, commands)?;
            let rendered = render::render_corners(game)?;
            println!("blade context: {}", rendered.device_name);
            println!(
                "texture readback: {}x{}",
                render::FRAME_WIDTH,
                render::FRAME_HEIGHT
            );
            let _ = std::io::Write::flush(&mut std::io::stdout());
            if let Some(output) = &output {
                for (index, frame) in rendered.frames.iter().enumerate() {
                    let path = output.join(format!("corner{index}.png"));
                    render::write_png(&path, frame)
                        .map_err(|err| format!("write {} failed: {err}", path.display()))?;
                }
            }
            let corner = game.corner().rem_euclid(4) as usize;
            rendered
                .frames
                .get(corner)
                .cloned()
                .ok_or_else(|| "render produced no frame".to_string())?
        };
        self.image = image;
        Ok(())
    }

    fn rerender(&mut self) -> Result<(), String> {
        let image = {
            let Some(game) = self.game.as_mut() else {
                return Ok(());
            };
            let rendered = render::render_corners(game)?;
            let corner = game.corner().rem_euclid(4) as usize;
            rendered
                .frames
                .get(corner)
                .cloned()
                .ok_or_else(|| "render produced no frame".to_string())?
        };
        self.image = image;
        Ok(())
    }

    /// Keys use the same commands as the scripted session.
    /// 7/9/6/3/1/4 step NW/NE/E/SE/SW/W. t talks, g/b choose option 0/1.
    /// c starts combat, v/f/r are unarmed/melee/shot, Enter ends the turn.
    fn key_command(&mut self, key: &Key) -> Option<Command> {
        let game = self.game.as_ref()?;
        let character = match key {
            Key::Character(text) => text.as_str(),
            Key::Named(NamedKey::Enter) => return Some(Command::EndTurn),
            _ => return None,
        };
        let talker = game.dialogue_target().or_else(|| {
            game.env
                .actors
                .iter()
                .find(|actor| actor.handle > 1 && actor.script_lst >= 0)
                .map(|actor| actor.map_id)
        });
        let foe = game.combat_target().or_else(|| {
            game.env
                .actors
                .iter()
                .filter(|actor| actor.handle > 1 && actor.kind == "critter" && actor.alive)
                .map(|actor| actor.map_id)
                .min()
        });
        match character {
            "9" => Some(Command::Step(0)),
            "6" => Some(Command::Step(1)),
            "3" => Some(Command::Step(2)),
            "1" => Some(Command::Step(3)),
            "4" => Some(Command::Step(4)),
            "7" => Some(Command::Step(5)),
            "t" => talker.map(Command::Talk),
            "g" if game.dialogue_len() > 0 => Some(Command::Choose(0)),
            "b" if game.dialogue_len() > 1 => Some(Command::Choose(1)),
            "c" => foe.map(Command::StartCombat),
            "v" => foe.map(|object| Command::Attack {
                object,
                kind: crate::AttackKind::Unarmed,
            }),
            "f" => foe.map(|object| Command::Attack {
                object,
                kind: crate::AttackKind::Melee,
            }),
            "r" => foe.map(|object| Command::Attack {
                object,
                kind: crate::AttackKind::Shot,
            }),
            _ => None,
        }
    }

    fn draw(&mut self, window: &Arc<Window>) -> Result<(), String> {
        if !self.scripted {
            self.play_script()?;
            self.scripted = true;
        } else if self.needs_render {
            self.gpu = None;
            self.rerender()?;
            self.needs_render = false;
        }
        self.ensure_gpu(window)?;
        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        let frame = gpu.surface.acquire_frame();
        {
            let mut pass = gpu.encoder.render(
                "window",
                gpu::RenderTargetSet {
                    colors: &[gpu::RenderTarget {
                        view: frame.texture_view(),
                        init_op: gpu::InitOp::Clear(gpu::TextureColor::OpaqueBlack),
                        finish_op: gpu::FinishOp::Store,
                    }],
                    depth_stencil: None,
                },
            );
            let mut pipe = pass.with(&gpu.pipeline);
            pipe.bind_vertex(0, gpu.vertex_buf.into());
            pipe.bind(
                0,
                &BlitData {
                    sprite_texture: gpu.view,
                    sprite_sampler: gpu.sampler,
                },
            );
            pipe.draw(0, 6, 0, 1);
        }
        gpu.encoder.present(frame);
        let sync = gpu.context.submit(&mut gpu.encoder);
        gpu.context
            .wait_for(&sync, u32::MAX)
            .map_err(|err| format!("present wait failed: {err}"))?;
        gpu.encoder.start();
        if !self.announced {
            self.announced = true;
            self.shown_at = Some(Instant::now());
            println!("blade context: {}", gpu.device_name);
            println!(
                "window presented: {}x{}",
                self.image.width, self.image.height
            );
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
        Ok(())
    }
}

impl ApplicationHandler for App<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Poll);
        if self.window.is_some() {
            return;
        }
        match event_loop.create_window(
            Window::default_attributes()
                .with_title("fallout2-3d")
                .with_decorations(false)
                .with_resizable(false)
                .with_position(PhysicalPosition::new(40, 40))
                .with_inner_size(PhysicalSize::new(640, 360)),
        ) {
            Ok(window) => {
                window.request_redraw();
                self.window = Some(Arc::new(window));
            }
            Err(err) => {
                self.error = Some(format!("create window failed: {err}"));
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed || event.repeat || !self.scripted {
                    return;
                }
                let turn = match &event.logical_key {
                    Key::Character(text) if text.as_str() == "]" => Some(1),
                    Key::Character(text) if text.as_str() == "[" => Some(-1),
                    _ => None,
                };
                if let Some(dir) = turn {
                    if let Some(game) = self.game.as_mut() {
                        if dir > 0 {
                            game.advance_corner();
                        } else {
                            game.back_corner();
                        }
                        println!("window corner {}", game.corner());
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                    }
                    self.needs_render = true;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                if let Some(command) = self.key_command(&event.logical_key) {
                    let result = self
                        .game
                        .as_mut()
                        .map(|game| game.command(command))
                        .transpose();
                    match result {
                        Ok(Some(outcome)) => {
                            println!("window {}", outcome.summary);
                            let _ = std::io::Write::flush(&mut std::io::stdout());
                            self.needs_render = true;
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                        Ok(None) => {}
                        Err(err) => {
                            self.error = Some(err);
                            event_loop.exit();
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let Some(window) = self.window.clone() else {
                    return;
                };
                if let Err(err) = self.draw(&window) {
                    self.error = Some(err);
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(started) = self.shown_at {
            if started.elapsed() >= self.hold {
                event_loop.exit();
                return;
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

/// Play `commands` on `game`, re-render, and keep the current corner on screen.
/// Further keypresses run the same commands and render again.
pub fn play(
    game: &mut Game,
    commands: Vec<Command>,
    output: Option<&Path>,
    hold: Duration,
) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|err| format!("event loop failed: {err}"))?;
    let mut app = App {
        image: RgbaImage {
            width: 1,
            height: 1,
            pixels: vec![0, 0, 0, 255],
        },
        hold,
        window: None,
        gpu: None,
        shown_at: None,
        announced: false,
        error: None,
        game: Some(game),
        commands,
        output: output.map(Path::to_path_buf),
        scripted: false,
        needs_render: false,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|err| format!("window loop failed: {err}"))?;
    match app.error.take() {
        Some(err) => Err(err),
        None if app.announced => Ok(()),
        None => Err("window closed before a frame was presented".into()),
    }
}

/// Show `image` in a window and keep it there for `hold`.
pub fn present_image(image: RgbaImage, hold: Duration) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|err| format!("event loop failed: {err}"))?;
    let mut app = App {
        image,
        hold,
        window: None,
        gpu: None,
        shown_at: None,
        announced: false,
        error: None,
        game: None,
        commands: Vec::new(),
        output: None,
        scripted: true,
        needs_render: false,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|err| format!("window loop failed: {err}"))?;
    match app.error.take() {
        Some(err) => Err(err),
        None if app.announced => Ok(()),
        None => Err("window closed before a frame was presented".into()),
    }
}
