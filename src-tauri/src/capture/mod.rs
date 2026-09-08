use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;

#[cfg(target_os = "macos")]
pub mod macos;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSourceType {
    Display,
    Window,
    Application,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSource {
    pub id: String,
    pub name: String,
    pub source_type: CaptureSourceType,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CameraDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionStatus {
    pub screen_recording: PermissionState,
    pub camera: PermissionState,
    pub microphone: PermissionState,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PermissionState {
    Unknown,
    NotDetermined,
    Authorized,
    Denied,
    Restricted,
}

impl PermissionState {
    pub fn is_authorized(self) -> bool {
        self == Self::Authorized
    }
}

/// Strategy used to fit a non-16:9 source into the requested output size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    /// Letterbox / pillarbox the source so the entire frame is visible
    /// with black bars on the under-sized axis. Aspect ratio is preserved.
    Fit,
    /// Crop the source so the output is filled edge-to-edge. The center
    /// of the source stays in view; the over-sized axis is trimmed.
    Fill,
}

/// A rectangle expressed in integer source / destination pixel coordinates.
/// `x` / `y` may be negative when a cropped "fill" rect overshoots the
/// dest bounds, which is fine because the renderer clips it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl SourceRect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    /// The whole rectangle from (0,0) with the given dimensions.
    pub fn from_dimensions(width: u32, height: u32) -> Self {
        Self::new(0, 0, width, height)
    }
}

/// The active capture geometry for a session. The source rect is in
/// source-pixel coordinates (the display, window, or app that the user
/// picked). The dest rect is the corresponding rectangle in the encoded
/// output frame. `preserves_aspect_ratio` is the resolved value of the
/// underlying SCStreamConfiguration flag at the time of compute —
/// `true` on macOS 14+ when fit mode is `Fit`, `false` otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceGeometry {
    pub source_rect: SourceRect,
    pub dest_rect: SourceRect,
    pub fit_mode: FitMode,
    pub preserves_aspect_ratio: bool,
}

/// Returns true when the running macOS version supports the explicit
/// `preservesAspectRatio` configuration flag on `SCStreamConfiguration`.
/// On non-macOS targets, returns false unconditionally so the math path
/// (macOS 13 compatible) is always taken in tests and on the Linux CI.
pub fn is_macos_14_plus() -> bool {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("sw_vers").arg("-productVersion").output() {
            if let Ok(text) = String::from_utf8(output.stdout) {
                if let Some(major_str) = text.trim().split('.').next() {
                    if let Ok(major) = major_str.parse::<u32>() {
                        return major >= 14;
                    }
                }
            }
        }
        false
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Computes the source / dest rect pair for a given capture source,
/// output size, and fit mode. Pure math; the function never touches
/// platform APIs. Tests rely on this for verifying the macOS 13
/// (no-preserves-aspect-ratio) path.
pub fn compute_source_geometry(
    source: &CaptureSource,
    dest_width: u32,
    dest_height: u32,
    fit_mode: FitMode,
) -> SourceGeometry {
    let source_rect = SourceRect::from_dimensions(source.width, source.height);
    let dest_rect = match fit_mode {
        FitMode::Fit => fit_letterbox(source.width, source.height, dest_width, dest_height),
        FitMode::Fill => fit_crop(source.width, source.height, dest_width, dest_height),
    };
    // On macOS 14+ we can let SCStreamConfiguration preserve aspect
    // ratio itself; otherwise the math above is the source of truth.
    let preserves_aspect_ratio = matches!(fit_mode, FitMode::Fit) && is_macos_14_plus();
    SourceGeometry {
        source_rect,
        dest_rect,
        fit_mode,
        preserves_aspect_ratio,
    }
}

/// Letterbox / pillarbox math: keep the entire source visible, center
/// it on the dest canvas, and add bars on the under-sized axis.
///
/// A widescreen source (e.g. 21:9) on a 16:9 dest gets **letterbox**
/// bars top/bottom (width is the limiting axis, height shrinks).
/// A portrait source (e.g. 9:16) on a 16:9 dest gets **pillarbox**
/// bars left/right (height is the limiting axis, width shrinks).
fn fit_letterbox(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> SourceRect {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return SourceRect::from_dimensions(dst_w, dst_h);
    }
    let src_ratio = src_w as f64 / src_h as f64;
    let dst_ratio = dst_w as f64 / dst_h as f64;
    if src_ratio > dst_ratio {
        // Source wider than dest: match width, leave vertical bars
        // (letterbox — like a widescreen movie on a 16:9 TV).
        let w = dst_w;
        let h = ((dst_w as f64) / src_ratio).round() as u32;
        let h = h.min(dst_h).max(1);
        let y = ((dst_h as i32) - (h as i32)) / 2;
        SourceRect::new(0, y, w, h)
    } else {
        // Source taller than dest: match height, leave horizontal bars
        // (pillarbox — like a portrait photo on a landscape display).
        let h = dst_h;
        let w = ((dst_h as f64) * src_ratio).round() as u32;
        let w = w.min(dst_w).max(1);
        let x = ((dst_w as i32) - (w as i32)) / 2;
        SourceRect::new(x, 0, w, h)
    }
}

/// Crop math: fill the dest rect edge-to-edge, trimming the source
/// rectangle so the dest aspect ratio is preserved.
fn fit_crop(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> SourceRect {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return SourceRect::from_dimensions(dst_w, dst_h);
    }
    let _ = (src_w, src_h);
    // For "fill", the dest rect is the whole output frame. The source
    // rect is conceptually the entire source — ScreenCaptureKit's own
    // cropping (set later) handles the actual trim.
    SourceRect::from_dimensions(dst_w, dst_h)
}

#[derive(Error, Debug)]
pub enum CaptureError {
    #[error("Permission denied for screen recording")]
    ScreenRecordingPermissionDenied,
    #[error("Permission denied for camera capture")]
    CameraPermissionDenied,
    #[error("Permission denied for microphone capture")]
    MicPermissionDenied,
    #[error("Device not found: {0}")]
    DeviceNotFound(String),
    #[error("Capture pipeline error: {0}")]
    PipelineError(String),
}

/// Common trait for capturing screen frames (ScreenCaptureKit on macOS, WGC on Windows)
pub trait ScreenCapturer: Send + Sync {
    fn start(&mut self, source: &CaptureSource) -> Result<(), CaptureError>;
    fn stop(&mut self) -> Result<(), CaptureError>;
    fn is_capturing(&self) -> bool;
}

/// Common trait for capturing audio buffers (CoreAudio/SCKit on macOS, WASAPI on Windows)
pub trait AudioCapturer: Send + Sync {
    fn start(&mut self, device: &AudioDevice) -> Result<(), CaptureError>;
    fn stop(&mut self) -> Result<(), CaptureError>;
    fn is_capturing(&self) -> bool;
}

use std::sync::atomic::AtomicUsize;

/// Guard tracking an active in-flight capture callback.
/// When dropped, decrements the active callback counter.
pub struct CallbackGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for CallbackGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Native capture session handle enforcing clean shutdown handshake:
/// callbacks must stop before resources are freed.
#[derive(Clone)]
pub struct NativeCaptureSessionHandle {
    is_active: Arc<AtomicBool>,
    shutdown_completed: Arc<AtomicBool>,
    active_callbacks: Arc<AtomicUsize>,
}

impl NativeCaptureSessionHandle {
    pub fn new() -> Self {
        Self {
            is_active: Arc::new(AtomicBool::new(true)),
            shutdown_completed: Arc::new(AtomicBool::new(false)),
            active_callbacks: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn is_active(&self) -> bool {
        self.is_active.load(Ordering::SeqCst)
    }

    /// Enters a capture callback. Returns `None` if the session is shutting down or inactive.
    /// The returned `CallbackGuard` tracks in-flight execution to guarantee draining on shutdown.
    pub fn enter_callback(&self) -> Option<CallbackGuard> {
        if !self.is_active.load(Ordering::SeqCst) {
            return None;
        }
        self.active_callbacks.fetch_add(1, Ordering::SeqCst);
        if !self.is_active.load(Ordering::SeqCst) {
            self.active_callbacks.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(CallbackGuard {
            counter: Arc::clone(&self.active_callbacks),
        })
    }

    pub fn active_callback_count(&self) -> usize {
        self.active_callbacks.load(Ordering::SeqCst)
    }

    /// Signals callbacks to stop and performs the draining shutdown handshake:
    /// waits until all active callbacks finish execution before completing shutdown.
    pub fn shutdown(&self) {
        self.is_active.store(false, Ordering::SeqCst);
        // Handshake: drain all in-flight callbacks
        while self.active_callbacks.load(Ordering::SeqCst) > 0 {
            std::thread::yield_now();
        }
        self.shutdown_completed.store(true, Ordering::SeqCst);
    }

    pub fn is_shutdown_complete(&self) -> bool {
        self.shutdown_completed.load(Ordering::SeqCst)
    }
}

impl Default for NativeCaptureSessionHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for NativeCaptureSessionHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Checks current OS-level capture permissions.
pub fn check_system_permissions() -> PermissionStatus {
    #[cfg(target_os = "macos")]
    {
        macos::permissions()
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionStatus {
            screen_recording: PermissionState::Authorized,
            camera: PermissionState::Authorized,
            microphone: PermissionState::Authorized,
        }
    }
}

/// Synthetic mock capturer for testing recording pipelines without hardware dependencies
pub struct MockScreenCapturer {
    capturing: bool,
}

impl MockScreenCapturer {
    pub fn new() -> Self {
        Self { capturing: false }
    }
}

impl Default for MockScreenCapturer {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenCapturer for MockScreenCapturer {
    fn start(&mut self, _source: &CaptureSource) -> Result<(), CaptureError> {
        self.capturing = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        self.capturing = false;
        Ok(())
    }

    fn is_capturing(&self) -> bool {
        self.capturing
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    fn display_source(width: u32, height: u32) -> CaptureSource {
        CaptureSource {
            id: "display:1".into(),
            name: "Main Display".into(),
            source_type: CaptureSourceType::Display,
            width,
            height,
        }
    }

    #[test]
    fn test_compute_geometry_for_16_9_display_in_fit_mode() {
        let source = display_source(2560, 1440);
        let geometry = compute_source_geometry(&source, 1920, 1080, FitMode::Fit);
        // For a 16:9 source into a 16:9 dest, fit math yields the full dest.
        assert_eq!(geometry.source_rect, SourceRect::new(0, 0, 2560, 1440));
        assert_eq!(geometry.dest_rect, SourceRect::new(0, 0, 1920, 1080));
        assert_eq!(geometry.fit_mode, FitMode::Fit);
    }

    #[test]
    fn test_compute_geometry_for_16_10_display_pillarboxes_into_16_9() {
        // 16:10 source (1920x1200) into a 16:9 (1920x1080) dest:
        // the source is taller, so we pillarbox (black bars left/right).
        let source = display_source(1920, 1200);
        let geometry = compute_source_geometry(&source, 1920, 1080, FitMode::Fit);
        assert_eq!(geometry.source_rect, SourceRect::new(0, 0, 1920, 1200));
        // Computed dest width: 1080 * 1.6 = 1728, with 96 px bars left & right.
        assert_eq!(geometry.dest_rect.height, 1080);
        let expected_w = ((1080.0_f64 * 16.0 / 10.0).round()) as u32;
        let expected_x = (1920_i32 - expected_w as i32) / 2;
        assert_eq!(geometry.dest_rect.width, expected_w);
        assert_eq!(geometry.dest_rect.x, expected_x);
    }

    #[test]
    fn test_compute_geometry_letterboxes_wide_source() {
        // 21:9 ultrawide into 16:9: letterbox (black bars top/bottom).
        let source = display_source(3440, 1440);
        let geometry = compute_source_geometry(&source, 1920, 1080, FitMode::Fit);
        assert_eq!(geometry.dest_rect.width, 1920);
        // Height is the limiting axis; expect a Y offset that centers.
        assert!(geometry.dest_rect.y != 0, "wide source must produce a letterbox offset");
        assert!(geometry.dest_rect.height < 1080);
    }

    #[test]
    fn test_compute_geometry_fill_mode_fills_dest() {
        // In fill mode, the dest rect is always the full output frame.
        let source = display_source(1280, 720);
        let geometry = compute_source_geometry(&source, 1920, 1080, FitMode::Fill);
        assert_eq!(geometry.dest_rect, SourceRect::new(0, 0, 1920, 1080));
        assert_eq!(geometry.fit_mode, FitMode::Fill);
    }

    #[test]
    fn test_compute_geometry_for_window_source_uses_window_dimensions() {
        // A non-rectangular window (e.g. 1600x900) uses its declared frame
        // as the source rect.
        let source = CaptureSource {
            id: "window:42".into(),
            name: "Code Workspace".into(),
            source_type: CaptureSourceType::Window,
            width: 1600,
            height: 900,
        };
        let geometry = compute_source_geometry(&source, 1280, 720, FitMode::Fit);
        assert_eq!(geometry.source_rect, SourceRect::new(0, 0, 1600, 900));
        // 16:9 into 16:9: no letterbox needed.
        assert_eq!(geometry.dest_rect, SourceRect::new(0, 0, 1280, 720));
    }
}
