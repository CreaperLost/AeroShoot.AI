use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;

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
    pub screen_recording: bool,
    pub camera: bool,
    pub microphone: bool,
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
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGPreflightScreenCaptureAccess() -> bool;
        }
        let screen = unsafe { CGPreflightScreenCaptureAccess() };
        PermissionStatus {
            screen_recording: screen,
            camera: true,
            microphone: true,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionStatus {
            screen_recording: true,
            camera: true,
            microphone: true,
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
