//! Direct3D 11 device shared by capture and the hardware encoder, and the
//! video-processor stage that scales, letterboxes, and converts BGRA capture
//! frames to NV12 encoder input without leaving the GPU.
use crate::capture::fit_letterbox;
use ::windows::core::{Interface, Result};
use ::windows::Win32::Foundation::{HMODULE, RECT};
use ::windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use ::windows::Win32::Graphics::Direct3D11::*;
use ::windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use ::windows::Win32::Media::MediaFoundation::{IMFDXGIDeviceManager, MFCreateDXGIDeviceManager};

pub struct Gpu {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub manager: IMFDXGIDeviceManager,
}

// SAFETY: the device is free-threaded, and the immediate context is made
// thread-safe with `SetMultithreadProtected(true)` at creation; the DXGI device
// manager serializes access itself.
unsafe impl Send for Gpu {}
unsafe impl Sync for Gpu {}

/// One device for the whole process: capture, encoders, and the live preview
/// share textures without copies through the CPU.
pub fn shared() -> std::result::Result<std::sync::Arc<Gpu>, String> {
    static SHARED: std::sync::OnceLock<std::result::Result<std::sync::Arc<Gpu>, String>> =
        std::sync::OnceLock::new();
    SHARED
        .get_or_init(|| {
            Gpu::create()
                .map(std::sync::Arc::new)
                .map_err(|e| format!("Direct3D 11 is unavailable: {e}"))
        })
        .clone()
}

impl Gpu {
    fn create() -> Result<Self> {
        unsafe {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.ok_or_else(::windows::core::Error::empty)?;
            let context = context.ok_or_else(::windows::core::Error::empty)?;
            // The encoder uses the device from its own threads.
            let _ = device
                .cast::<ID3D11Multithread>()?
                .SetMultithreadProtected(true);
            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.ok_or_else(::windows::core::Error::empty)?;
            manager.ResetDevice(&device, token)?;
            Ok(Self {
                device,
                context,
                manager,
            })
        }
    }

    pub fn texture(&self, width: u32, height: u32, format: DXGI_FORMAT) -> Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))?
        };
        texture.ok_or_else(::windows::core::Error::empty)
    }
}

/// BGRA source texture → ring of NV12 output textures at the recording size.
/// Rebuilt whenever the source size changes (a resized window).
pub struct Converter {
    video_context: ID3D11VideoContext,
    processor: ID3D11VideoProcessor,
    input_view: ID3D11VideoProcessorInputView,
    pub source: ID3D11Texture2D,
    source_size: (u32, u32),
    outputs: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    next_output: usize,
    output_size: (u32, u32),
    content_size: (u32, u32),
}

/// Output textures in flight: the encoder may still read earlier frames.
const OUTPUT_RING: usize = 8;

impl Converter {
    pub fn new(
        gpu: &Gpu,
        source_size: (u32, u32),
        output_size: (u32, u32),
        fps: u32,
    ) -> Result<Self> {
        unsafe {
            let video_device: ID3D11VideoDevice = gpu.device.cast()?;
            let video_context: ID3D11VideoContext = gpu.context.cast()?;
            let rate = DXGI_RATIONAL {
                Numerator: fps,
                Denominator: 1,
            };
            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: source_size.0,
                InputHeight: source_size.1,
                OutputFrameRate: rate,
                OutputWidth: output_size.0,
                OutputHeight: output_size.1,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };
            let enumerator = video_device.CreateVideoProcessorEnumerator(&content)?;
            let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;

            let source = gpu.texture(source_size.0, source_size.1, DXGI_FORMAT_B8G8R8A8_UNORM)?;
            let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV {
                        MipSlice: 0,
                        ArraySlice: 0,
                    },
                },
            };
            let mut input_view = None;
            video_device.CreateVideoProcessorInputView(
                &source,
                &enumerator,
                &input_desc,
                Some(&mut input_view),
            )?;
            let input_view = input_view.ok_or_else(::windows::core::Error::empty)?;

            let output_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };
            let mut outputs = Vec::with_capacity(OUTPUT_RING);
            for _ in 0..OUTPUT_RING {
                let texture = gpu.texture(output_size.0, output_size.1, DXGI_FORMAT_NV12)?;
                let mut view = None;
                video_device.CreateVideoProcessorOutputView(
                    &texture,
                    &enumerator,
                    &output_desc,
                    Some(&mut view),
                )?;
                outputs.push((texture, view.ok_or_else(::windows::core::Error::empty)?));
            }

            // Full-range RGB in, BT.709 limited-range YUV out, black bars.
            let input_space = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 };
            // YCbCr_Matrix = 1 (BT.709), Nominal_Range = 1 (16-235).
            let output_space = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                _bitfield: (1 << 2) | (1 << 4),
            };
            video_context.VideoProcessorSetStreamColorSpace(&processor, 0, &input_space);
            video_context.VideoProcessorSetOutputColorSpace(&processor, &output_space);
            let black = D3D11_VIDEO_COLOR {
                Anonymous: D3D11_VIDEO_COLOR_0 {
                    RGBA: D3D11_VIDEO_COLOR_RGBA {
                        R: 0.0,
                        G: 0.0,
                        B: 0.0,
                        A: 1.0,
                    },
                },
            };
            video_context.VideoProcessorSetOutputBackgroundColor(&processor, false, &black);
            video_context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);

            Ok(Self {
                video_context,
                processor,
                input_view,
                source,
                source_size,
                outputs,
                next_output: 0,
                output_size,
                content_size: source_size,
            })
        }
    }

    pub fn source_size(&self) -> (u32, u32) {
        self.source_size
    }

    pub fn content_size(&self) -> (u32, u32) {
        self.content_size
    }

    /// The part of `source` holding the picture; a window's frame can be
    /// smaller than the capture buffer until the pool is recreated.
    pub fn set_content_size(&mut self, width: u32, height: u32) {
        self.content_size = (
            width.clamp(1, self.source_size.0),
            height.clamp(1, self.source_size.1),
        );
    }

    /// Render the current source into the next output texture.
    pub fn convert(&mut self) -> Result<ID3D11Texture2D> {
        let (texture, view) = &self.outputs[self.next_output];
        self.next_output = (self.next_output + 1) % self.outputs.len();
        let (content_w, content_h) = self.content_size;
        let fit = fit_letterbox(content_w, content_h, self.output_size.0, self.output_size.1);
        let source_rect = RECT {
            left: 0,
            top: 0,
            right: content_w as i32,
            bottom: content_h as i32,
        };
        let dest_rect = RECT {
            left: fit.x,
            top: fit.y,
            right: fit.x + fit.width as i32,
            bottom: fit.y + fit.height as i32,
        };
        unsafe {
            self.video_context.VideoProcessorSetStreamSourceRect(
                &self.processor,
                0,
                true,
                Some(&source_rect),
            );
            self.video_context.VideoProcessorSetStreamDestRect(
                &self.processor,
                0,
                true,
                Some(&dest_rect),
            );
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                OutputIndex: 0,
                InputFrameOrField: 0,
                PastFrames: 0,
                FutureFrames: 0,
                ppPastSurfaces: std::ptr::null_mut(),
                pInputSurface: std::mem::ManuallyDrop::new(Some(self.input_view.clone())),
                ppFutureSurfaces: std::ptr::null_mut(),
                ppPastSurfacesRight: std::ptr::null_mut(),
                pInputSurfaceRight: std::mem::ManuallyDrop::new(None),
                ppFutureSurfacesRight: std::ptr::null_mut(),
            };
            let streams = [stream];
            let result = self
                .video_context
                .VideoProcessorBlt(&self.processor, view, 0, &streams);
            // Release the input view reference the stream descriptor holds.
            let [mut stream] = streams;
            std::mem::ManuallyDrop::drop(&mut stream.pInputSurface);
            result?;
        }
        Ok(texture.clone())
    }
}
