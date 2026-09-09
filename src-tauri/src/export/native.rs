//! Streaming H.264/AAC writer FFI. Pixels and PCM stay in-process.
use crate::media::{validate_dim, VideoFrame, MAX_FRAME_DIM};
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::ptr::NonNull;

#[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
use std::ffi::CString;

#[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
extern "C" {
    fn aeroshoot_export_begin(
        path: *const c_char,
        width: i32,
        height: i32,
        fps: i32,
        sample_rate: i32,
        channels: i32,
    ) -> *mut c_void;
    fn aeroshoot_export_video(
        handle: *mut c_void,
        pts_us: i64,
        width: i32,
        height: i32,
        stride: i32,
        pixels: *const u8,
        len: i32,
    ) -> c_int;
    fn aeroshoot_export_audio(
        handle: *mut c_void,
        pts_us: i64,
        frames: i32,
        pcm: *const i16,
        len: i32,
    ) -> c_int;
    fn aeroshoot_export_finish(handle: *mut c_void, duration_us: i64) -> c_int;
    fn aeroshoot_export_abort(handle: *mut c_void);
    fn aeroshoot_media_duration_us(path: *const c_char) -> i64;
}

pub struct NativeExport {
    handle: Option<NonNull<c_void>>,
    width: u32,
    height: u32,
}

unsafe impl Send for NativeExport {}

impl NativeExport {
    pub fn begin(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, String> {
        validate_dim(width, height)?;
        if width % 2 != 0 || height % 2 != 0 {
            return Err("H.264 export requires even dimensions".into());
        }
        if width > MAX_FRAME_DIM || height > MAX_FRAME_DIM {
            return Err("Export canvas exceeds the working-set limit".into());
        }
        #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
        unsafe {
            let c_path =
                CString::new(path.to_string_lossy().as_ref()).map_err(|_| "Invalid export path")?;
            let handle = aeroshoot_export_begin(
                c_path.as_ptr(),
                width as i32,
                height as i32,
                fps as i32,
                sample_rate as i32,
                channels as i32,
            );
            let handle = NonNull::new(handle).ok_or("Failed to open the native export session")?;
            Ok(Self {
                handle: Some(handle),
                width,
                height,
            })
        }
        #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
        {
            let _ = (path, fps, sample_rate, channels);
            Err("Native H.264/AAC export is not implemented on this platform".into())
        }
    }

    pub fn write_video(&self, pts_us: u64, frame: &VideoFrame) -> Result<(), String> {
        let handle = self.handle.ok_or("Export session is closed")?;
        if frame.width != self.width || frame.height != self.height {
            return Err("Export frame geometry does not match the session".into());
        }
        let len = (frame.stride * frame.height) as usize;
        if frame.data.len() < len {
            return Err("Export frame buffer is truncated".into());
        }
        #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
        unsafe {
            let code = aeroshoot_export_video(
                handle.as_ptr(),
                pts_us as i64,
                frame.width as i32,
                frame.height as i32,
                frame.stride as i32,
                frame.data.as_ptr(),
                len as i32,
            );
            if code == 0 {
                Ok(())
            } else {
                Err("Native export video append failed".into())
            }
        }
        #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
        {
            let _ = (handle, pts_us, frame);
            Err("Native H.264/AAC export is not implemented on this platform".into())
        }
    }

    pub fn write_audio(
        &self,
        pts_us: u64,
        pcm: &[i16],
        frames: u32,
        channels: u16,
    ) -> Result<(), String> {
        let handle = self.handle.ok_or("Export session is closed")?;
        if frames == 0 {
            return Ok(());
        }
        let expected = frames as usize * channels.max(1) as usize;
        if pcm.len() < expected {
            return Err("Export audio buffer is truncated".into());
        }
        #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
        unsafe {
            let code = aeroshoot_export_audio(
                handle.as_ptr(),
                pts_us as i64,
                frames as i32,
                pcm.as_ptr(),
                pcm.len() as i32,
            );
            if code == 0 {
                Ok(())
            } else {
                Err("Native export audio append failed".into())
            }
        }
        #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
        {
            let _ = (handle, pts_us, pcm, channels);
            Err("Native H.264/AAC export is not implemented on this platform".into())
        }
    }

    pub fn finish(mut self, duration_us: u64) -> Result<(), String> {
        let handle = self.handle.take().ok_or("Export session is closed")?;
        #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
        unsafe {
            let code = aeroshoot_export_finish(handle.as_ptr(), duration_us as i64);
            if code == 0 {
                Ok(())
            } else {
                Err("Native export finish failed".into())
            }
        }
        #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
        {
            let _ = handle;
            Err("Native H.264/AAC export is not implemented on this platform".into())
        }
    }

    fn abort_handle(handle: NonNull<c_void>) {
        #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
        unsafe {
            aeroshoot_export_abort(handle.as_ptr());
        }
        #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
        {
            let _ = handle;
        }
    }
}

impl Drop for NativeExport {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            Self::abort_handle(handle);
        }
    }
}

pub fn media_duration_us(path: &Path) -> Result<u64, String> {
    #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
    unsafe {
        let c_path =
            CString::new(path.to_string_lossy().as_ref()).map_err(|_| "Invalid media path")?;
        let value = aeroshoot_media_duration_us(c_path.as_ptr());
        if value < 0 {
            return Err("Failed to read media duration".into());
        }
        Ok(value as u64)
    }
    #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
    {
        let _ = path;
        Err("Media duration is not implemented on this platform".into())
    }
}

#[cfg(all(test, stub_swift_ffi))]
mod stub_export_ffi {
    use super::*;

    #[no_mangle]
    pub extern "C" fn aeroshoot_export_begin(
        _path: *const c_char,
        _width: i32,
        _height: i32,
        _fps: i32,
        _sample_rate: i32,
        _channels: i32,
    ) -> *mut c_void {
        std::ptr::null_mut()
    }

    #[no_mangle]
    pub extern "C" fn aeroshoot_export_video(
        _handle: *mut c_void,
        _pts_us: i64,
        _width: i32,
        _height: i32,
        _stride: i32,
        _pixels: *const u8,
        _len: i32,
    ) -> c_int {
        1
    }

    #[no_mangle]
    pub extern "C" fn aeroshoot_export_audio(
        _handle: *mut c_void,
        _pts_us: i64,
        _frames: i32,
        _pcm: *const i16,
        _len: i32,
    ) -> c_int {
        1
    }

    #[no_mangle]
    pub extern "C" fn aeroshoot_export_finish(_handle: *mut c_void, _duration_us: i64) -> c_int {
        1
    }

    #[no_mangle]
    pub extern "C" fn aeroshoot_export_abort(_handle: *mut c_void) {}

    #[no_mangle]
    pub extern "C" fn aeroshoot_media_duration_us(_path: *const c_char) -> i64 {
        -1
    }
}
