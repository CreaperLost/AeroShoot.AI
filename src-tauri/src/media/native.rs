//! VideoToolbox decode/encode FFI. Pixels stay in-process.
use super::{ColorInfo, PixelFormat, VideoFrame, MAX_ENCODE_FRAMES, MAX_FRAME_DIM};
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::ptr::NonNull;

#[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
use std::ffi::CString;

#[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
extern "C" {
    fn aeroshoot_media_decode_bgra(
        path: *const c_char,
        time_us: i64,
        out_width: *mut i32,
        out_height: *mut i32,
        out_stride: *mut i32,
        out_pts_us: *mut i64,
        out_len: *mut i32,
    ) -> *mut u8;
    fn aeroshoot_media_free(ptr: *mut c_void);
    fn aeroshoot_media_encode_bgra_mp4(
        path: *const c_char,
        width: i32,
        height: i32,
        fps: i32,
        frame_count: i32,
        pixels: *const u8,
        len: i32,
        stride: i32,
    ) -> c_int;
    fn aeroshoot_preview_write_solid_mp4(
        path: *const c_char,
        width: i32,
        height: i32,
        r: f32,
        g: f32,
        b: f32,
    ) -> c_int;
}

pub fn decode_bgra(path: &Path, time_us: u64) -> Result<VideoFrame, String> {
    #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
    unsafe {
        let c_path =
            CString::new(path.to_string_lossy().as_ref()).map_err(|_| "Invalid media path")?;
        let mut width = 0i32;
        let mut height = 0i32;
        let mut stride = 0i32;
        let mut pts = 0i64;
        let mut len = 0i32;
        let pointer = aeroshoot_media_decode_bgra(
            c_path.as_ptr(),
            time_us as i64,
            &mut width,
            &mut height,
            &mut stride,
            &mut pts,
            &mut len,
        );
        let pointer = NonNull::new(pointer).ok_or("Failed to decode H.264 frame")?;
        if width <= 0 || height <= 0 || stride < width * 4 || len < stride * height {
            aeroshoot_media_free(pointer.as_ptr() as *mut c_void);
            return Err("Decoded frame had invalid geometry".into());
        }
        if width as u32 > MAX_FRAME_DIM || height as u32 > MAX_FRAME_DIM {
            aeroshoot_media_free(pointer.as_ptr() as *mut c_void);
            return Err("Decoded frame exceeds the F2 working-set limit".into());
        }
        let bytes = std::slice::from_raw_parts(pointer.as_ptr(), len as usize).to_vec();
        aeroshoot_media_free(pointer.as_ptr() as *mut c_void);
        Ok(VideoFrame {
            pts_us: pts.max(0) as u64,
            width: width as u32,
            height: height as u32,
            stride: stride as u32,
            format: PixelFormat::Bgra8888,
            color: ColorInfo::rec709_full(),
            data: bytes,
        })
    }
    #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
    {
        let _ = (path, time_us);
        Err("H.264 decode is not implemented on this platform".into())
    }
}

pub fn encode_bgra_mp4(path: &Path, frames: &[VideoFrame], fps: u32) -> Result<(), String> {
    if frames.is_empty() || frames.len() as u32 > MAX_ENCODE_FRAMES {
        return Err("Encoder requires 1..=8 frames".into());
    }
    let width = frames[0].width;
    let height = frames[0].height;
    let stride = frames[0].stride;
    super::validate_dim(width, height)?;
    if width % 2 != 0 || height % 2 != 0 {
        return Err("H.264 encoder requires even dimensions".into());
    }
    for frame in frames {
        if frame.width != width || frame.height != height || frame.stride != stride {
            return Err("All encoder frames must share geometry".into());
        }
        if frame.data.len() < (stride * height) as usize {
            return Err("Encoder frame buffer is truncated".into());
        }
    }
    let packed_stride = width * 4;
    let mut packed = Vec::with_capacity((packed_stride * height) as usize * frames.len());
    for frame in frames {
        for row in 0..height {
            let start = (row * frame.stride) as usize;
            packed.extend_from_slice(&frame.data[start..start + packed_stride as usize]);
        }
    }
    #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
    unsafe {
        let c_path =
            CString::new(path.to_string_lossy().as_ref()).map_err(|_| "Invalid media path")?;
        let code = aeroshoot_media_encode_bgra_mp4(
            c_path.as_ptr(),
            width as i32,
            height as i32,
            fps.max(1) as i32,
            frames.len() as i32,
            packed.as_ptr(),
            packed.len() as i32,
            packed_stride as i32,
        );
        if code == 0 {
            Ok(())
        } else {
            Err("Native H.264 encoder failed".into())
        }
    }
    #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
    {
        let _ = (path, fps, packed);
        Err("H.264 encode is not implemented on this platform".into())
    }
}

pub fn write_solid_mp4(
    path: &Path,
    width: u32,
    height: u32,
    r: f32,
    g: f32,
    b: f32,
) -> Result<(), String> {
    super::validate_dim(width, height)?;
    #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
    unsafe {
        let c_path =
            CString::new(path.to_string_lossy().as_ref()).map_err(|_| "Invalid media path")?;
        let code = aeroshoot_preview_write_solid_mp4(
            c_path.as_ptr(),
            width as i32,
            height as i32,
            r,
            g,
            b,
        );
        if code == 0 {
            Ok(())
        } else {
            Err("Failed to write the H.264 fixture".into())
        }
    }
    #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
    {
        let _ = (path, r, g, b);
        Err("H.264 fixture write is not implemented on this platform".into())
    }
}

#[cfg(all(test, stub_swift_ffi))]
mod stub_media_ffi {
    use super::*;

    #[no_mangle]
    pub extern "C" fn aeroshoot_media_decode_bgra(
        _path: *const c_char,
        _time_us: i64,
        out_width: *mut i32,
        out_height: *mut i32,
        out_stride: *mut i32,
        out_pts_us: *mut i64,
        out_len: *mut i32,
    ) -> *mut u8 {
        unsafe {
            if !out_width.is_null() {
                *out_width = 16;
            }
            if !out_height.is_null() {
                *out_height = 16;
            }
            if !out_stride.is_null() {
                *out_stride = 64;
            }
            if !out_pts_us.is_null() {
                *out_pts_us = 0;
            }
            if !out_len.is_null() {
                *out_len = 16 * 64;
            }
        }
        std::ptr::null_mut()
    }

    #[no_mangle]
    pub extern "C" fn aeroshoot_media_free(_ptr: *mut c_void) {}

    #[no_mangle]
    pub extern "C" fn aeroshoot_media_encode_bgra_mp4(
        _path: *const c_char,
        _width: i32,
        _height: i32,
        _fps: i32,
        _frame_count: i32,
        _pixels: *const u8,
        _len: i32,
        _stride: i32,
    ) -> c_int {
        1
    }
}
