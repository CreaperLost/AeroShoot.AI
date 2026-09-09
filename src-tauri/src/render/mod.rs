//! Offscreen WGPU compositor. One device, CPU upload + readback (bounded-copy fallback).
use crate::media::{validate_dim, ColorInfo, PixelFormat, VideoFrame, MAX_FRAME_DIM};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

pub const COPIES_COMPOSITE: u32 = 2;
pub const MAX_LAYERS: usize = 4;
const SHADER: &str = include_str!("composite.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub frame: VideoFrame,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
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
        let mut layers = vec![Layer {
            frame: screen,
            x: 12,
            y: 16,
            width: 40,
            height: 32,
        }];
        if let Some(webcam) = webcam {
            layers.push(Layer {
                frame: webcam,
                x: 46,
                y: 4,
                width: 12,
                height: 12,
            });
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
        padding_px: u32,
        screen: Option<VideoFrame>,
        webcam: Option<VideoFrame>,
    ) -> Result<Self, String> {
        validate_dim(width, height)?;
        if padding_px.saturating_mul(2) >= width || padding_px.saturating_mul(2) >= height {
            return Err("Export padding leaves no content rectangle".into());
        }
        let content_w = width - padding_px * 2;
        let content_h = height - padding_px * 2;
        let mut layers = Vec::new();
        if let Some(screen) = screen {
            layers.push(Layer {
                frame: screen,
                x: padding_px,
                y: padding_px,
                width: content_w,
                height: content_h,
            });
        }
        if let Some(webcam) = webcam {
            let bubble = width.min(height) / 5;
            let bubble = bubble.max(8);
            layers.push(Layer {
                frame: webcam,
                x: width.saturating_sub(bubble + 4),
                y: 4,
                width: bubble,
                height: bubble,
            });
        }
        Ok(Self {
            width,
            height,
            background: [0.05, 0.12, 0.28, 1.0],
            layers,
        })
    }
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
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
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
        let mut uploads = Vec::new();
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
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                ],
            });
            let verts = quad_vertices(scene.width, scene.height, layer);
            let vbuf = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("aeroshoot-quad"),
                    contents: bytemuck::cast_slice(&verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            uploads.push((texture, bind, vbuf, layer_view));
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
            for (_tex, bind, vbuf, _view) in &uploads {
                pass.set_bind_group(0, bind, &[]);
                pass.set_vertex_buffer(0, vbuf.slice(..));
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
        let _keep = uploads;
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

fn blit_nearest(dest: &mut VideoFrame, layer: &Layer) -> Result<(), String> {
    if layer.width == 0 || layer.height == 0 {
        return Err("Layer size is empty".into());
    }
    for dy in 0..layer.height {
        let y = layer.y + dy;
        if y >= dest.height {
            continue;
        }
        let src_y =
            ((2 * dy + 1) * layer.frame.height / (2 * layer.height)).min(layer.frame.height - 1);
        for dx in 0..layer.width {
            let x = layer.x + dx;
            if x >= dest.width {
                continue;
            }
            let src_x =
                ((2 * dx + 1) * layer.frame.width / (2 * layer.width)).min(layer.frame.width - 1);
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

fn quad_vertices(canvas_w: u32, canvas_h: u32, layer: &Layer) -> [Vertex; 6] {
    let x0 = layer.x as f32 / canvas_w as f32 * 2.0 - 1.0;
    let x1 = (layer.x + layer.width) as f32 / canvas_w as f32 * 2.0 - 1.0;
    let y0 = 1.0 - layer.y as f32 / canvas_h as f32 * 2.0;
    let y1 = 1.0 - (layer.y + layer.height) as f32 / canvas_h as f32 * 2.0;
    [
        Vertex {
            pos: [x0, y0],
            uv: [0.0, 0.0],
        },
        Vertex {
            pos: [x1, y0],
            uv: [1.0, 0.0],
        },
        Vertex {
            pos: [x0, y1],
            uv: [0.0, 1.0],
        },
        Vertex {
            pos: [x1, y0],
            uv: [1.0, 0.0],
        },
        Vertex {
            pos: [x1, y1],
            uv: [1.0, 1.0],
        },
        Vertex {
            pos: [x0, y1],
            uv: [0.0, 1.0],
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
