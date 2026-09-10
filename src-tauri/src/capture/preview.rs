//! Native Record-scene preview. Configuration shares the recording lifecycle lock.
use crate::media::{ColorInfo, PixelFormat, VideoFrame};
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
pub fn frame() -> Option<VideoFrame> {
    let mut data = vec![0u8; 1280 * 720 * 4];
    #[cfg(target_os = "macos")]
    let flags = unsafe { aeroshoot_live_preview_read(data.as_mut_ptr().cast(), data.len() as i32) };
    #[cfg(not(target_os = "macos"))]
    let flags = 0;
    if flags == 0 {
        return None;
    }
    Some(VideoFrame {
        pts_us: 0,
        width: 1280,
        height: 720,
        stride: 1280 * 4,
        format: PixelFormat::Bgra8888,
        color: ColorInfo::rec709_full(),
        data,
    })
}

/// Latest camera mailbox frame from the existing capture session. Does not start a camera.
pub fn camera_frame() -> Option<VideoFrame> {
    let mut data = vec![0u8; 1280 * 720 * 4];
    #[cfg(target_os = "macos")]
    let flags =
        unsafe { aeroshoot_live_preview_read_camera(data.as_mut_ptr().cast(), data.len() as i32) };
    #[cfg(not(target_os = "macos"))]
    let flags = 0;
    if flags == 0 {
        return None;
    }
    Some(VideoFrame {
        pts_us: 0,
        width: 1280,
        height: 720,
        stride: 1280 * 4,
        format: PixelFormat::Bgra8888,
        color: ColorInfo::rec709_full(),
        data,
    })
}
