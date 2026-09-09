//! Shared decoder/encoder adapters. Frames are owned on the native/Rust side.
pub mod audio;
mod native;

use crate::project::pcm::PcmReader;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const MAX_FRAME_DIM: u32 = 4096;
pub const MAX_ENCODE_FRAMES: u32 = 8;
pub const CONCURRENT_ENCODER_LIMIT: u32 = 1;
pub const DECODER_BACKEND: &str = "videotoolbox";
pub const ENCODER_BACKEND: &str = "videotoolbox-h264";
pub const COMPOSITOR_BACKEND: &str = "wgpu-27.0.1";
pub const COPIES_DECODE: u32 = 1;
pub const COPIES_ENCODE: u32 = 1;
/// Mean absolute BGRA delta allowed between preview readback and independently decoded export.
pub const PARITY_MEAN_TOLERANCE: f32 = 16.0;
pub const PARITY_MAX_TOLERANCE: u8 = 255;
pub const PARITY_REGION_MEAN_TOLERANCE: f32 = 48.0;
pub const COMPOSITOR_MEAN_TOLERANCE: f32 = 3.0;
pub const COMPOSITOR_MAX_TOLERANCE: u8 = 40;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    Bgra8888,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ColorInfo {
    pub primaries: String,
    pub transfer: String,
    pub matrix: String,
    pub range: String,
    pub compositing_space: String,
}

impl ColorInfo {
    /// SDR Rec.709 RGB, full range in the in-process BGRA buffer.
    pub fn rec709_full() -> Self {
        Self {
            primaries: "bt709".into(),
            transfer: "bt709".into(),
            matrix: "identity".into(),
            range: "full".into(),
            compositing_space: "bt709_encoded".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VideoFrame {
    pub pts_us: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub color: ColorInfo,
    pub data: Vec<u8>,
}

impl VideoFrame {
    pub fn solid(
        width: u32,
        height: u32,
        b: u8,
        g: u8,
        r: u8,
        pts_us: u64,
    ) -> Result<Self, String> {
        validate_dim(width, height)?;
        let stride = width.saturating_mul(4);
        let mut data = vec![0u8; (stride * height) as usize];
        for pixel in data.chunks_exact_mut(4) {
            pixel[0] = b;
            pixel[1] = g;
            pixel[2] = r;
            pixel[3] = 255;
        }
        Ok(Self {
            pts_us,
            width,
            height,
            stride,
            format: PixelFormat::Bgra8888,
            color: ColorInfo::rec709_full(),
            data,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioBuffer {
    pub pts_us: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaInteropStatus {
    pub decoder_backend: String,
    pub compositor_backend: String,
    pub encoder_backend: String,
    pub ffmpeg_pinned: bool,
    pub supported: bool,
    pub preview_available: bool,
    pub copies_decode: u32,
    pub copies_composite: u32,
    pub copies_encode: u32,
    pub concurrent_encoder_limit: u32,
    pub color_space: String,
    pub wgpu_adapter: Option<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaParityReport {
    pub matched: bool,
    pub preview_width: u32,
    pub preview_height: u32,
    pub export_width: u32,
    pub export_height: u32,
    pub preview_pts_us: u64,
    pub export_pts_us: u64,
    pub max_abs_delta: u8,
    pub mean_abs_delta: f32,
    pub region_mean_delta: f32,
    pub compositor_mean_delta: f32,
    pub compositor_max_delta: u8,
    pub copies_decode: u32,
    pub copies_composite: u32,
    pub copies_encode: u32,
    pub concurrent_encoder_limit: u32,
    pub decoder_backend: String,
    pub compositor_backend: String,
    pub encoder_backend: String,
    pub ffmpeg_pinned: bool,
    pub color_space: String,
    pub pcm_peak: f32,
    pub pcm_rms: f32,
    pub diagnostics: Vec<String>,
}

#[derive(Debug)]
pub struct EncoderGate {
    busy: AtomicBool,
    in_flight: AtomicU32,
}

#[derive(Debug)]
pub struct EncoderSlot<'a> {
    gate: &'a EncoderGate,
}

impl EncoderGate {
    pub fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
            in_flight: AtomicU32::new(0),
        }
    }

    pub fn try_acquire(&self) -> Result<EncoderSlot<'_>, String> {
        if self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("Concurrent encoder limit is 1".into());
        }
        self.in_flight.store(1, Ordering::SeqCst);
        Ok(EncoderSlot { gate: self })
    }

    pub fn in_flight(&self) -> u32 {
        self.in_flight.load(Ordering::SeqCst)
    }
}

impl Drop for EncoderSlot<'_> {
    fn drop(&mut self) {
        self.gate.in_flight.store(0, Ordering::SeqCst);
        self.gate.busy.store(false, Ordering::SeqCst);
    }
}

impl Default for EncoderGate {
    fn default() -> Self {
        Self::new()
    }
}

pub fn validate_dim(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || width > MAX_FRAME_DIM || height > MAX_FRAME_DIM {
        return Err("Invalid media frame dimensions".into());
    }
    Ok(())
}

pub fn decode_h264_frame(path: &Path, time_us: u64) -> Result<VideoFrame, String> {
    native::decode_bgra(path, time_us)
}

pub fn encode_h264_frames(path: &Path, frames: &[VideoFrame], fps: u32) -> Result<(), String> {
    native::encode_bgra_mp4(path, frames, fps)
}

pub fn write_solid_h264(
    path: &Path,
    width: u32,
    height: u32,
    r: f32,
    g: f32,
    b: f32,
) -> Result<(), String> {
    native::write_solid_mp4(path, width, height, r, g, b)
}

pub fn decode_pcm(path: &Path, max_frames: usize) -> Result<AudioBuffer, String> {
    let mut reader = PcmReader::open(path)?;
    let info = reader.info().clone();
    let mut buf = vec![
        0.0f32;
        max_frames
            .saturating_mul(info.channels as usize)
            .max(info.channels as usize)
    ];
    let frames = reader.read_frames(&mut buf, max_frames)?;
    buf.truncate(frames * info.channels as usize);
    Ok(AudioBuffer {
        pts_us: 0,
        sample_rate: info.sample_rate,
        channels: info.channels,
        samples: buf,
    })
}

pub fn compare_frames(preview: &VideoFrame, exported: &VideoFrame) -> Result<(u8, f32), String> {
    if preview.width != exported.width || preview.height != exported.height {
        return Err("Preview and export dimensions differ".into());
    }
    if preview.format != exported.format {
        return Err("Preview and export pixel formats differ".into());
    }
    let inset = preview.width.min(preview.height).saturating_div(8).max(4);
    let mut max_delta = 0u8;
    let mut sum = 0u64;
    let mut count = 0u64;
    for y in inset..preview.height.saturating_sub(inset) {
        for x in inset..preview.width.saturating_sub(inset) {
            let pi = (y * preview.stride + x * 4) as usize;
            let ei = (y * exported.stride + x * 4) as usize;
            for c in 0..3 {
                let delta = preview.data[pi + c].abs_diff(exported.data[ei + c]);
                max_delta = max_delta.max(delta);
                sum += u64::from(delta);
                count += 1;
            }
        }
    }
    if count == 0 {
        return Err("Parity compare window was empty".into());
    }
    Ok((max_delta, sum as f32 / count as f32))
}

fn region_mean(frame: &VideoFrame, x: u32, y: u32, w: u32, h: u32) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    let mut count = 0.0f32;
    for row in y..y.saturating_add(h).min(frame.height) {
        for col in x..x.saturating_add(w).min(frame.width) {
            let i = (row * frame.stride + col * 4) as usize;
            sum[0] += f32::from(frame.data[i]);
            sum[1] += f32::from(frame.data[i + 1]);
            sum[2] += f32::from(frame.data[i + 2]);
            count += 1.0;
        }
    }
    if count > 0.0 {
        [sum[0] / count, sum[1] / count, sum[2] / count]
    } else {
        [0.0, 0.0, 0.0]
    }
}

pub fn region_mean_delta(preview: &VideoFrame, exported: &VideoFrame) -> f32 {
    let mut worst = 0.0f32;
    for (x, y, w, h) in [(2u32, 50, 6, 6), (22, 26, 12, 8), (50, 8, 4, 4)] {
        let a = region_mean(preview, x, y, w, h);
        let b = region_mean(exported, x, y, w, h);
        for i in 0..3 {
            worst = worst.max((a[i] - b[i]).abs());
        }
    }
    worst
}

pub fn interop_status(
    adapter_name: Option<String>,
    diagnostics: Vec<String>,
) -> MediaInteropStatus {
    MediaInteropStatus {
        decoder_backend: DECODER_BACKEND.into(),
        compositor_backend: COMPOSITOR_BACKEND.into(),
        encoder_backend: ENCODER_BACKEND.into(),
        ffmpeg_pinned: false,
        supported: cfg!(target_os = "macos"),
        preview_available: false,
        copies_decode: COPIES_DECODE,
        copies_composite: crate::render::COPIES_COMPOSITE,
        copies_encode: COPIES_ENCODE,
        concurrent_encoder_limit: CONCURRENT_ENCODER_LIMIT,
        color_space: "rec709_full".into(),
        wgpu_adapter: adapter_name,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_gate_rejects_a_second_encode() {
        let gate = EncoderGate::new();
        let first = gate.try_acquire().unwrap();
        assert_eq!(gate.in_flight(), 1);
        assert!(gate
            .try_acquire()
            .unwrap_err()
            .contains("Concurrent encoder"));
        drop(first);
        assert_eq!(gate.in_flight(), 0);
        let _second = gate.try_acquire().expect("gate should release");
    }

    #[test]
    fn solid_frame_is_owned_bgra() {
        let frame = VideoFrame::solid(16, 16, 16, 32, 48, 1_000).unwrap();
        assert_eq!(frame.format, PixelFormat::Bgra8888);
        assert_eq!(frame.data.len(), 16 * 16 * 4);
        assert_eq!(&frame.data[..4], &[16, 32, 48, 255]);
        let json = serde_json::to_value(interop_status(None, Vec::new())).unwrap();
        assert!(json.get("pixels").is_none());
        assert_eq!(json["previewAvailable"], false);
        assert_eq!(json["ffmpegPinned"], false);
        assert_eq!(json["copiesComposite"], 2);
    }
}
