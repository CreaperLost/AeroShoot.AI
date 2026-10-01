//! Windows Graphics Capture of a display or window on the shared device.
//! Used by the screen recorder and the live preview.
use super::gpu::Gpu;
use ::windows::core::{factory, Interface};
use ::windows::Foundation::TypedEventHandler;
use ::windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use ::windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use ::windows::Graphics::DirectX::DirectXPixelFormat;
use ::windows::Graphics::SizeInt32;
use ::windows::Win32::Foundation::HWND;
use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use ::windows::Win32::Graphics::Dxgi::IDXGIDevice;
use ::windows::Win32::Graphics::Gdi::HMONITOR;
use ::windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use ::windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use ::windows::Win32::UI::WindowsAndMessaging::IsWindow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub enum ScreenTarget {
    Display(HMONITOR),
    Window(HWND),
}

// SAFETY: HMONITOR and HWND are plain system handles, valid from any thread.
unsafe impl Send for ScreenTarget {}

impl ScreenTarget {
    /// Resolve a `display:<n>` or `window:<hwnd>` source ID.
    pub fn from_source_id(source_id: &str) -> Result<Self, String> {
        if let Some(number) = source_id
            .strip_prefix("display:")
            .and_then(|n| n.parse().ok())
        {
            return crate::capture::windows::sources::monitor_for_display(number)
                .map(Self::Display)
                .ok_or_else(|| format!("Display {number} is not connected"));
        }
        if let Some(handle) = source_id
            .strip_prefix("window:")
            .and_then(|h| h.parse::<usize>().ok())
        {
            let hwnd = HWND(handle as *mut _);
            if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                return Ok(Self::Window(hwnd));
            }
            return Err("The selected window has been closed".into());
        }
        Err(format!("Unsupported capture source {source_id}"))
    }
}

pub struct ScreenCapture {
    winrt_device: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    _session: GraphicsCaptureSession,
    pool_size: SizeInt32,
    closed: Arc<AtomicBool>,
}

impl ScreenCapture {
    pub fn open(gpu: &Gpu, target: ScreenTarget, show_cursor: bool) -> Result<Self, String> {
        let describe = |e: ::windows::core::Error| format!("Could not start screen capture: {e}");
        unsafe {
            let interop =
                factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(describe)?;
            let item: GraphicsCaptureItem = match target {
                ScreenTarget::Display(monitor) => interop.CreateForMonitor(monitor),
                ScreenTarget::Window(hwnd) => interop.CreateForWindow(hwnd),
            }
            .map_err(describe)?;
            let dxgi: IDXGIDevice = gpu.device.cast().map_err(describe)?;
            let winrt_device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
                .and_then(|device| device.cast())
                .map_err(describe)?;
            let pool_size = item.Size().map_err(describe)?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &winrt_device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                pool_size,
            )
            .map_err(describe)?;
            let closed = Arc::new(AtomicBool::new(false));
            let flag = closed.clone();
            item.Closed(&TypedEventHandler::new(move |_, _| {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            }))
            .map_err(describe)?;
            let session = pool.CreateCaptureSession(&item).map_err(describe)?;
            let _ = session.SetIsCursorCaptureEnabled(show_cursor);
            // Windows 11 lets desktop apps hide the yellow capture border.
            let _ = session.SetIsBorderRequired(false);
            session.StartCapture().map_err(describe)?;
            Ok(Self {
                winrt_device,
                pool,
                _session: session,
                pool_size,
                closed,
            })
        }
    }

    /// The display was removed or the window closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Hand every delivered frame to `on_frame` with its picture size, oldest
    /// first, and return how many there were. Frames go back to the pool
    /// right after the callback.
    pub fn drain(
        &mut self,
        mut on_frame: impl FnMut(&ID3D11Texture2D, (u32, u32)) -> ::windows::core::Result<()>,
    ) -> Result<u32, String> {
        let mut count = 0;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            count += 1;
            let result = (|| -> ::windows::core::Result<()> {
                let content = frame.ContentSize()?;
                let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
                let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
                on_frame(
                    &texture,
                    (content.Width.max(1) as u32, content.Height.max(1) as u32),
                )?;
                // A resized window: reallocate buffers at the new size.
                if content.Width != self.pool_size.Width || content.Height != self.pool_size.Height
                {
                    self.pool_size = content;
                    self.pool.Recreate(
                        &self.winrt_device,
                        DirectXPixelFormat::B8G8R8A8UIntNormalized,
                        2,
                        content,
                    )?;
                }
                Ok(())
            })();
            let _ = frame.Close();
            result.map_err(|e| format!("Screen capture failed: {e}"))?;
        }
        Ok(count)
    }
}
