//! Platform adapter for recorder HUD preview overlay.
use super::preview::{PreviewHitMode, PreviewViewport};
#[allow(unused_imports)]
use std::os::raw::{c_char, c_int, c_void};
use std::ptr::NonNull;

#[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
use std::ffi::{CStr, CString};

#[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
extern "C" {
    fn aeroshoot_preview_attach(ns_window: *mut c_void, generation: u64) -> *mut c_void;
    fn aeroshoot_preview_detach(handle: *mut c_void);
    fn aeroshoot_preview_set_geometry(
        handle: *mut c_void,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        backing_scale: f64,
        visible: bool,
        occluded: bool,
        revision: u64,
        generation: u64,
    ) -> c_int;
    fn aeroshoot_preview_set_clip(
        handle: *mut c_void,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> c_int;
    fn aeroshoot_preview_set_hit_mode(handle: *mut c_void, mode: c_int) -> c_int;
    fn aeroshoot_preview_present_fixed(
        handle: *mut c_void,
        r: f32,
        g: f32,
        b: f32,
        generation: u64,
    ) -> c_int;
    fn aeroshoot_preview_present_bgra(
        handle: *mut c_void,
        width: i32,
        height: i32,
        stride: i32,
        pixels: *const u8,
        len: i32,
        generation: u64,
    ) -> c_int;
    fn aeroshoot_preview_decode_present(
        handle: *mut c_void,
        path: *const c_char,
        generation: u64,
    ) -> c_int;
    fn aeroshoot_preview_copy_stats_json(handle: *mut c_void) -> *mut c_char;
}

#[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
extern "C" {
    fn aeroshoot_macos_free_string(value: *mut c_char);
}

pub fn attach(ns_window: *mut c_void, generation: u64) -> Result<NonNull<c_void>, String> {
    #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
    unsafe {
        NonNull::new(aeroshoot_preview_attach(ns_window, generation))
            .ok_or_else(|| "Failed to attach the native preview view".into())
    }
    #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
    {
        let _ = (ns_window, generation);
        Err("Native preview is not implemented on this platform".into())
    }
}

pub fn detach(handle: NonNull<c_void>) {
    #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
    unsafe {
        aeroshoot_preview_detach(handle.as_ptr());
    }
    #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
    {
        let _ = handle;
    }
}

pub fn set_geometry(
    handle: NonNull<c_void>,
    viewport: &PreviewViewport,
    generation: u64,
) -> Result<(), String> {
    #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
    unsafe {
        map_status(aeroshoot_preview_set_geometry(
            handle.as_ptr(),
            viewport.x,
            viewport.y,
            viewport.width,
            viewport.height,
            viewport.backing_scale,
            viewport.visible,
            viewport.occluded,
            viewport.revision,
            generation,
        ))?;
        let [x, y, w, h] = viewport
            .clip
            .unwrap_or([0.0, 0.0, viewport.width, viewport.height]);
        map_status(aeroshoot_preview_set_clip(handle.as_ptr(), x, y, w, h))
    }
    #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
    {
        let _ = (handle, viewport, generation);
        Ok(())
    }
}

pub fn set_hit_mode(handle: NonNull<c_void>, mode: PreviewHitMode) -> Result<(), String> {
    let value = match mode {
        PreviewHitMode::Consume => 0,
        PreviewHitMode::Circle => 1,
        PreviewHitMode::PassThrough => 2,
        PreviewHitMode::CirclePassThrough => 3,
        PreviewHitMode::SquirclePassThrough => 4,
    };
    #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
    unsafe {
        map_status(aeroshoot_preview_set_hit_mode(handle.as_ptr(), value))
    }
    #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
    {
        let _ = (handle, value);
        Ok(())
    }
}

pub fn present_fixed(
    handle: NonNull<c_void>,
    r: f32,
    g: f32,
    b: f32,
    generation: u64,
) -> Result<(), String> {
    #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
    unsafe {
        map_status(aeroshoot_preview_present_fixed(
            handle.as_ptr(),
            r,
            g,
            b,
            generation,
        ))
    }
    #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
    {
        let _ = (handle, r, g, b, generation);
        Ok(())
    }
}

/// Push a BGRA8 frame into the preview NSView. The pixel buffer must remain
/// valid for the duration of the call (Swift copies the bytes synchronously
/// inside `previewPresentBgra`).
pub fn present_frame(
    handle: NonNull<c_void>,
    width: u32,
    height: u32,
    stride: u32,
    pixels: &[u8],
    generation: u64,
) -> Result<(), String> {
    #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
    unsafe {
        map_status(aeroshoot_preview_present_bgra(
            handle.as_ptr(),
            width as i32,
            height as i32,
            stride as i32,
            pixels.as_ptr(),
            pixels.len() as i32,
            generation,
        ))
    }
    #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
    {
        let _ = (handle, width, height, stride, pixels, generation);
        Ok(())
    }
}

pub fn decode_present(handle: NonNull<c_void>, path: &str, generation: u64) -> Result<(), String> {
    #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
    unsafe {
        let c_path = CString::new(path).map_err(|_| "Invalid fixture path")?;
        map_status(aeroshoot_preview_decode_present(
            handle.as_ptr(),
            c_path.as_ptr(),
            generation,
        ))
    }
    #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
    {
        let _ = (handle, path, generation);
        Err("Native decode present requires the macOS preview adapter".into())
    }
}

#[allow(dead_code)]
pub fn stats_json(handle: NonNull<c_void>) -> Result<String, String> {
    #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
    unsafe {
        let pointer = aeroshoot_preview_copy_stats_json(handle.as_ptr());
        let pointer = NonNull::new(pointer).ok_or("Native preview stats were null")?;
        let value = CStr::from_ptr(pointer.as_ptr())
            .to_string_lossy()
            .into_owned();
        aeroshoot_macos_free_string(pointer.as_ptr());
        Ok(value)
    }
    #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
    {
        let _ = handle;
        Ok(r#"{"attached":false}"#.into())
    }
}

#[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
fn map_status(code: c_int) -> Result<(), String> {
    match code {
        0 => Ok(()),
        2 => Err("Stale preview generation".into()),
        3 => Err("Stale preview layout revision".into()),
        4 => Err("Invalid preview viewport".into()),
        5 => Err("Invalid preview BGRA buffer".into()),
        6 => Err("Failed to decode preview fixture".into()),
        _ => Err("Native preview adapter failed".into()),
    }
}

#[cfg(all(test, stub_swift_ffi))]
mod stub_preview_ffi {
    use super::*;

    #[no_mangle]
    pub extern "C" fn aeroshoot_preview_attach(
        _ns_window: *mut c_void,
        _generation: u64,
    ) -> *mut c_void {
        1 as *mut c_void
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_preview_detach(_handle: *mut c_void) {}
    #[no_mangle]
    pub extern "C" fn aeroshoot_preview_set_geometry(
        _handle: *mut c_void,
        _x: f64,
        _y: f64,
        _width: f64,
        _height: f64,
        _backing_scale: f64,
        _visible: bool,
        _occluded: bool,
        _revision: u64,
        _generation: u64,
    ) -> c_int {
        0
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_preview_set_hit_mode(_handle: *mut c_void, _mode: c_int) -> c_int {
        0
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_preview_present_fixed(
        _handle: *mut c_void,
        _r: f32,
        _g: f32,
        _b: f32,
        _generation: u64,
    ) -> c_int {
        0
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_preview_present_bgra(
        _handle: *mut c_void,
        _width: i32,
        _height: i32,
        _stride: i32,
        _pixels: *const u8,
        _len: i32,
        _generation: u64,
    ) -> c_int {
        0
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_preview_decode_present(
        _handle: *mut c_void,
        _path: *const c_char,
        _generation: u64,
    ) -> c_int {
        0
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_preview_copy_stats_json(_handle: *mut c_void) -> *mut c_char {
        unsafe { libc::strdup(b"{\"attached\":true}\0".as_ptr() as *const i8) }
    }
}
