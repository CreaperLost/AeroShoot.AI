//! Windows capture bridge: source and device enumeration, privacy state, and
//! the native recorder (Windows Graphics Capture, WASAPI, Media Foundation).
mod devices;
pub(crate) mod exclusion;
mod permissions;
pub(crate) mod preview;
pub(crate) mod preview_surface;
mod record;
mod sources;

use ::windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

pub use devices::devices;
pub use permissions::{permissions, request_permissions};
pub use record::WinCaptureSession;
pub use sources::capture_sources;

/// COM initialized as multithreaded for the current thread. Windows capture
/// runs on dedicated threads, never on the UI thread's apartment.
struct ComApartment;

impl ComApartment {
    fn enter() -> Result<Self, String> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|e| format!("COM initialization failed: {e}"))?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

/// Decode a NUL-terminated UTF-16 buffer.
fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}
