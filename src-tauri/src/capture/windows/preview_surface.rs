//! The record-scene preview on Windows: a child window layered over the web
//! view, mirroring the host element's rectangle like the macOS AppKit view.
//! A render thread draws the preview mailbox into a Direct3D swap chain on the
//! shared device; pixels never go through the web view.
//!
//! The canvas matches macOS: 1280×720, the screen letterboxed, the webcam in a
//! 280×158 box in the bottom-right corner, on a near-black background.
use super::record::frames::{self, CameraFrame, ScreenFrame};
use super::record::gpu::{self, Gpu};
use crate::capture::fit_letterbox;
use crate::hud::{PreviewHitMode, PreviewViewport};
use ::windows::core::{w, Interface, Result as WinResult};
use ::windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use ::windows::Win32::Graphics::Direct3D11::*;
use ::windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL,
    DXGI_SAMPLE_DESC,
};
use ::windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter, IDXGIDevice, IDXGIFactory2, IDXGISwapChain1, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use ::windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateRectRgn, EndPaint, ScreenToClient, SetWindowRgn, PAINTSTRUCT,
};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::UI::WindowsAndMessaging::*;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

const CANVAS: (u32, u32) = (1280, 720);
const CAMERA_BOX: RECT = RECT {
    left: 980,
    top: 720 - 20 - 158,
    right: 980 + 280,
    bottom: 720 - 20,
};
const BACKGROUND: [f32; 4] = [0.02, 0.02, 0.03, 1.0];
/// 30 fps normally; 15 fps while recording, when the preview only monitors.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const RECORDING_FRAME_INTERVAL: Duration = Duration::from_millis(66);

struct SurfaceState {
    stop: AtomicBool,
    visible: AtomicBool,
    /// Physical size, packed as `width << 32 | height`.
    size: AtomicU64,
    hit_mode: AtomicU32,
    fixed: Mutex<Option<[f32; 3]>>,
    /// Bumped by anything that needs a redraw besides new frames.
    revision: AtomicU64,
    presented: AtomicU64,
}

struct Surface {
    hwnd: HWND,
    state: Arc<SurfaceState>,
    renderer: Option<JoinHandle<()>>,
}

fn hit_mode_value(mode: PreviewHitMode) -> u32 {
    match mode {
        PreviewHitMode::Consume => 0,
        PreviewHitMode::Circle => 1,
        PreviewHitMode::PassThrough => 2,
        PreviewHitMode::CirclePassThrough => 3,
        PreviewHitMode::SquirclePassThrough => 4,
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => {
            let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const SurfaceState;
            let mode = if state.is_null() {
                0
            } else {
                unsafe { (*state).hit_mode.load(Ordering::Relaxed) }
            };
            let consumes = match mode {
                0 => true,
                1 => {
                    // Screen coordinates in LPARAM: signed low and high words.
                    let mut point = POINT {
                        x: (lparam.0 & 0xffff) as i16 as i32,
                        y: ((lparam.0 >> 16) & 0xffff) as i16 as i32,
                    };
                    let _ = unsafe { ScreenToClient(hwnd, &mut point) };
                    let mut client = RECT::default();
                    let _ = unsafe { GetClientRect(hwnd, &mut client) };
                    inside_ellipse(point, client)
                }
                _ => false,
            };
            // Transparent hits fall through to the web view underneath.
            LRESULT(if consumes {
                HTCLIENT as isize
            } else {
                HTTRANSPARENT as isize
            })
        }
        // Never take focus from the web view.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            unsafe {
                BeginPaint(hwnd, &mut paint);
                let _ = EndPaint(hwnd, &paint);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn inside_ellipse(point: POINT, bounds: RECT) -> bool {
    let (width, height) = (
        f64::from(bounds.right - bounds.left),
        f64::from(bounds.bottom - bounds.top),
    );
    if width <= 0.0 || height <= 0.0 {
        return false;
    }
    let dx = (f64::from(point.x) - width / 2.0) / (width / 2.0);
    let dy = (f64::from(point.y) - height / 2.0) / (height / 2.0);
    dx * dx + dy * dy <= 1.0
}

fn window_class() -> Result<::windows::core::PCWSTR, String> {
    static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
    let class = w!("AeroShootPreviewSurface");
    REGISTERED
        .get_or_init(|| unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None).map_err(|e| e.to_string())?.into();
            let description = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassExW(&description) == 0 {
                return Err("Could not register the preview window class".into());
            }
            Ok(())
        })
        .clone()?;
    Ok(class)
}

/// Create the preview surface as a hidden child of `parent`. Call on the
/// parent window's (UI) thread.
pub fn attach(parent: *mut c_void, _generation: u64) -> Result<NonNull<c_void>, String> {
    let gpu = gpu::shared()?;
    let class = window_class()?;
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("AeroShoot Preview"),
            WS_CHILD | WS_CLIPSIBLINGS,
            0,
            0,
            1,
            1,
            Some(HWND(parent)),
            None,
            GetModuleHandleW(None).ok().map(|m| m.into()),
            None,
        )
    }
    .map_err(|e| format!("Could not create the preview window: {e}"))?;
    let state = Arc::new(SurfaceState {
        stop: AtomicBool::new(false),
        visible: AtomicBool::new(false),
        size: AtomicU64::new(0),
        hit_mode: AtomicU32::new(0),
        fixed: Mutex::new(None),
        revision: AtomicU64::new(0),
        presented: AtomicU64::new(0),
    });
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Arc::as_ptr(&state) as isize) };
    let render_state = state.clone();
    let render_hwnd = hwnd.0 as usize;
    let renderer = std::thread::Builder::new()
        .name("aeroshoot-preview-render".into())
        .spawn(move || render_loop(gpu, HWND(render_hwnd as *mut _), render_state))
        .map_err(|e| e.to_string())?;
    let surface = Box::new(Surface {
        hwnd,
        state,
        renderer: Some(renderer),
    });
    Ok(NonNull::new(Box::into_raw(surface) as *mut c_void).expect("box pointer"))
}

fn surface<'a>(handle: NonNull<c_void>) -> &'a Surface {
    unsafe { &*(handle.as_ptr() as *const Surface) }
}

pub fn detach(handle: NonNull<c_void>) {
    let mut surface = unsafe { Box::from_raw(handle.as_ptr() as *mut Surface) };
    surface.state.stop.store(true, Ordering::SeqCst);
    if let Some(renderer) = surface.renderer.take() {
        let _ = renderer.join();
    }
    unsafe {
        SetWindowLongPtrW(surface.hwnd, GWLP_USERDATA, 0);
        let _ = DestroyWindow(surface.hwnd);
    }
}

/// Mirror the host element: position, size, clip, and visibility.
pub fn set_geometry(handle: NonNull<c_void>, viewport: &PreviewViewport) -> Result<(), String> {
    let surface = surface(handle);
    let physical = viewport.physical()?;
    let visible =
        viewport.visible && !viewport.occluded && physical.width > 0 && physical.height > 0;
    let flags = SWP_NOACTIVATE
        | if visible {
            SWP_SHOWWINDOW
        } else {
            SWP_HIDEWINDOW
        };
    unsafe {
        // Always above the web view, which is a sibling child window.
        SetWindowPos(
            surface.hwnd,
            Some(HWND_TOP),
            physical.x,
            physical.y,
            physical.width.max(1) as i32,
            physical.height.max(1) as i32,
            flags,
        )
        .map_err(|e| format!("Could not place the preview: {e}"))?;
        let scale = viewport.backing_scale;
        let region = match viewport.clip {
            Some([x, y, w, h])
                if w < viewport.width || h < viewport.height || x > 0.0 || y > 0.0 =>
            {
                let rect = [x * scale, y * scale, (x + w) * scale, (y + h) * scale]
                    .map(|v| v.round() as i32);
                Some(CreateRectRgn(rect[0], rect[1], rect[2], rect[3]))
            }
            _ => None,
        };
        // The window owns the region after this call.
        SetWindowRgn(surface.hwnd, region, true);
    }
    surface.state.size.store(
        (u64::from(physical.width) << 32) | u64::from(physical.height),
        Ordering::SeqCst,
    );
    surface.state.visible.store(visible, Ordering::SeqCst);
    surface.state.revision.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

pub fn set_hit_mode(handle: NonNull<c_void>, mode: PreviewHitMode) {
    surface(handle)
        .state
        .hit_mode
        .store(hit_mode_value(mode), Ordering::SeqCst);
}

/// Fill the surface with one color (contract tests and diagnostics).
pub fn present_fixed(handle: NonNull<c_void>, r: f32, g: f32, b: f32) {
    let state = &surface(handle).state;
    *state.fixed.lock().unwrap_or_else(PoisonError::into_inner) = Some([r, g, b]);
    state.revision.fetch_add(1, Ordering::SeqCst);
}

pub fn stats_json(handle: NonNull<c_void>) -> String {
    let state = &surface(handle).state;
    let size = state.size.load(Ordering::SeqCst);
    serde_json::json!({
        "attached": true,
        "visible": state.visible.load(Ordering::SeqCst),
        "width": size >> 32,
        "height": size & 0xffff_ffff,
        "hitMode": state.hit_mode.load(Ordering::SeqCst),
        "presented": state.presented.load(Ordering::SeqCst),
    })
    .to_string()
}

fn render_loop(gpu: Arc<Gpu>, hwnd: HWND, state: Arc<SurfaceState>) {
    let mut renderer: Option<Renderer> = None;
    let mut last = (u64::MAX, u64::MAX, 0u64);
    while !state.stop.load(Ordering::SeqCst) {
        let interval = if frames::is_recording() {
            RECORDING_FRAME_INTERVAL
        } else {
            FRAME_INTERVAL
        };
        std::thread::sleep(interval);
        if !state.visible.load(Ordering::SeqCst) {
            continue;
        }
        let size = state.size.load(Ordering::SeqCst);
        let (width, height) = ((size >> 32) as u32, (size & 0xffff_ffff) as u32);
        if width == 0 || height == 0 {
            continue;
        }
        let (sequence, screen, camera) = frames::snapshot();
        let revision = state.revision.load(Ordering::SeqCst);
        if (sequence, revision, size) == last {
            continue;
        }
        let fixed = *state.fixed.lock().unwrap_or_else(PoisonError::into_inner);
        let result = (|| -> WinResult<()> {
            if renderer.as_ref().is_none_or(|r| r.size != (width, height)) {
                match renderer.as_mut() {
                    Some(existing) => existing.resize(&gpu, (width, height))?,
                    None => renderer = Some(Renderer::new(&gpu, hwnd, (width, height))?),
                }
            }
            let renderer = renderer.as_mut().expect("renderer created above");
            match fixed {
                Some([r, g, b]) => renderer.clear(&gpu, [r, g, b, 1.0]),
                None => renderer.draw(&gpu, screen.as_ref(), camera.as_deref())?,
            }
            renderer.present()
        })();
        if result.is_ok() {
            last = (sequence, revision, size);
            state.presented.fetch_add(1, Ordering::Relaxed);
        } else {
            // Rebuild everything on the next frame (device or swap chain lost).
            renderer = None;
        }
    }
}

struct Renderer {
    swap_chain: IDXGISwapChain1,
    size: (u32, u32),
    target: Option<(ID3D11RenderTargetView, ID3D11VideoProcessorOutputView)>,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    processor: Option<(ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor)>,
    camera_staging: Option<(ID3D11Texture2D, ID3D11Texture2D, (u32, u32))>,
}

impl Renderer {
    fn new(gpu: &Gpu, hwnd: HWND, size: (u32, u32)) -> WinResult<Self> {
        unsafe {
            let dxgi: IDXGIDevice = gpu.device.cast()?;
            let adapter: IDXGIAdapter = dxgi.GetAdapter()?;
            let factory: IDXGIFactory2 = adapter.GetParent()?;
            let description = DXGI_SWAP_CHAIN_DESC1 {
                Width: size.0,
                Height: size.1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let swap_chain =
                factory.CreateSwapChainForHwnd(&gpu.device, hwnd, &description, None, None)?;
            let mut renderer = Self {
                swap_chain,
                size,
                target: None,
                video_device: gpu.device.cast()?,
                video_context: gpu.context.cast()?,
                processor: None,
                camera_staging: None,
            };
            renderer.make_target(gpu)?;
            Ok(renderer)
        }
    }

    fn resize(&mut self, gpu: &Gpu, size: (u32, u32)) -> WinResult<()> {
        self.target = None;
        self.processor = None;
        unsafe {
            self.swap_chain.ResizeBuffers(
                0,
                size.0,
                size.1,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }
        self.size = size;
        self.make_target(gpu)
    }

    fn make_target(&mut self, gpu: &Gpu) -> WinResult<()> {
        unsafe {
            let back: ID3D11Texture2D = self.swap_chain.GetBuffer(0)?;
            let mut rtv = None;
            gpu.device
                .CreateRenderTargetView(&back, None, Some(&mut rtv))?;
            let (enumerator, processor) = self.processor(gpu)?;
            let _ = processor;
            let description = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };
            let mut output = None;
            self.video_device.CreateVideoProcessorOutputView(
                &back,
                &enumerator,
                &description,
                Some(&mut output),
            )?;
            self.target = Some((
                rtv.ok_or_else(::windows::core::Error::empty)?,
                output.ok_or_else(::windows::core::Error::empty)?,
            ));
        }
        Ok(())
    }

    fn processor(
        &mut self,
        _gpu: &Gpu,
    ) -> WinResult<(ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor)> {
        if let Some(existing) = &self.processor {
            return Ok(existing.clone());
        }
        let rate = DXGI_RATIONAL {
            Numerator: 30,
            Denominator: 1,
        };
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: CANVAS.0,
            InputHeight: CANVAS.1,
            OutputFrameRate: rate,
            OutputWidth: self.size.0,
            OutputHeight: self.size.1,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        unsafe {
            let enumerator = self.video_device.CreateVideoProcessorEnumerator(&content)?;
            let processor = self.video_device.CreateVideoProcessor(&enumerator, 0)?;
            let background = D3D11_VIDEO_COLOR {
                Anonymous: D3D11_VIDEO_COLOR_0 {
                    RGBA: D3D11_VIDEO_COLOR_RGBA {
                        R: BACKGROUND[0],
                        G: BACKGROUND[1],
                        B: BACKGROUND[2],
                        A: 1.0,
                    },
                },
            };
            self.video_context.VideoProcessorSetOutputBackgroundColor(
                &processor,
                false,
                &background,
            );
            // Full-range RGB out; camera input is BT.709 limited-range YUV.
            let rgb = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 };
            let yuv = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                _bitfield: (1 << 2) | (1 << 4),
            };
            self.video_context
                .VideoProcessorSetOutputColorSpace(&processor, &rgb);
            self.video_context
                .VideoProcessorSetStreamColorSpace(&processor, 0, &rgb);
            self.video_context
                .VideoProcessorSetStreamColorSpace(&processor, 1, &yuv);
            for stream in 0..2 {
                self.video_context.VideoProcessorSetStreamFrameFormat(
                    &processor,
                    stream,
                    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                );
                self.video_context
                    .VideoProcessorSetStreamAutoProcessingMode(&processor, stream, false);
            }
            self.processor = Some((enumerator.clone(), processor.clone()));
            Ok((enumerator, processor))
        }
    }

    fn clear(&mut self, gpu: &Gpu, color: [f32; 4]) {
        if let Some((rtv, _)) = &self.target {
            unsafe { gpu.context.ClearRenderTargetView(rtv, &color) };
        }
    }

    /// Screen letterboxed into the canvas, the webcam in its corner box.
    fn draw(
        &mut self,
        gpu: &Gpu,
        screen: Option<&ScreenFrame>,
        camera: Option<&CameraFrame>,
    ) -> WinResult<()> {
        self.clear(gpu, BACKGROUND);
        let (enumerator, processor) = self.processor(gpu)?;
        let output = self
            .target
            .as_ref()
            .map(|(_, output)| output.clone())
            .ok_or_else(::windows::core::Error::empty)?;
        let canvas = fit_letterbox(CANVAS.0, CANVAS.1, self.size.0, self.size.1);
        let scale = f64::from(canvas.width) / f64::from(CANVAS.0);
        let to_output = |r: RECT| RECT {
            left: canvas.x + (f64::from(r.left) * scale).round() as i32,
            top: canvas.y + (f64::from(r.top) * scale).round() as i32,
            right: canvas.x + (f64::from(r.right) * scale).round() as i32,
            bottom: canvas.y + (f64::from(r.bottom) * scale).round() as i32,
        };
        if let Some(screen) = screen {
            let fit = fit_letterbox(screen.content.0, screen.content.1, CANVAS.0, CANVAS.1);
            let dest = to_output(RECT {
                left: fit.x,
                top: fit.y,
                right: fit.x + fit.width as i32,
                bottom: fit.y + fit.height as i32,
            });
            let source = RECT {
                left: 0,
                top: 0,
                right: screen.content.0 as i32,
                bottom: screen.content.1 as i32,
            };
            self.blit(
                &enumerator,
                &processor,
                &output,
                &screen.texture,
                source,
                dest,
                0,
            )?;
        }
        if let Some(camera) = camera {
            let texture = self.upload_camera(gpu, camera)?;
            let fit = fit_letterbox(
                camera.width,
                camera.height,
                (CAMERA_BOX.right - CAMERA_BOX.left) as u32,
                (CAMERA_BOX.bottom - CAMERA_BOX.top) as u32,
            );
            let dest = to_output(RECT {
                left: CAMERA_BOX.left + fit.x,
                top: CAMERA_BOX.top + fit.y,
                right: CAMERA_BOX.left + fit.x + fit.width as i32,
                bottom: CAMERA_BOX.top + fit.y + fit.height as i32,
            });
            let source = RECT {
                left: 0,
                top: 0,
                right: camera.width as i32,
                bottom: camera.height as i32,
            };
            self.blit(&enumerator, &processor, &output, &texture, source, dest, 1)?;
        }
        Ok(())
    }

    /// One video-processor pass drawing `texture` into `dest` only; the rest
    /// of the back buffer is left as is.
    #[allow(clippy::too_many_arguments)]
    fn blit(
        &self,
        enumerator: &ID3D11VideoProcessorEnumerator,
        processor: &ID3D11VideoProcessor,
        output: &ID3D11VideoProcessorOutputView,
        texture: &ID3D11Texture2D,
        source: RECT,
        dest: RECT,
        color_stream: u32,
    ) -> WinResult<()> {
        unsafe {
            let description = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV {
                        MipSlice: 0,
                        ArraySlice: 0,
                    },
                },
            };
            let mut input = None;
            self.video_device.CreateVideoProcessorInputView(
                texture,
                enumerator,
                &description,
                Some(&mut input),
            )?;
            // Stream 0 carries the colour space for this pass.
            let rgb = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 };
            let yuv = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                _bitfield: (1 << 2) | (1 << 4),
            };
            self.video_context.VideoProcessorSetStreamColorSpace(
                processor,
                0,
                if color_stream == 0 { &rgb } else { &yuv },
            );
            self.video_context
                .VideoProcessorSetOutputTargetRect(processor, true, Some(&dest));
            self.video_context
                .VideoProcessorSetStreamSourceRect(processor, 0, true, Some(&source));
            self.video_context
                .VideoProcessorSetStreamDestRect(processor, 0, true, Some(&dest));
            let streams = [D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                OutputIndex: 0,
                InputFrameOrField: 0,
                PastFrames: 0,
                FutureFrames: 0,
                ppPastSurfaces: std::ptr::null_mut(),
                pInputSurface: std::mem::ManuallyDrop::new(input),
                ppFutureSurfaces: std::ptr::null_mut(),
                ppPastSurfacesRight: std::ptr::null_mut(),
                pInputSurfaceRight: std::mem::ManuallyDrop::new(None),
                ppFutureSurfacesRight: std::ptr::null_mut(),
            }];
            let result = self
                .video_context
                .VideoProcessorBlt(processor, output, 0, &streams);
            let [mut stream] = streams;
            std::mem::ManuallyDrop::drop(&mut stream.pInputSurface);
            result
        }
    }

    /// Upload a packed NV12 camera frame through a staging texture.
    fn upload_camera(&mut self, gpu: &Gpu, camera: &CameraFrame) -> WinResult<ID3D11Texture2D> {
        let size = (camera.width, camera.height);
        if self
            .camera_staging
            .as_ref()
            .is_none_or(|(_, _, s)| *s != size)
        {
            let description = D3D11_TEXTURE2D_DESC {
                Width: size.0,
                Height: size.1,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            unsafe {
                gpu.device
                    .CreateTexture2D(&description, None, Some(&mut staging))?
            };
            let texture = gpu.texture(size.0, size.1, DXGI_FORMAT_NV12)?;
            self.camera_staging = Some((
                staging.ok_or_else(::windows::core::Error::empty)?,
                texture,
                size,
            ));
        }
        let (staging, texture, _) = self.camera_staging.as_ref().expect("created above");
        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            gpu.context
                .Map(staging, 0, D3D11_MAP_WRITE, 0, Some(&mut mapped))?;
            let (width, height) = (size.0 as usize, size.1 as usize);
            let pitch = mapped.RowPitch as usize;
            let base = mapped.pData as *mut u8;
            // Y rows, then the interleaved UV rows, each at the mapped pitch.
            for row in 0..height + height / 2 {
                std::ptr::copy_nonoverlapping(
                    camera.nv12.as_ptr().add(row * width),
                    base.add(row * pitch),
                    width,
                );
            }
            gpu.context.Unmap(staging, 0);
            gpu.context.CopyResource(texture, staging);
        }
        Ok(texture.clone())
    }

    fn present(&self) -> WinResult<()> {
        unsafe { self.swap_chain.Present(1, DXGI_PRESENT(0)).ok() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_hit_test_uses_the_inscribed_ellipse() {
        let bounds = RECT {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        assert!(inside_ellipse(POINT { x: 50, y: 50 }, bounds));
        assert!(!inside_ellipse(POINT { x: 2, y: 2 }, bounds));
        assert!(inside_ellipse(POINT { x: 50, y: 1 }, bounds));
    }

    #[test]
    fn camera_box_matches_macos_corner() {
        assert_eq!(CAMERA_BOX.right, 1260);
        assert_eq!(CAMERA_BOX.bottom, 700);
        assert_eq!(CAMERA_BOX.right - CAMERA_BOX.left, 280);
        assert_eq!(CAMERA_BOX.bottom - CAMERA_BOX.top, 158);
    }
}

#[cfg(test)]
mod on_screen {
    use super::*;
    use ::windows::Win32::Graphics::Gdi::ClientToScreen;

    fn pump(duration: Duration) {
        let deadline = std::time::Instant::now() + duration;
        while std::time::Instant::now() < deadline {
            let mut message = MSG::default();
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// (r, g, b) of the composed desktop at client point (x, y) of `hwnd`,
    /// read through Windows Graphics Capture: what a viewer actually sees.
    fn pixel(hwnd: HWND, x: i32, y: i32) -> (u8, u8, u8) {
        use crate::capture::windows::record::screen_capture::{ScreenCapture, ScreenTarget};
        let mut point = POINT { x, y };
        unsafe {
            let _ = ClientToScreen(hwnd, &mut point);
        }
        let gpu = gpu::shared().unwrap();
        let mut capture = ScreenCapture::open(
            &gpu,
            ScreenTarget::from_source_id("display:1").unwrap(),
            false,
        )
        .unwrap();
        let mut result = None;
        for _ in 0..100 {
            let _ = capture.drain(|texture, _| {
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                unsafe { texture.GetDesc(&mut desc) };
                desc.Usage = D3D11_USAGE_STAGING;
                desc.BindFlags = 0;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                desc.MiscFlags = 0;
                let mut staging = None;
                unsafe {
                    gpu.device
                        .CreateTexture2D(&desc, None, Some(&mut staging))?;
                    let staging = staging.unwrap();
                    gpu.context.CopyResource(&staging, texture);
                    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                    gpu.context
                        .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                    let offset = point.y as usize * mapped.RowPitch as usize + point.x as usize * 4;
                    let bgra =
                        std::slice::from_raw_parts((mapped.pData as *const u8).add(offset), 4);
                    result = Some((bgra[2], bgra[1], bgra[0]));
                    gpu.context.Unmap(&staging, 0);
                }
                Ok(())
            });
            if result.is_some() {
                break;
            }
            pump(Duration::from_millis(20));
        }
        result.expect("a captured frame")
    }

    #[test]
    #[ignore = "opens a window and reads screen pixels"]
    fn surface_draws_fixed_color_and_live_screen_on_screen() {
        let parent = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("AeroShoot preview test"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_CLIPCHILDREN,
                100,
                100,
                900,
                600,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        // Above every other window, so screen pixels belong to this test.
        unsafe {
            let _ = SetWindowPos(
                parent,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE,
            );
        }
        pump(Duration::from_millis(300));
        let viewport = PreviewViewport {
            window_label: "main".into(),
            x: 40.0,
            y: 40.0,
            width: 640.0,
            height: 360.0,
            backing_scale: 1.0,
            visible: true,
            occluded: false,
            revision: 1,
            generation: 0,
            clip: None,
        };

        let handle = attach(parent.0, 1).unwrap();
        set_geometry(handle, &viewport).unwrap();
        present_fixed(handle, 1.0, 0.0, 0.0);
        pump(Duration::from_millis(500));
        let red = pixel(parent, 40 + 320, 40 + 180);
        let child = surface(handle).hwnd;
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(child, &mut rect);
        }
        println!(
            "fixed red at centre: {red:?}; child visible {} rect {rect:?}; {}",
            unsafe { IsWindowVisible(child) }.as_bool(),
            stats_json(handle)
        );
        assert!(red.0 > 200 && red.1 < 40 && red.2 < 40, "{red:?}");
        // Excluded from capture, the same spot shows what is behind the window.
        let exclusion = crate::capture::windows::exclusion::exclude_own_windows();
        pump(Duration::from_millis(300));
        let hidden = pixel(parent, 40 + 320, 40 + 180);
        println!("with the app excluded from capture: {hidden:?}");
        assert_ne!(hidden, red, "excluded windows must not appear in captures");
        drop(exclusion);
        pump(Duration::from_millis(300));
        assert_eq!(
            pixel(parent, 40 + 320, 40 + 180),
            red,
            "visible to captures again"
        );
        detach(handle);

        // Checked with a camera-only preview: the webcam fills its corner box
        // and, with no screen, the rest of the canvas is background.
        let camera = crate::capture::windows::devices()
            .unwrap()
            .0
            .into_iter()
            .next();
        if let Some(camera) = camera {
            crate::capture::windows::preview::start(
                "display:1",
                false,
                false,
                Some(&camera.id),
                None,
                0.0,
            )
            .unwrap();
            let handle = attach(parent.0, 2).unwrap();
            set_geometry(handle, &viewport).unwrap();
            // Cameras take a moment to deliver their first frame.
            pump(Duration::from_millis(2500));
            let stats = stats_json(handle);
            // The 640×360 view shows the 1280×720 canvas at half scale.
            let corner = pixel(
                parent,
                40 + (CAMERA_BOX.left + CAMERA_BOX.right) / 4,
                40 + (CAMERA_BOX.top + CAMERA_BOX.bottom) / 4,
            );
            let centre = pixel(parent, 40 + 320, 40 + 180);
            println!("camera corner: {corner:?}; canvas centre: {centre:?}; {stats}");
            let background = BACKGROUND.map(|c| (c * 255.0).round() as i32);
            let near_background = |p: (u8, u8, u8)| {
                [p.0, p.1, p.2]
                    .iter()
                    .zip(background)
                    .all(|(v, b)| (i32::from(*v) - b).abs() <= 3)
            };
            assert!(
                near_background(centre),
                "no screen: the canvas centre is background"
            );
            assert!(!near_background(corner), "the webcam fills its corner box");
            detach(handle);
            crate::capture::windows::preview::stop();
        }
        unsafe {
            let _ = DestroyWindow(parent);
        }
    }

    /// Save a PNG of the first top-level window whose title starts with
    /// $AEROSHOOT_SNAPSHOT_TITLE to $AEROSHOOT_SNAPSHOT_PATH.
    #[test]
    #[ignore = "diagnostic: screenshots a running window"]
    fn snapshot_window() {
        use crate::capture::windows::record::screen_capture::{ScreenCapture, ScreenTarget};
        let title = std::env::var("AEROSHOOT_SNAPSHOT_TITLE").unwrap();
        let path = std::env::var("AEROSHOOT_SNAPSHOT_PATH").unwrap();
        // A window by title prefix, or a source ID such as `display:1`.
        let source = crate::capture::windows::capture_sources()
            .unwrap()
            .into_iter()
            .find(|s| {
                s.id == title
                    || s.name
                        .split(" \u{2014} ")
                        .nth(1)
                        .is_some_and(|t| t.starts_with(&title))
            })
            .expect("the source");
        let gpu = gpu::shared().unwrap();
        let mut capture = ScreenCapture::open(
            &gpu,
            ScreenTarget::from_source_id(&source.id).unwrap(),
            false,
        )
        .unwrap();
        let mut image = None;
        for _ in 0..100 {
            let _ = capture.drain(|texture, (width, height)| {
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                unsafe { texture.GetDesc(&mut desc) };
                desc.Usage = D3D11_USAGE_STAGING;
                desc.BindFlags = 0;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                desc.MiscFlags = 0;
                let mut staging = None;
                unsafe {
                    gpu.device
                        .CreateTexture2D(&desc, None, Some(&mut staging))?;
                    let staging = staging.unwrap();
                    gpu.context.CopyResource(&staging, texture);
                    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                    gpu.context
                        .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
                    for y in 0..height as usize {
                        let row = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
                        for x in 0..width as usize {
                            let p = std::slice::from_raw_parts(row.add(x * 4), 4);
                            rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
                        }
                    }
                    gpu.context.Unmap(&staging, 0);
                    image = Some((rgba, width, height));
                }
                Ok(())
            });
            if image.is_some() {
                break;
            }
            pump(Duration::from_millis(20));
        }
        let (rgba, width, height) = image.expect("a frame");
        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&rgba)
            .unwrap();
        println!("saved {width}x{height} of {} to {path}", source.name);
    }
}
