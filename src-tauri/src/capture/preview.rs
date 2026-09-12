//! Native Record-scene preview. Configuration shares the recording lifecycle lock.
#[cfg(target_os = "macos")]
extern "C" {
    fn aeroshoot_live_preview_start(
        source: *const std::ffi::c_char,
        camera: *const std::ffi::c_char,
    ) -> *mut std::ffi::c_char;
    fn aeroshoot_live_preview_stop();
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
