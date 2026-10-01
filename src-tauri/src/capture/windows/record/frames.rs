//! Newest screen and camera picture for the live preview, as on macOS: the
//! idle preview capture and the recorder both offer frames here, and the
//! preview surface draws whatever is newest. Nothing goes through the web view.
use super::gpu::Gpu;
use ::windows::core::Result;
use ::windows::Win32::Graphics::Direct3D11::{ID3D11Texture2D, D3D11_TEXTURE2D_DESC};
use ::windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// A BGRA texture on the shared device; `content` is the picture's size
/// within it.
#[derive(Clone)]
pub struct ScreenFrame {
    pub texture: ID3D11Texture2D,
    pub content: (u32, u32),
}

// SAFETY: the texture lives on the shared, multithread-protected device.
unsafe impl Send for ScreenFrame {}

/// A camera picture in NV12, tightly packed (Y plane, then interleaved UV).
pub struct CameraFrame {
    pub width: u32,
    pub height: u32,
    pub nv12: Vec<u8>,
}

#[derive(Default)]
struct Mailbox {
    screen: Option<ScreenFrame>,
    camera: Option<Arc<CameraFrame>>,
    hide_screen: bool,
    sequence: u64,
}

static MAILBOX: Mutex<Option<Mailbox>> = Mutex::new(None);
static RECORDING: AtomicBool = AtomicBool::new(false);

fn with<T>(f: impl FnOnce(&mut Mailbox) -> T) -> T {
    let mut guard = MAILBOX.lock().unwrap_or_else(PoisonError::into_inner);
    f(guard.get_or_insert_with(Mailbox::default))
}

/// Copy a screen picture into the mailbox's own texture.
pub fn offer_screen(gpu: &Gpu, source: &ID3D11Texture2D, content: (u32, u32)) -> Result<()> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { source.GetDesc(&mut desc) };
    with(|mailbox| {
        let reuse = mailbox.screen.as_ref().filter(|frame| {
            let mut own = D3D11_TEXTURE2D_DESC::default();
            unsafe { frame.texture.GetDesc(&mut own) };
            own.Width == desc.Width && own.Height == desc.Height
        });
        let texture = match reuse {
            Some(frame) => frame.texture.clone(),
            None => gpu.texture(desc.Width, desc.Height, DXGI_FORMAT_B8G8R8A8_UNORM)?,
        };
        unsafe { gpu.context.CopyResource(&texture, source) };
        mailbox.screen = Some(ScreenFrame { texture, content });
        mailbox.sequence = mailbox.sequence.wrapping_add(1);
        Ok(())
    })
}

pub fn offer_camera(frame: CameraFrame) {
    with(|mailbox| {
        mailbox.camera = Some(Arc::new(frame));
        mailbox.sequence = mailbox.sequence.wrapping_add(1);
    });
}

/// A camera-only session keeps no stale screen picture visible.
pub fn set_screen_shown(shown: bool) {
    with(|mailbox| {
        mailbox.hide_screen = !shown;
        mailbox.sequence = mailbox.sequence.wrapping_add(1);
    });
}

pub fn clear_camera() {
    with(|mailbox| {
        mailbox.camera = None;
        mailbox.sequence = mailbox.sequence.wrapping_add(1);
    });
}

pub fn clear() {
    with(|mailbox| {
        let sequence = mailbox.sequence.wrapping_add(1);
        *mailbox = Mailbox {
            sequence,
            ..Mailbox::default()
        };
    });
}

/// The newest pictures and a sequence number that changes with any update.
pub fn snapshot() -> (u64, Option<ScreenFrame>, Option<Arc<CameraFrame>>) {
    with(|mailbox| {
        let screen = (!mailbox.hide_screen)
            .then(|| mailbox.screen.clone())
            .flatten();
        (mailbox.sequence, screen, mailbox.camera.clone())
    })
}

/// While recording, the preview is only a monitor and renders at half rate.
pub fn set_recording(recording: bool) {
    RECORDING.store(recording, Ordering::SeqCst);
}

pub fn is_recording() -> bool {
    RECORDING.load(Ordering::SeqCst)
}
