//! Offscreen WGPU compositor. One device, CPU upload + readback (bounded-copy fallback).
use crate::media::{validate_dim, ColorInfo, PixelFormat, VideoFrame, MAX_FRAME_DIM};
use crate::project::layout::{validate_wallpaper_relative, MAX_WALLPAPER_BYTES};
use crate::project::reader::{open_regular, safe_path};
use crate::project::EditLayout;
use bytemuck::{Pod, Zeroable};
use std::io::Read;
use std::path::Path;
use wgpu::util::DeviceExt;

pub const COPIES_COMPOSITE: u32 = 2;
pub const MAX_LAYERS: usize = 4;
const SHADER: &str = include_str!("composite.wgsl");
const WEBCAM_SHADOW_BLUR_PX: f32 = 16.0;
const WEBCAM_SHADOW_OPACITY: f32 = 0.55;
const SHADOW_OFFSET_FACTOR: f32 = 0.35;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    local: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LayerParams {
    size_px: [f32; 2],
    radius_px: f32,
    clip_mode: u32,
    shadow_blur_px: f32,
    shadow_opacity: f32,
    shadow_offset: [f32; 2],
    pass_kind: u32,
    _pad: [u32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LayerRole {
    Background,
    #[default]
    Screen,
    WebcamBorder,
    Webcam,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ClipMode {
    #[default]
    None = 0,
    RoundedRect = 1,
    Circle = 2,
    Squircle = 3,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub frame: VideoFrame,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub uv_x: f32,
    pub uv_y: f32,
    pub uv_w: f32,
    pub uv_h: f32,
    pub role: LayerRole,
    pub clip: ClipMode,
    pub radius_px: f32,
    pub shadow_blur_px: f32,
    pub shadow_opacity: f32,
    pub shadow_offset: [f32; 2],
}

impl Layer {
    pub fn placed(frame: VideoFrame, x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            frame,
            x,
            y,
            width,
            height,
            uv_x: 0.0,
            uv_y: 0.0,
            uv_w: 1.0,
            uv_h: 1.0,
            role: LayerRole::Screen,
            clip: ClipMode::None,
            radius_px: 0.0,
            shadow_blur_px: 0.0,
            shadow_opacity: 0.0,
            shadow_offset: [0.0, 0.0],
        }
    }

    pub fn with_role(mut self, role: LayerRole) -> Self {
        self.role = role;
        self
    }

    pub fn with_clip(mut self, clip: ClipMode, radius_px: f32) -> Self {
        self.clip = clip;
        self.radius_px = radius_px.max(0.0);
        self
    }

    pub fn with_shadow(mut self, blur_px: f32, opacity: f32) -> Self {
        self.shadow_blur_px = blur_px.max(0.0);
        self.shadow_opacity = opacity.clamp(0.0, 1.0);
        self.shadow_offset = [0.0, self.shadow_blur_px * SHADOW_OFFSET_FACTOR];
        self
    }

    pub fn mirrored(mut self) -> Self {
        self.uv_x += self.uv_w;
        self.uv_w = -self.uv_w;
        self
    }

    pub fn cover_uv(mut self, dest_w: u32, dest_h: u32) -> Self {
        let (uv_x, uv_y, uv_w, uv_h) =
            cover_uv(self.frame.width, self.frame.height, dest_w, dest_h);
        self.uv_x = uv_x;
        self.uv_y = uv_y;
        self.uv_w = uv_w;
        self.uv_h = uv_h;
        self
    }

    fn has_shadow(&self) -> bool {
        self.shadow_blur_px > 0.0 && self.shadow_opacity > 0.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub background: [f32; 4],
    pub layers: Vec<Layer>,
}

impl Scene {
    pub fn styled_preview(screen: VideoFrame, webcam: Option<VideoFrame>) -> Result<Self, String> {
        validate_dim(64, 64)?;
        let mut layers = vec![Layer::placed(screen, 12, 16, 40, 32)];
        if let Some(webcam) = webcam {
            layers.push(Layer::placed(webcam, 46, 4, 12, 12));
        }
        Ok(Self {
            width: 64,
            height: 64,
            background: [0.05, 0.12, 0.28, 1.0],
            layers,
        })
    }

    pub fn from_layout(
        width: u32,
        height: u32,
        layout: &EditLayout,
        screen: Option<VideoFrame>,
        webcam: Option<VideoFrame>,
    ) -> Result<Self, String> {
        Self::from_layout_with_wallpaper(width, height, layout, screen, webcam, None)
    }

    pub fn from_layout_with_wallpaper(
        width: u32,
        height: u32,
        layout: &EditLayout,
        screen: Option<VideoFrame>,
        webcam: Option<VideoFrame>,
        wallpaper: Option<VideoFrame>,
    ) -> Result<Self, String> {
        validate_dim(width, height)?;
        layout.validate()?;
        if layout.padding_px.saturating_mul(2) >= width
            || layout.padding_px.saturating_mul(2) >= height
        {
            return Err("Export padding leaves no content rectangle".into());
        }
        let (background, background_end) = layout.background_rgba()?;
        let content_x = layout.padding_px;
        let content_y = layout.padding_px;
        let content_w = width - layout.padding_px * 2;
        let content_h = height - layout.padding_px * 2;
        let mut layers = Vec::new();
        if let Some(paper) = wallpaper {
            layers.push(
                Layer::placed(paper, 0, 0, width, height)
                    .with_role(LayerRole::Background)
                    .cover_uv(width, height),
            );
        } else if let Some(end) = background_end {
            let gradient = gradient_frame(width, height, background, end)?;
            layers.push(Layer::placed(gradient, 0, 0, width, height).with_role(LayerRole::Background));
        }
        let screen_clip = if layout.corner_radius_px > 0 {
            ClipMode::RoundedRect
        } else {
            ClipMode::None
        };
        let screen_radius = layout.corner_radius_px as f32;
        let screen_shadow = if layout.shadow_blur_px > 0 && layout.shadow_opacity > 0.0 {
            Some((layout.shadow_blur_px as f32, layout.shadow_opacity))
        } else {
            None
        };
        if let Some(screen) = screen {
            let (x, y, w, h) = fit_inside(
                screen.width,
                screen.height,
                content_x,
                content_y,
                content_w,
                content_h,
            );
            let mut layer = Layer::placed(screen, x, y, w, h)
                .with_role(LayerRole::Screen)
                .with_clip(screen_clip, screen_radius);
            if let Some((blur, opacity)) = screen_shadow {
                layer = layer.with_shadow(blur, opacity);
            }
            layers.push(layer);
        }
        if layout.webcam_enabled {
            if let Some(webcam) = webcam {
                let (bw, bh) = webcam_bubble_size(width, height, webcam.width, webcam.height, layout);
                let (x, y) = webcam_origin(width, height, bw, bh, layout);
                let (cam_clip, cam_radius) = webcam_clip(layout);
                let cam_shadow = if layout.webcam_shadow {
                    Some((WEBCAM_SHADOW_BLUR_PX, WEBCAM_SHADOW_OPACITY))
                } else {
                    None
                };
                if layout.webcam_border_width > 0 {
                    let inset = layout.webcam_border_width;
                    let bx = x.saturating_sub(inset);
                    let by = y.saturating_sub(inset);
                    let bwidth = (bw + inset * 2).min(width.saturating_sub(bx)).max(1);
                    let bheight = (bh + inset * 2).min(height.saturating_sub(by)).max(1);
                    let [r, g, b] = crate::project::layout::parse_hex_rgb(&layout.webcam_border_color)?;
                    let border = VideoFrame::solid(8, 8, b, g, r, 0)?;
                    let mut border_layer = Layer::placed(border, bx, by, bwidth, bheight)
                        .with_role(LayerRole::WebcamBorder)
                        .with_clip(cam_clip, cam_radius);
                    if let Some((blur, opacity)) = cam_shadow {
                        border_layer = border_layer.with_shadow(blur, opacity);
                    }
                    layers.push(border_layer);
                }
                let mut bubble = Layer::placed(webcam, x, y, bw, bh)
                    .with_role(LayerRole::Webcam)
                    .with_clip(cam_clip, cam_radius);
                if matches!(layout.webcam_shape.as_str(), "circle" | "squircle") {
                    bubble = bubble.cover_uv(bw, bh);
                }
                if layout.webcam_border_width == 0 {
                    if let Some((blur, opacity)) = cam_shadow {
                        bubble = bubble.with_shadow(blur, opacity);
                    }
                }
                if layout.webcam_mirror {
                    bubble = bubble.mirrored();
                }
                layers.push(bubble);
            }
        }
        if layers.len() > MAX_LAYERS {
            return Err("Compositor layer count exceeds the F2 bound".into());
        }
        Ok(Self {
            width,
            height,
            background,
            layers,
        })
    }

    pub fn apply_screen_uv(&mut self, uv_x: f32, uv_y: f32, uv_w: f32, uv_h: f32) {
        if let Some(layer) = self
            .layers
            .iter_mut()
            .find(|layer| layer.role == LayerRole::Screen)
        {
            layer.uv_x = uv_x;
            layer.uv_y = uv_y;
            layer.uv_w = uv_w;
            layer.uv_h = uv_h;
        }
    }
}

struct LayerDraw {
    _texture: wgpu::Texture,
    _view: wgpu::TextureView,
    _content_uniform: wgpu::Buffer,
    bind_content: wgpu::BindGroup,
    v_content: wgpu::Buffer,
    shadow: Option<(wgpu::BindGroup, wgpu::Buffer, wgpu::Buffer)>,
}

pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_layout: wgpu::BindGroupLayout,
    adapter_name: String,
    copies: u32,
}

impl Compositor {
    pub fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .map_err(|e| format!("No GPU adapter for the F2 compositor: {e}"))?;
        let adapter_name = adapter.get_info().name;
        let mut desc = wgpu::DeviceDescriptor::default();
        desc.label = Some("aeroshoot-compositor");
        let (device, queue) = pollster::block_on(adapter.request_device(&desc))
            .map_err(|e| format!("Failed to open the compositor device: {e}"))?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("aeroshoot-composite"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("aeroshoot-layer"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("aeroshoot-composite-layout"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("aeroshoot-composite-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2,
                        1 => Float32x2,
                        2 => Float32x2
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("aeroshoot-nearest"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Ok(Self {
            device,
            queue,
            pipeline,
            sampler,
            bind_layout,
            adapter_name,
            copies: COPIES_COMPOSITE,
        })
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub fn copies(&self) -> u32 {
        self.copies
    }

    pub fn composite_cpu(scene: &Scene) -> Result<VideoFrame, String> {
        validate_dim(scene.width, scene.height)?;
        let r = (scene.background[0].clamp(0.0, 1.0) * 255.0).round() as u8;
        let g = (scene.background[1].clamp(0.0, 1.0) * 255.0).round() as u8;
        let b = (scene.background[2].clamp(0.0, 1.0) * 255.0).round() as u8;
        let mut frame = VideoFrame::solid(scene.width, scene.height, b, g, r, 0)?;
        for layer in &scene.layers {
            if layer.has_shadow() {
                blit_shadow(&mut frame, layer)?;
            }
            blit_nearest(&mut frame, layer)?;
        }
        if let Some(first) = scene.layers.first() {
            frame.pts_us = first.frame.pts_us;
        }
        Ok(frame)
    }

    pub fn composite(&self, scene: &Scene) -> Result<VideoFrame, String> {
        validate_dim(scene.width, scene.height)?;
        if scene.layers.len() > MAX_LAYERS {
            return Err("Compositor layer count exceeds the F2 bound".into());
        }
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("aeroshoot-target"),
            size: wgpu::Extent3d {
                width: scene.width,
                height: scene.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut draws = Vec::new();
        for layer in &scene.layers {
            validate_dim(layer.frame.width, layer.frame.height)?;
            if layer.frame.width > MAX_FRAME_DIM || layer.frame.height > MAX_FRAME_DIM {
                return Err("Layer exceeds the compositor working-set limit".into());
            }
            let rgba = bgra_to_rgba(&layer.frame)?;
            let padded_row = padded_bytes_per_row(layer.frame.width);
            let mut upload = vec![0u8; (padded_row * layer.frame.height) as usize];
            let tight = layer.frame.width * 4;
            for y in 0..layer.frame.height {
                let src = (y * tight) as usize;
                let dst = (y * padded_row) as usize;
                upload[dst..dst + tight as usize].copy_from_slice(&rgba[src..src + tight as usize]);
            }
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("aeroshoot-layer"),
                size: wgpu::Extent3d {
                    width: layer.frame.width,
                    height: layer.frame.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &upload,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(layer.frame.height),
                },
                wgpu::Extent3d {
                    width: layer.frame.width,
                    height: layer.frame.height,
                    depth_or_array_layers: 1,
                },
            );
            let layer_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let content_params = layer_params(layer, 0);
            let content_uniform = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("aeroshoot-layer-params"),
                contents: bytemuck::bytes_of(&content_params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind_content = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("aeroshoot-layer-bind"),
                layout: &self.bind_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&layer_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: content_uniform.as_entire_binding(),
                    },
                ],
            });
            let v_content = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("aeroshoot-quad"),
                contents: bytemuck::cast_slice(&quad_vertices(scene.width, scene.height, layer, 0.0)),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let shadow = if layer.has_shadow() {
                let pad = shadow_pad(layer);
                let shadow_params = layer_params(layer, 1);
                let shadow_uniform =
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("aeroshoot-shadow-params"),
                            contents: bytemuck::bytes_of(&shadow_params),
                            usage: wgpu::BufferUsages::UNIFORM,
                        });
                let bind_shadow = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("aeroshoot-shadow-bind"),
                    layout: &self.bind_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&layer_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: shadow_uniform.as_entire_binding(),
                        },
                    ],
                });
                let v_shadow = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("aeroshoot-shadow-quad"),
                    contents: bytemuck::cast_slice(&quad_vertices(
                        scene.width,
                        scene.height,
                        layer,
                        pad,
                    )),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                Some((bind_shadow, v_shadow, shadow_uniform))
            } else {
                None
            };
            draws.push(LayerDraw {
                _texture: texture,
                _view: layer_view,
                _content_uniform: content_uniform,
                bind_content,
                v_content,
                shadow,
            });
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("aeroshoot-composite-enc"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("aeroshoot-layers"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: scene.background[0] as f64,
                            g: scene.background[1] as f64,
                            b: scene.background[2] as f64,
                            a: scene.background[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            for draw in &draws {
                if let Some((bind, vbuf, _)) = &draw.shadow {
                    pass.set_bind_group(0, bind, &[]);
                    pass.set_vertex_buffer(0, vbuf.slice(..));
                    pass.draw(0..6, 0..1);
                }
                pass.set_bind_group(0, &draw.bind_content, &[]);
                pass.set_vertex_buffer(0, draw.v_content.slice(..));
                pass.draw(0..6, 0..1);
            }
        }

        let padded = padded_bytes_per_row(scene.width);
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aeroshoot-readback"),
            size: u64::from(padded * scene.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(scene.height),
                },
            },
            wgpu::Extent3d {
                width: scene.width,
                height: scene.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| ());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| format!("Compositor readback poll failed: {e}"))?;
        let data = slice.get_mapped_range();
        let mut packed = Vec::with_capacity((scene.width * scene.height * 4) as usize);
        for row in 0..scene.height {
            let start = (row * padded) as usize;
            packed.extend_from_slice(&data[start..start + (scene.width * 4) as usize]);
        }
        drop(data);
        staging.unmap();
        let bgra = rgba_to_bgra(&packed);
        let _keep = draws;
        Ok(VideoFrame {
            pts_us: scene.layers.first().map(|l| l.frame.pts_us).unwrap_or(0),
            width: scene.width,
            height: scene.height,
            stride: scene.width * 4,
            format: PixelFormat::Bgra8888,
            color: ColorInfo::rec709_full(),
            data: bgra,
        })
    }
}

fn fit_inside(
    src_w: u32,
    src_h: u32,
    dest_x: u32,
    dest_y: u32,
    dest_w: u32,
    dest_h: u32,
) -> (u32, u32, u32, u32) {
    if src_w == 0 || src_h == 0 || dest_w == 0 || dest_h == 0 {
        return (dest_x, dest_y, dest_w.max(1), dest_h.max(1));
    }
    let src_a = src_w as f64 / src_h as f64;
    let dest_a = dest_w as f64 / dest_h as f64;
    let (w, h) = if src_a > dest_a {
        let w = dest_w;
        let h = ((dest_w as f64 / src_a).round() as u32).clamp(1, dest_h);
        (w, h)
    } else {
        let h = dest_h;
        let w = ((dest_h as f64 * src_a).round() as u32).clamp(1, dest_w);
        (w, h)
    };
    let x = dest_x + dest_w.saturating_sub(w) / 2;
    let y = dest_y + dest_h.saturating_sub(h) / 2;
    (x, y, w, h)
}

fn webcam_bubble_size(
    canvas_w: u32,
    canvas_h: u32,
    src_w: u32,
    src_h: u32,
    layout: &EditLayout,
) -> (u32, u32) {
    let frac = match layout.webcam_size.as_str() {
        "sm" => 0.125,
        "lg" => 0.28,
        "xl" => 0.36,
        _ => 0.2,
    };
    let target = ((canvas_w.min(canvas_h) as f32) * frac).round() as u32;
    let target = target.max(8);
    if src_w == 0 || src_h == 0 {
        return (target, target);
    }
    let (w, h) = if src_w >= src_h {
        let h = ((target as f64 * src_h as f64 / src_w as f64).round() as u32).max(8);
        (target.min(canvas_w).max(1), h.min(canvas_h).max(1))
    } else {
        let w = ((target as f64 * src_w as f64 / src_h as f64).round() as u32).max(8);
        (w.min(canvas_w).max(1), target.min(canvas_h).max(1))
    };
    if matches!(layout.webcam_shape.as_str(), "circle" | "squircle") {
        let side = w.min(h).max(8);
        return (
            side.min(canvas_w).max(1),
            side.min(canvas_h).max(1),
        );
    }
    (w, h)
}

fn webcam_origin(
    canvas_w: u32,
    canvas_h: u32,
    bubble_w: u32,
    bubble_h: u32,
    layout: &EditLayout,
) -> (u32, u32) {
    let max_x = canvas_w.saturating_sub(bubble_w);
    let max_y = canvas_h.saturating_sub(bubble_h);
    let margin = 8u32.min(max_x / 2).min(max_y / 2);
    match layout.webcam_position.as_str() {
        "top-left" => (margin.min(max_x), margin.min(max_y)),
        "top-right" => (max_x.saturating_sub(margin), margin.min(max_y)),
        "bottom-left" => (margin.min(max_x), max_y.saturating_sub(margin)),
        "custom" => {
            let x = (layout.webcam_custom_x.clamp(0.0, 100.0) / 100.0 * max_x as f32).round() as u32;
            let y = (layout.webcam_custom_y.clamp(0.0, 100.0) / 100.0 * max_y as f32).round() as u32;
            (x.min(max_x), y.min(max_y))
        }
        _ => (
            max_x.saturating_sub(margin),
            max_y.saturating_sub(margin),
        ),
    }
}

fn gradient_frame(
    width: u32,
    height: u32,
    start: [f32; 4],
    end: [f32; 4],
) -> Result<VideoFrame, String> {
    let mut frame = VideoFrame::solid(
        width,
        height,
        (start[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        (start[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (start[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        0,
    )?;
    let denom_x = (width.saturating_sub(1)).max(1) as f32;
    let denom_y = (height.saturating_sub(1)).max(1) as f32;
    for y in 0..height {
        for x in 0..width {
            let t = 0.5 * (x as f32 / denom_x + y as f32 / denom_y);
            let lerp = |a: f32, b: f32| a + (b - a) * t;
            let i = (y * frame.stride + x * 4) as usize;
            frame.data[i] = (lerp(start[2], end[2]).clamp(0.0, 1.0) * 255.0).round() as u8;
            frame.data[i + 1] = (lerp(start[1], end[1]).clamp(0.0, 1.0) * 255.0).round() as u8;
            frame.data[i + 2] = (lerp(start[0], end[0]).clamp(0.0, 1.0) * 255.0).round() as u8;
            frame.data[i + 3] = 255;
        }
    }
    Ok(frame)
}

fn webcam_clip(layout: &EditLayout) -> (ClipMode, f32) {
    match layout.webcam_shape.as_str() {
        "circle" => (ClipMode::Circle, 0.0),
        "squircle" => (ClipMode::Squircle, 0.0),
        _ => (ClipMode::None, 0.0),
    }
}

fn cover_uv(src_w: u32, src_h: u32, dest_w: u32, dest_h: u32) -> (f32, f32, f32, f32) {
    if src_w == 0 || src_h == 0 || dest_w == 0 || dest_h == 0 {
        return (0.0, 0.0, 1.0, 1.0);
    }
    let src_a = src_w as f32 / src_h as f32;
    let dest_a = dest_w as f32 / dest_h as f32;
    if src_a > dest_a {
        let vis_w = dest_a / src_a;
        ((1.0 - vis_w) * 0.5, 0.0, vis_w, 1.0)
    } else {
        let vis_h = src_a / dest_a;
        (0.0, (1.0 - vis_h) * 0.5, 1.0, vis_h)
    }
}

fn sdf_rounded_rect(px: f32, py: f32, half_w: f32, half_h: f32, radius: f32) -> f32 {
    let r = radius.min(half_w).min(half_h).max(0.0);
    let qx = px.abs() - (half_w - r);
    let qy = py.abs() - (half_h - r);
    let outside_x = qx.max(0.0);
    let outside_y = qy.max(0.0);
    (outside_x * outside_x + outside_y * outside_y).sqrt() + qx.max(qy).min(0.0) - r
}

fn sdf_circle(px: f32, py: f32, half_w: f32, half_h: f32) -> f32 {
    (px * px + py * py).sqrt() - half_w.min(half_h)
}

fn sdf_squircle(px: f32, py: f32, half_w: f32, half_h: f32) -> f32 {
    let nx = if half_w > 0.0 { px.abs() / half_w } else { 0.0 };
    let ny = if half_h > 0.0 { py.abs() / half_h } else { 0.0 };
    let k = nx * nx * nx * nx + ny * ny * ny * ny;
    let r = half_w.min(half_h);
    k.max(0.0).powf(0.25) * r - r
}

fn layer_sdf(layer: &Layer, local_x: f32, local_y: f32, shadow: bool) -> f32 {
    let mut px = (local_x - 0.5) * layer.width as f32;
    let mut py = (local_y - 0.5) * layer.height as f32;
    if shadow {
        px -= layer.shadow_offset[0];
        py -= layer.shadow_offset[1];
    }
    let half_w = layer.width as f32 * 0.5;
    let half_h = layer.height as f32 * 0.5;
    match layer.clip {
        ClipMode::Circle => sdf_circle(px, py, half_w, half_h),
        ClipMode::Squircle => sdf_squircle(px, py, half_w, half_h),
        ClipMode::RoundedRect | ClipMode::None => {
            sdf_rounded_rect(px, py, half_w, half_h, layer.radius_px)
        }
    }
}

fn shadow_coverage(sdf: f32, blur: f32) -> f32 {
    if blur <= 0.0 {
        if sdf <= 0.0 {
            1.0
        } else {
            0.0
        }
    } else {
        1.0 - (sdf / blur).clamp(0.0, 1.0)
    }
}

fn shadow_pad(layer: &Layer) -> f32 {
    layer.shadow_blur_px
        + layer.shadow_offset[0].abs()
        + layer.shadow_offset[1].abs()
}

fn layer_params(layer: &Layer, pass_kind: u32) -> LayerParams {
    LayerParams {
        size_px: [layer.width as f32, layer.height as f32],
        radius_px: layer.radius_px,
        clip_mode: layer.clip as u32,
        shadow_blur_px: layer.shadow_blur_px,
        shadow_opacity: layer.shadow_opacity,
        shadow_offset: layer.shadow_offset,
        pass_kind,
        _pad: [0, 0, 0],
    }
}

/// Decode a project-relative wallpaper asset. Never follows URLs or symlinks.
pub fn load_wallpaper_frame(
    root: &Path,
    layout: &EditLayout,
    _canvas_w: u32,
    _canvas_h: u32,
) -> Result<Option<VideoFrame>, String> {
    if layout.background_type != "wallpaper" {
        return Ok(None);
    }
    let relative = layout
        .wallpaper_asset
        .as_deref()
        .ok_or_else(|| "Wallpaper background requires a project asset".to_string())?;
    if relative.contains("://") {
        return Err("Wallpaper cannot be an external URL".into());
    }
    validate_wallpaper_relative(relative)?;
    let path = safe_path(root, relative)?;
    let file = open_regular(&path)?;
    let mut bytes = Vec::new();
    file.take(MAX_WALLPAPER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_WALLPAPER_BYTES {
        return Err("Wallpaper exceeds size limit".into());
    }
    let img = image::load_from_memory(&bytes)
        .map_err(|e| format!("Wallpaper decode failed: {e}"))?
        .to_rgba8();
    if img.width() > MAX_FRAME_DIM || img.height() > MAX_FRAME_DIM || img.width() == 0 || img.height() == 0
    {
        return Err("Wallpaper exceeds compositor working-set limit".into());
    }
    validate_dim(img.width(), img.height())?;
    let mut frame = VideoFrame::solid(img.width(), img.height(), 0, 0, 0, 0)?;
    for y in 0..img.height() {
        for x in 0..img.width() {
            let p = img[(x, y)].0;
            let i = (y * frame.stride + x * 4) as usize;
            frame.data[i] = p[2];
            frame.data[i + 1] = p[1];
            frame.data[i + 2] = p[0];
            frame.data[i + 3] = 255;
        }
    }
    Ok(Some(frame))
}

fn blit_shadow(dest: &mut VideoFrame, layer: &Layer) -> Result<(), String> {
    if layer.width == 0 || layer.height == 0 {
        return Err("Layer size is empty".into());
    }
    let pad = shadow_pad(layer).ceil() as i32;
    let x0 = layer.x as i32 - pad;
    let y0 = layer.y as i32 - pad;
    let x1 = layer.x as i32 + layer.width as i32 + pad;
    let y1 = layer.y as i32 + layer.height as i32 + pad;
    for y in y0.max(0)..y1.min(dest.height as i32) {
        for x in x0.max(0)..x1.min(dest.width as i32) {
            let local_x = (x as f32 + 0.5 - layer.x as f32) / layer.width as f32;
            let local_y = (y as f32 + 0.5 - layer.y as f32) / layer.height as f32;
            let sdf = layer_sdf(layer, local_x, local_y, true);
            let alpha = layer.shadow_opacity * shadow_coverage(sdf, layer.shadow_blur_px);
            if alpha <= 0.0 {
                continue;
            }
            let di = (y as u32 * dest.stride + x as u32 * 4) as usize;
            let keep = 1.0 - alpha.clamp(0.0, 1.0);
            dest.data[di] = (dest.data[di] as f32 * keep).round() as u8;
            dest.data[di + 1] = (dest.data[di + 1] as f32 * keep).round() as u8;
            dest.data[di + 2] = (dest.data[di + 2] as f32 * keep).round() as u8;
        }
    }
    Ok(())
}

fn blit_nearest(dest: &mut VideoFrame, layer: &Layer) -> Result<(), String> {
    if layer.width == 0 || layer.height == 0 {
        return Err("Layer size is empty".into());
    }
    for dy in 0..layer.height {
        let y = layer.y + dy;
        if y >= dest.height {
            continue;
        }
        let src_y = {
            let v = layer.uv_y + layer.uv_h * ((dy as f32 + 0.5) / layer.height as f32);
            let y = (v * layer.frame.height as f32).floor() as i64;
            y.clamp(0, layer.frame.height as i64 - 1) as u32
        };
        for dx in 0..layer.width {
            let x = layer.x + dx;
            if x >= dest.width {
                continue;
            }
            let local_x = (dx as f32 + 0.5) / layer.width as f32;
            let local_y = (dy as f32 + 0.5) / layer.height as f32;
            if layer_sdf(layer, local_x, local_y, false) > 0.0 {
                continue;
            }
            let src_x = {
                let u = layer.uv_x + layer.uv_w * ((dx as f32 + 0.5) / layer.width as f32);
                let sx = (u * layer.frame.width as f32).floor() as i64;
                sx.clamp(0, layer.frame.width as i64 - 1) as u32
            };
            let si = (src_y * layer.frame.stride + src_x * 4) as usize;
            let di = (y * dest.stride + x * 4) as usize;
            dest.data[di..di + 4].copy_from_slice(&layer.frame.data[si..si + 4]);
        }
    }
    Ok(())
}

fn padded_bytes_per_row(width: u32) -> u32 {
    let unpadded = width * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    (unpadded + align - 1) / align * align
}

fn bgra_to_rgba(frame: &VideoFrame) -> Result<Vec<u8>, String> {
    let mut out = vec![0u8; (frame.width * frame.height * 4) as usize];
    for y in 0..frame.height {
        for x in 0..frame.width {
            let src = (y * frame.stride + x * 4) as usize;
            let dst = ((y * frame.width + x) * 4) as usize;
            out[dst] = frame.data[src + 2];
            out[dst + 1] = frame.data[src + 1];
            out[dst + 2] = frame.data[src];
            out[dst + 3] = frame.data[src + 3];
        }
    }
    Ok(out)
}

fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; rgba.len()];
    for (dst, src) in out.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
    out
}

fn quad_vertices(canvas_w: u32, canvas_h: u32, layer: &Layer, pad: f32) -> [Vertex; 6] {
    let x0 = (layer.x as f32 - pad) / canvas_w as f32 * 2.0 - 1.0;
    let x1 = (layer.x as f32 + layer.width as f32 + pad) / canvas_w as f32 * 2.0 - 1.0;
    let y0 = 1.0 - (layer.y as f32 - pad) / canvas_h as f32 * 2.0;
    let y1 = 1.0 - (layer.y as f32 + layer.height as f32 + pad) / canvas_h as f32 * 2.0;
    let u0 = layer.uv_x;
    let v0 = layer.uv_y;
    let u1 = layer.uv_x + layer.uv_w;
    let v1 = layer.uv_y + layer.uv_h;
    let lx0 = -pad / layer.width as f32;
    let lx1 = 1.0 + pad / layer.width as f32;
    let ly0 = -pad / layer.height as f32;
    let ly1 = 1.0 + pad / layer.height as f32;
    [
        Vertex {
            pos: [x0, y0],
            uv: [u0, v0],
            local: [lx0, ly0],
        },
        Vertex {
            pos: [x1, y0],
            uv: [u1, v0],
            local: [lx1, ly0],
        },
        Vertex {
            pos: [x0, y1],
            uv: [u0, v1],
            local: [lx0, ly1],
        },
        Vertex {
            pos: [x1, y0],
            uv: [u1, v0],
            local: [lx1, ly0],
        },
        Vertex {
            pos: [x1, y1],
            uv: [u1, v1],
            local: [lx1, ly1],
        },
        Vertex {
            pos: [x0, y1],
            uv: [u0, v1],
            local: [lx0, ly1],
        },
    ]
}

pub fn run_parity(
    dir: &std::path::Path,
    gate: &crate::media::EncoderGate,
) -> Result<crate::media::MediaParityReport, String> {
    use crate::fixtures::generate_pcm16_wav;
    use crate::media::{
        compare_frames, decode_h264_frame, decode_pcm, encode_h264_frames, region_mean_delta,
        write_solid_h264, COMPOSITOR_BACKEND, COMPOSITOR_MAX_TOLERANCE, COMPOSITOR_MEAN_TOLERANCE,
        CONCURRENT_ENCODER_LIMIT, COPIES_DECODE, COPIES_ENCODE, DECODER_BACKEND, ENCODER_BACKEND,
        PARITY_MEAN_TOLERANCE, PARITY_REGION_MEAN_TOLERANCE,
    };
    use crate::project::pcm::channel_peak_rms;
    use std::fs;

    let compositor = Compositor::new()?;
    let screen_path = dir.join("f2-screen.mp4");
    write_solid_h264(&screen_path, 64, 64, 0.92, 0.12, 0.10)?;
    let screen = decode_h264_frame(&screen_path, 0)?;
    let webcam = VideoFrame::solid(16, 16, 32, 200, 48, 0)?;
    let scene = Scene::styled_preview(screen, Some(webcam))?;
    let preview = compositor.composite(&scene)?;
    let cpu = Compositor::composite_cpu(&scene)?;
    let (compositor_max_delta, compositor_mean_delta) = compare_frames(&preview, &cpu)?;
    let _slot = gate.try_acquire()?;
    if gate.try_acquire().is_ok() {
        return Err("Encoder gate allowed a second concurrent encode".into());
    }
    let export_path = dir.join("f2-export.mp4");
    encode_h264_frames(&export_path, &[preview.clone(), preview.clone()], 30)?;
    drop(_slot);
    let exported = decode_h264_frame(&export_path, 0)?;
    let (max_abs_delta, mean_abs_delta) = compare_frames(&preview, &exported)?;
    let region_mean_delta = region_mean_delta(&preview, &exported);
    let wav_path = dir.join("f2-mic.wav");
    let pcm_bytes = generate_pcm16_wav(48_000, 1, &[16384i16; 480]);
    fs::write(&wav_path, pcm_bytes).map_err(|e| e.to_string())?;
    let audio = decode_pcm(&wav_path, 64)?;
    let (pcm_peak, pcm_rms) = channel_peak_rms(&audio.samples);
    let matched = compositor_mean_delta <= COMPOSITOR_MEAN_TOLERANCE
        && compositor_max_delta <= COMPOSITOR_MAX_TOLERANCE
        && mean_abs_delta <= PARITY_MEAN_TOLERANCE
        && region_mean_delta <= PARITY_REGION_MEAN_TOLERANCE;
    let mut diagnostics = vec![
        format!("adapter={}", compositor.adapter_name()),
        format!("ffmpeg_pinned=false"),
        format!("software_fallback=none"),
    ];
    if !matched {
        diagnostics.push(format!(
            "parity missed tolerance compositor_mean={compositor_mean_delta:.2} compositor_max={compositor_max_delta} encode_mean={mean_abs_delta:.2} region={region_mean_delta:.2}"
        ));
    }
    Ok(crate::media::MediaParityReport {
        matched,
        preview_width: preview.width,
        preview_height: preview.height,
        export_width: exported.width,
        export_height: exported.height,
        preview_pts_us: preview.pts_us,
        export_pts_us: exported.pts_us,
        max_abs_delta,
        mean_abs_delta,
        region_mean_delta,
        compositor_mean_delta,
        compositor_max_delta,
        copies_decode: COPIES_DECODE,
        copies_composite: compositor.copies(),
        copies_encode: COPIES_ENCODE,
        concurrent_encoder_limit: CONCURRENT_ENCODER_LIMIT,
        decoder_backend: DECODER_BACKEND.into(),
        compositor_backend: COMPOSITOR_BACKEND.into(),
        encoder_backend: ENCODER_BACKEND.into(),
        ffmpeg_pinned: false,
        color_space: "rec709_full".into(),
        pcm_peak,
        pcm_rms,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::VideoFrame;

    #[test]
    fn cpu_composite_crops_screen_uv_and_leaves_webcam() {
        let mut screen = VideoFrame::solid(8, 8, 0, 0, 255, 0).unwrap();
        for y in 0..8u32 {
            for x in 4..8u32 {
                let i = ((y * 8 + x) * 4) as usize;
                screen.data[i] = 255;
                screen.data[i + 1] = 0;
                screen.data[i + 2] = 0;
            }
        }
        let webcam = VideoFrame::solid(8, 8, 0, 255, 0, 0).unwrap();
        let mut scene = Scene {
            width: 8,
            height: 8,
            background: [0.0, 0.0, 0.0, 1.0],
            layers: vec![
                Layer::placed(screen, 0, 0, 8, 8),
                Layer::placed(webcam, 0, 0, 2, 2),
            ],
        };
        scene.layers[0].uv_x = 0.5;
        scene.layers[0].uv_w = 0.5;
        let out = Compositor::composite_cpu(&scene).unwrap();
        let center = ((4 * 8 + 4) * 4) as usize;
        assert!(
            out.data[center] > 200 && out.data[center + 2] < 40,
            "screen UV crop should sample the blue half"
        );
        assert!(
            out.data[1] > 200 && out.data[2] < 40,
            "webcam layer must keep identity UV"
        );
    }

    fn split_frame(width: u32, height: u32, left: [u8; 3], right: [u8; 3]) -> VideoFrame {
        let mut frame = VideoFrame::solid(width, height, left[0], left[1], left[2], 0).unwrap();
        for y in 0..height {
            for x in width / 2..width {
                let i = (y * frame.stride + x * 4) as usize;
                frame.data[i] = right[0];
                frame.data[i + 1] = right[1];
                frame.data[i + 2] = right[2];
            }
        }
        frame
    }

    #[test]
    fn layout_letterboxes_without_stretching() {
        let screen = VideoFrame::solid(16, 8, 0, 0, 255, 0).unwrap();
        let mut landscape = EditLayout::default();
        landscape.background_type = "solid".into();
        landscape.color_start = "#000000".into();
        landscape.webcam_enabled = false;
        landscape.padding_px = 0;
        let wide = Scene::from_layout(16, 8, &landscape, Some(screen.clone()), None).unwrap();
        let screen_layer = wide
            .layers
            .iter()
            .find(|l| l.role == LayerRole::Screen)
            .unwrap();
        assert_eq!((screen_layer.width, screen_layer.height), (16, 8));

        let mut portrait = landscape.clone();
        portrait.aspect_ratio = "9:16".into();
        let tall = Scene::from_layout(8, 16, &portrait, Some(screen), None).unwrap();
        let placed = tall
            .layers
            .iter()
            .find(|l| l.role == LayerRole::Screen)
            .unwrap();
        let placed_aspect = placed.width as f32 / placed.height as f32;
        assert!(
            (placed_aspect - 2.0).abs() < 0.05,
            "9:16 canvas must letterbox a 16:9 source, got {placed_aspect}"
        );
        assert_eq!(placed.width, 8);
        assert_eq!(placed.height, 4);
    }

    #[test]
    fn webcam_mirror_flips_only_webcam_pixels() {
        let screen = split_frame(16, 16, [0, 0, 255], [255, 0, 0]);
        let webcam = split_frame(16, 8, [0, 255, 0], [255, 255, 255]);
        let mut layout = EditLayout::default();
        layout.background_type = "solid".into();
        layout.color_start = "#000000".into();
        layout.padding_px = 0;
        layout.webcam_enabled = true;
        layout.webcam_mirror = true;
        layout.webcam_position = "top-left".into();
        layout.webcam_size = "xl".into();
        layout.webcam_border_width = 0;
        let scene = Scene::from_layout(16, 16, &layout, Some(screen), Some(webcam)).unwrap();
        let cam = scene
            .layers
            .iter()
            .find(|l| l.role == LayerRole::Webcam)
            .unwrap();
        assert_eq!(cam.uv_x, 1.0);
        assert_eq!(cam.uv_w, -1.0);
        let screen_layer = scene
            .layers
            .iter()
            .find(|l| l.role == LayerRole::Screen)
            .unwrap();
        assert_eq!(screen_layer.uv_w, 1.0);
        let out = Compositor::composite_cpu(&scene).unwrap();
        let screen_left = out.data[0];
        assert!(
            out.data[2] > 200 && screen_left < 40,
            "screen left must stay red (unmirrored)"
        );
        let cx = cam.x;
        let cy = cam.y;
        let left = (cy * out.stride + cx * 4) as usize;
        let right = (cy * out.stride + (cx + cam.width - 1) * 4) as usize;
        assert!(
            out.data[left] > 200 && out.data[left + 1] > 200 && out.data[left + 2] > 200,
            "mirrored webcam should put the white half on the left"
        );
        assert!(
            out.data[right + 1] > 200 && out.data[right + 2] < 40,
            "mirrored webcam should put the green half on the right"
        );
    }

    #[test]
    fn styled_solid_background_differs_from_identity() {
        let screen = VideoFrame::solid(8, 8, 0, 0, 255, 0).unwrap();
        let mut identity = EditLayout::default();
        identity.background_type = "solid".into();
        identity.color_start = "#000000".into();
        identity.padding_px = 2;
        identity.webcam_enabled = false;
        let mut styled = identity.clone();
        styled.color_start = "#ff0000".into();
        let a = Compositor::composite_cpu(
            &Scene::from_layout(16, 16, &identity, Some(screen.clone()), None).unwrap(),
        )
        .unwrap();
        let b = Compositor::composite_cpu(
            &Scene::from_layout(16, 16, &styled, Some(screen), None).unwrap(),
        )
        .unwrap();
        assert_ne!(a.data[0..4], b.data[0..4], "padding should show background");
        assert!(b.data[2] > 200 && a.data[2] < 40);
    }

    fn pixel(frame: &VideoFrame, x: u32, y: u32) -> [u8; 4] {
        let i = (y * frame.stride + x * 4) as usize;
        [
            frame.data[i],
            frame.data[i + 1],
            frame.data[i + 2],
            frame.data[i + 3],
        ]
    }

    fn full_frame_delta(a: &VideoFrame, b: &VideoFrame) -> (u8, f32) {
        let mut max = 0u8;
        let mut sum = 0u64;
        let mut count = 0u64;
        for y in 0..a.height {
            for x in 0..a.width {
                let i = (y * a.stride + x * 4) as usize;
                for c in 0..3 {
                    let d = a.data[i + c].abs_diff(b.data[i + c]);
                    max = max.max(d);
                    sum += u64::from(d);
                    count += 1;
                }
            }
        }
        (max, sum as f32 / count as f32)
    }

    fn solid_layout() -> EditLayout {
        let mut layout = EditLayout::default();
        layout.background_type = "solid".into();
        layout.color_start = "#00ff00".into();
        layout.padding_px = 4;
        layout.webcam_enabled = false;
        layout.corner_radius_px = 0;
        layout.shadow_blur_px = 0;
        layout
    }

    #[test]
    fn rounded_rect_reveals_background_in_screen_corners() {
        let screen = VideoFrame::solid(24, 24, 0, 0, 255, 0).unwrap();
        let mut layout = solid_layout();
        layout.corner_radius_px = 12;
        let scene = Scene::from_layout(32, 32, &layout, Some(screen), None).unwrap();
        let out = Compositor::composite_cpu(&scene).unwrap();
        let corner = pixel(&out, 4, 4);
        assert!(
            corner[1] > 200 && corner[2] < 40,
            "rounded clip must show green background in the screen AABB corner, got {corner:?}"
        );
        let center = pixel(&out, 16, 16);
        assert!(center[2] > 200 && center[1] < 40, "screen interior stays red");
    }

    #[test]
    fn circle_and_squircle_clip_webcam_only() {
        let screen = VideoFrame::solid(32, 32, 0, 0, 255, 0).unwrap();
        let webcam = VideoFrame::solid(16, 16, 0, 255, 0, 0).unwrap();
        let mut layout = solid_layout();
        layout.padding_px = 0;
        layout.webcam_enabled = true;
        layout.webcam_shape = "circle".into();
        layout.webcam_position = "top-left".into();
        layout.webcam_size = "xl".into();
        layout.webcam_border_width = 0;
        layout.webcam_mirror = false;
        let scene = Scene::from_layout(32, 32, &layout, Some(screen.clone()), Some(webcam.clone()))
            .unwrap();
        let cam = scene
            .layers
            .iter()
            .find(|l| l.role == LayerRole::Webcam)
            .unwrap();
        assert_eq!(cam.clip, ClipMode::Circle);
        assert_eq!(cam.width, cam.height);
        let out = Compositor::composite_cpu(&scene).unwrap();
        let cam_corner = pixel(&out, cam.x, cam.y);
        assert!(
            cam_corner[2] > 200 && cam_corner[1] < 40,
            "circle clip must not cover the webcam AABB corner, got {cam_corner:?}"
        );
        let cx = cam.x + cam.width / 2;
        let cy = cam.y + cam.height / 2;
        let cam_center = pixel(&out, cx, cy);
        assert!(cam_center[1] > 200 && cam_center[2] < 40, "circle interior is webcam");
        let screen_far = pixel(&out, 30, 30);
        assert!(screen_far[2] > 200, "screen layer is not circle-clipped");

        layout.webcam_shape = "squircle".into();
        let squircle =
            Scene::from_layout(32, 32, &layout, Some(screen), Some(webcam)).unwrap();
        let cam = squircle
            .layers
            .iter()
            .find(|l| l.role == LayerRole::Webcam)
            .unwrap();
        assert_eq!(cam.clip, ClipMode::Squircle);
        let out = Compositor::composite_cpu(&squircle).unwrap();
        let cam_corner = pixel(&out, cam.x, cam.y);
        assert!(
            cam_corner[2] > 200 && cam_corner[1] < 40,
            "squircle clip must drop the webcam AABB corner"
        );
    }

    #[test]
    fn circle_webcam_cover_crops_nonsquare_source() {
        let screen = VideoFrame::solid(32, 32, 0, 0, 255, 0).unwrap();
        let webcam = VideoFrame::solid(32, 16, 0, 255, 0, 0).unwrap();
        let mut layout = solid_layout();
        layout.padding_px = 0;
        layout.webcam_enabled = true;
        layout.webcam_shape = "circle".into();
        layout.webcam_position = "top-left".into();
        layout.webcam_size = "xl".into();
        layout.webcam_border_width = 0;
        layout.webcam_mirror = false;
        let scene = Scene::from_layout(32, 32, &layout, Some(screen), Some(webcam)).unwrap();
        let cam = scene
            .layers
            .iter()
            .find(|l| l.role == LayerRole::Webcam)
            .unwrap();
        assert_eq!(cam.width, cam.height);
        assert!(
            (cam.uv_w - 0.5).abs() < 0.05 && (cam.uv_x - 0.25).abs() < 0.05 && cam.uv_h == 1.0,
            "16:9 source in a square bubble must cover-crop, got uv=({}, {}, {}, {})",
            cam.uv_x,
            cam.uv_y,
            cam.uv_w,
            cam.uv_h
        );
    }

    #[test]
    fn shadow_does_not_punch_a_hole_in_background() {
        let screen = VideoFrame::solid(16, 16, 0, 0, 255, 0).unwrap();
        let mut layout = solid_layout();
        layout.padding_px = 8;
        layout.corner_radius_px = 6;
        layout.shadow_blur_px = 8;
        layout.shadow_opacity = 0.8;
        let scene = Scene::from_layout(32, 32, &layout, Some(screen), None).unwrap();
        let screen_layer = scene
            .layers
            .iter()
            .find(|l| l.role == LayerRole::Screen)
            .unwrap();
        let out = Compositor::composite_cpu(&scene).unwrap();
        let interior = pixel(
            &out,
            screen_layer.x + screen_layer.width / 2,
            screen_layer.y + screen_layer.height / 2,
        );
        assert!(
            interior[2] > 200 && interior[1] < 40,
            "opaque screen must cover its own shadow"
        );
        let below = pixel(
            &out,
            screen_layer.x + screen_layer.width / 2,
            (screen_layer.y + screen_layer.height).min(31),
        );
        assert!(
            below[1] > 20 && below[1] < 240 && below[2] < 80,
            "drop shadow should darken green padding, not replace it with a hole, got {below:?}"
        );
        let far = pixel(&out, 1, 1);
        assert!(
            far[1] > 200 && far[2] < 40,
            "background far from the screen must stay green"
        );
        let clip_corner = pixel(&out, screen_layer.x, screen_layer.y);
        assert!(
            clip_corner[1] > 0,
            "rounded corner must keep background (possibly shadowed), not a punched hole"
        );
    }

    #[test]
    fn wallpaper_decodes_project_asset_and_rejects_url() {
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().join("assets");
        std::fs::create_dir_all(&assets).unwrap();
        let relative = "assets/wallpaper-test.png";
        let mut img = image::RgbaImage::new(8, 8);
        for p in img.pixels_mut() {
            *p = image::Rgba([255, 0, 255, 255]);
        }
        img.save(dir.path().join(relative)).unwrap();
        let mut layout = solid_layout();
        layout.background_type = "wallpaper".into();
        layout.wallpaper_asset = Some(relative.into());
        layout.color_start = "#00ff00".into();
        layout.padding_px = 4;
        let paper = load_wallpaper_frame(dir.path(), &layout, 16, 16)
            .unwrap()
            .expect("decoded wallpaper");
        let screen = VideoFrame::solid(8, 8, 0, 0, 255, 0).unwrap();
        let scene = Scene::from_layout_with_wallpaper(
            16,
            16,
            &layout,
            Some(screen),
            None,
            Some(paper),
        )
        .unwrap();
        let out = Compositor::composite_cpu(&scene).unwrap();
        let pad = pixel(&out, 0, 0);
        assert!(
            pad[0] > 200 && pad[2] > 200 && pad[1] < 40,
            "padding must blit the magenta wallpaper, not the green clear color, got {pad:?}"
        );

        let mut url = layout.clone();
        url.wallpaper_asset = Some("https://example.com/bg.png".into());
        assert!(load_wallpaper_frame(dir.path(), &url, 16, 16)
            .unwrap_err()
            .contains("URL"));
    }

    #[test]
    fn gpu_cpu_clip_shadow_wallpaper_within_tolerance() {
        let compositor = match Compositor::new() {
            Ok(c) => c,
            Err(err) => panic!("A1 GPU compositor required for contract tests: {err}"),
        };
        let screen = VideoFrame::solid(24, 24, 0, 0, 255, 0).unwrap();
        let webcam = VideoFrame::solid(16, 16, 32, 200, 48, 0).unwrap();
        let mut layout = solid_layout();
        layout.corner_radius_px = 8;
        layout.shadow_blur_px = 6;
        layout.shadow_opacity = 0.6;
        layout.webcam_enabled = true;
        layout.webcam_shape = "circle".into();
        layout.webcam_position = "bottom-right".into();
        layout.webcam_size = "lg".into();
        layout.webcam_border_width = 0;
        layout.webcam_shadow = true;
        layout.webcam_mirror = false;
        let paper = VideoFrame::solid(16, 8, 180, 40, 40, 0).unwrap();
        layout.background_type = "wallpaper".into();
        layout.wallpaper_asset = Some("assets/unused.png".into());
        let scene = Scene::from_layout_with_wallpaper(
            32,
            32,
            &layout,
            Some(screen),
            Some(webcam),
            Some(paper),
        )
        .unwrap();
        let cpu = Compositor::composite_cpu(&scene).unwrap();
        let gpu = compositor.composite(&scene).unwrap();
        let (max, mean) = crate::media::compare_frames(&gpu, &cpu).unwrap();
        assert!(
            mean <= crate::media::COMPOSITOR_MEAN_TOLERANCE,
            "GPU vs CPU mean {mean} max {max}"
        );
        assert!(
            max <= crate::media::COMPOSITOR_MAX_TOLERANCE,
            "GPU vs CPU max {max} mean {mean}"
        );
        let (full_max, full_mean) = full_frame_delta(&gpu, &cpu);
        assert!(
            full_mean <= crate::media::COMPOSITOR_MEAN_TOLERANCE + 2.0,
            "full-frame GPU vs CPU mean {full_mean} max {full_max}"
        );
    }
}
