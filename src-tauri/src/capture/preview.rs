//! Native Record-scene preview. Configuration shares the recording lifecycle lock.
//!
//! `start` / `stop` bring the SCStream + AVCaptureSession up and tear them down.
//! `frame` / `camera_frame` pull the latest BGRA mailbox buffers so the live
//! preview pump can forward them into the studio / HUD preview surfaces.
#[cfg(target_os = "macos")]
extern "C" {
    fn aeroshoot_live_preview_start(
        source: *const std::ffi::c_char,
        camera: *const std::ffi::c_char,
    ) -> *mut std::ffi::c_char;
    fn aeroshoot_live_preview_stop();
    fn aeroshoot_live_preview_read(bytes: *mut std::ffi::c_void, length: i32) -> i32;
    fn aeroshoot_live_preview_read_camera(bytes: *mut std::ffi::c_void, length: i32) -> i32;
}

pub const PREVIEW_WIDTH: usize = 1280;
pub const PREVIEW_HEIGHT: usize = 720;
pub const PREVIEW_STRIDE: usize = PREVIEW_WIDTH * 4;
pub const PREVIEW_BYTES: usize = PREVIEW_STRIDE * PREVIEW_HEIGHT;

/// Flags returned by `aeroshoot_live_preview_read`. Bit 0 = screen, bit 1 = camera.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LivePreviewFlags(u32);

impl LivePreviewFlags {
    pub const NONE: LivePreviewFlags = LivePreviewFlags(0);
    pub const SCREEN: LivePreviewFlags = LivePreviewFlags(1);
    pub const CAMERA: LivePreviewFlags = LivePreviewFlags(2);
    pub const BOTH: LivePreviewFlags = LivePreviewFlags(3);

    pub fn has_screen(self) -> bool {
        self.0 & Self::SCREEN.0 != 0
    }
    pub fn has_camera(self) -> bool {
        self.0 & Self::CAMERA.0 != 0
    }
    pub fn raw(self) -> u32 {
        self.0
    }
}

impl std::ops::BitOr for LivePreviewFlags {
    type Output = LivePreviewFlags;
    fn bitor(self, rhs: LivePreviewFlags) -> LivePreviewFlags {
        LivePreviewFlags(self.0 | rhs.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveFrame {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    /// Bit flags from the native reader — what the mailbox actually contained.
    pub flags: LivePreviewFlags,
    pub data: Vec<u8>,
}

impl LiveFrame {
    pub fn new(flags: LivePreviewFlags, data: Vec<u8>) -> Self {
        Self {
            width: PREVIEW_WIDTH,
            height: PREVIEW_HEIGHT,
            stride: PREVIEW_STRIDE,
            flags,
            data,
        }
    }
}

pub fn stop() {
    #[cfg(target_os = "macos")]
    unsafe {
        aeroshoot_live_preview_stop();
    }
}

pub fn start(source: &str, camera: Option<&str>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    unsafe {
        let source = std::ffi::CString::new(source).map_err(|_| "Invalid source ID")?;
        let camera = camera
            .map(std::ffi::CString::new)
            .transpose()
            .map_err(|_| "Invalid camera ID")?;
        let error = aeroshoot_live_preview_start(
            source.as_ptr(),
            camera.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
        );
        if !error.is_null() {
            let message = std::ffi::CStr::from_ptr(error)
                .to_string_lossy()
                .into_owned();
            libc::free(error.cast());
            return Err(message);
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (source, camera);
        Err("Native capture preview is unavailable on this platform".into())
    }
}

/// Latest composited (screen + camera-bubble) mailbox frame from the live
/// preview session. Returns `None` if the mailbox is empty (both bits clear).
pub fn frame() -> Option<LiveFrame> {
    let mut data = vec![0u8; PREVIEW_BYTES];
    #[cfg(target_os = "macos")]
    let flags_raw = unsafe { aeroshoot_live_preview_read(data.as_mut_ptr().cast(), data.len() as i32) };
    #[cfg(not(target_os = "macos"))]
    let flags_raw = 0;
    let flags = LivePreviewFlags(flags_raw.max(0) as u32);
    if flags == LivePreviewFlags::NONE {
        return None;
    }
    Some(LiveFrame::new(flags, data))
}

/// Latest camera-only mailbox frame from the live preview session. The camera
/// stream is opened by `start(camera=Some(_))` and is left running if the user
/// toggled it back on; this reader never restarts capture on its own.
pub fn camera_frame() -> Option<LiveFrame> {
    let mut data = vec![0u8; PREVIEW_BYTES];
    #[cfg(target_os = "macos")]
    let flags_raw = unsafe {
        aeroshoot_live_preview_read_camera(data.as_mut_ptr().cast(), data.len() as i32)
    };
    #[cfg(not(target_os = "macos"))]
    let flags_raw = 0;
    let flags = LivePreviewFlags(flags_raw.max(0) as u32);
    if !flags.has_camera() {
        return None;
    }
    Some(LiveFrame::new(flags, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_compose() {
        assert_eq!(
            (LivePreviewFlags::SCREEN | LivePreviewFlags::CAMERA).raw(),
            LivePreviewFlags::BOTH.raw()
        );
        assert!(LivePreviewFlags::BOTH.has_screen());
        assert!(LivePreviewFlags::BOTH.has_camera());
        assert!(!LivePreviewFlags::SCREEN.has_camera());
        assert_eq!(LivePreviewFlags::default(), LivePreviewFlags::NONE);
    }

    #[test]
    fn live_frame_has_fixed_dimensions() {
        let frame = LiveFrame::new(LivePreviewFlags::SCREEN, vec![0; 16]);
        assert_eq!(frame.width, PREVIEW_WIDTH);
        assert_eq!(frame.height, PREVIEW_HEIGHT);
        assert_eq!(frame.stride, PREVIEW_STRIDE);
    }
}
