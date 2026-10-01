//! Platform capture backend. Lifecycle commands talk only to this module;
//! each OS plugs its native recorder in here, so shared code never names a
//! platform bridge directly.
//!
//! - macOS: ScreenCaptureKit / AVFoundation through the Swift bridge.
//! - Windows: Windows Graphics Capture, WASAPI, and Media Foundation.
//! - Every other target: an unsupported backend.
//!
//! Without a native recorder, commands fall back to the synthetic development
//! path.
use super::{
    AudioDevice, CameraDevice, CaptureSource, NativeCaptureStats, NativeRecordingConfig,
    PermissionStatus,
};
use crate::project::journal::ProjectJournal;
use crate::project::segment_writer::TrackSegmentWriter;
use crate::session::{SessionDiagnostics, SessionEpoch, SessionStateMachine};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Contract for a live native recording. A platform recorder writes each track
/// as independently valid segments of at most 60 seconds (fMP4 H.264 video,
/// WAV PCM audio) and publishes them through the callback targets installed by
/// [`install_callback_targets`].
pub trait NativeCaptureSession: Sized + Send + Sync {
    /// Start capture. Sources warm up during `config.start_delay_ms`.
    fn start(config: NativeRecordingConfig<'_>) -> Result<Self, String>;

    /// `false` resumes after [`Self::pause_and_finalize`]; the next sample
    /// opens a new segment.
    fn set_paused(&self, paused: bool);

    /// Stop sample admission and finalize open segments before Pause is
    /// acknowledged.
    fn pause_and_finalize(&self) -> Result<(), (i32, String)>;

    fn stats(&self) -> NativeCaptureStats;

    /// Stop capture and surface the encoder's final status. Segments are
    /// published before this returns.
    fn stop_with_result(self) -> Result<(), (i32, String)>;

    /// Wait until every selected source that is expected to produce continuous
    /// samples has delivered at least one callback. System audio is excluded:
    /// loopback capture legitimately emits no audio buffers while the system is
    /// silent, and a successful start is its readiness signal. This prevents the
    /// app from advertising Recording while a stale display, busy camera, or
    /// unavailable microphone is producing nothing.
    fn wait_until_ready(
        &self,
        capture_screen: bool,
        camera_enabled: bool,
        mic_enabled: bool,
        timeout: Duration,
    ) -> Result<(), String> {
        if !capture_screen && !camera_enabled && !mic_enabled {
            return Ok(());
        }
        let deadline = Instant::now() + timeout;
        loop {
            let stats = self.stats();
            if let Some(error) = stats.last_error.filter(|error| !error.is_empty()) {
                return Err(format!("native capture failed during startup: {error}"));
            }
            let screen_ready = !capture_screen || stats.screen_samples > 0;
            let camera_ready = !camera_enabled || stats.camera_samples > 0;
            let mic_ready = !mic_enabled || stats.mic_samples > 0;
            if screen_ready && camera_ready && mic_ready {
                return Ok(());
            }
            if Instant::now() >= deadline {
                let mut pending = Vec::new();
                if !screen_ready {
                    pending.push("screen");
                }
                if !camera_ready {
                    pending.push("webcam");
                }
                if !mic_ready {
                    pending.push("microphone");
                }
                return Err(format!(
                    "Timed out waiting for first native sample from {}",
                    pending.join(", ")
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    pub use crate::capture::macos::{
        capture_sources, devices, mouse_permission, permissions, request_permissions,
        MacCaptureSession as Session,
    };
    pub(crate) use crate::capture::macos::{
        clear_callback_targets, install_callback_targets, set_callback_epoch,
        take_native_segment_writers,
    };

    pub const NATIVE_RECORDING: bool = true;
    pub const NATIVE_DEVICES: bool = true;
    pub const MOUSE_TELEMETRY: bool = true;
    pub const CAMERA_FORMATS: bool = true;
}

#[cfg(target_os = "windows")]
mod platform {
    pub(crate) use crate::capture::segments::{
        clear_callback_targets, install_callback_targets, set_callback_epoch,
        take_native_segment_writers,
    };
    pub use crate::capture::windows::{
        capture_sources, devices, permissions, request_permissions, WinCaptureSession as Session,
    };

    /// The low-level mouse hook needs no permission on Windows.
    pub fn mouse_permission(_request: bool) -> bool {
        true
    }

    pub const NATIVE_RECORDING: bool = true;
    pub const NATIVE_DEVICES: bool = true;
    pub const MOUSE_TELEMETRY: bool = true;
    pub const CAMERA_FORMATS: bool = true;
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::*;
    use crate::capture::PermissionState;

    pub use super::unsupported::{
        clear_callback_targets, install_callback_targets, mouse_permission, set_callback_epoch,
        take_native_segment_writers, Session, UNSUPPORTED,
    };

    pub const NATIVE_RECORDING: bool = false;
    pub const NATIVE_DEVICES: bool = false;
    pub const MOUSE_TELEMETRY: bool = false;
    pub const CAMERA_FORMATS: bool = false;

    pub fn capture_sources() -> Result<Vec<CaptureSource>, String> {
        Err(UNSUPPORTED.into())
    }

    pub fn devices() -> Result<(Vec<CameraDevice>, Vec<AudioDevice>), String> {
        Err(UNSUPPORTED.into())
    }

    pub fn permissions() -> PermissionStatus {
        PermissionStatus {
            screen_recording: PermissionState::Authorized,
            camera: PermissionState::Authorized,
            microphone: PermissionState::Authorized,
        }
    }

    pub fn request_permissions(
        _screen: bool,
        _camera: bool,
        _microphone: bool,
    ) -> PermissionStatus {
        permissions()
    }
}

/// Recording pieces for platforms without a native recorder.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported {
    use super::*;

    pub const UNSUPPORTED: &str = "Native capture is not implemented on this platform";

    /// Uninhabited: no native session can exist without a native recorder.
    pub enum Session {}

    impl NativeCaptureSession for Session {
        fn start(_config: NativeRecordingConfig<'_>) -> Result<Self, String> {
            Err(UNSUPPORTED.into())
        }
        fn set_paused(&self, _paused: bool) {
            match *self {}
        }
        fn pause_and_finalize(&self) -> Result<(), (i32, String)> {
            match *self {}
        }
        fn stats(&self) -> NativeCaptureStats {
            match *self {}
        }
        fn stop_with_result(self) -> Result<(), (i32, String)> {
            match self {}
        }
    }

    pub fn mouse_permission(_request: bool) -> bool {
        false
    }

    pub fn install_callback_targets(
        _diagnostics: Arc<SessionDiagnostics>,
        _state_machine: Arc<SessionStateMachine>,
        _epoch: SessionEpoch,
        _journal: Option<Arc<ProjectJournal>>,
        _project_root: Option<PathBuf>,
    ) {
    }

    pub fn set_callback_epoch(_epoch: SessionEpoch) {}

    pub fn take_native_segment_writers() -> Vec<TrackSegmentWriter> {
        Vec::new()
    }

    pub fn clear_callback_targets() {}
}

/// The platform's native recording session type.
pub type Session = platform::Session;

/// Whether this build has a native recorder. When `false`, lifecycle commands
/// use the synthetic development path.
pub const NATIVE_RECORDING: bool = platform::NATIVE_RECORDING;

/// Whether sources and devices come from the OS. When `false`, listings fall
/// back to synthetic development entries.
pub const NATIVE_DEVICES: bool = platform::NATIVE_DEVICES;

/// Whether pointer telemetry can be recorded on this platform.
pub const MOUSE_TELEMETRY: bool = platform::MOUSE_TELEMETRY;

/// Whether the recorder honours a requested camera resolution, frame rate,
/// and bitrate. Without it the camera is recorded at 1280×720.
pub const CAMERA_FORMATS: bool = platform::CAMERA_FORMATS;

/// Screens, windows, and applications the native recorder can capture.
pub fn capture_sources() -> Result<Vec<CaptureSource>, String> {
    platform::capture_sources()
}

/// Cameras and microphones the native recorder can open.
pub fn devices() -> Result<(Vec<CameraDevice>, Vec<AudioDevice>), String> {
    platform::devices()
}

/// Current OS privacy state for screen, camera, and microphone capture.
pub fn permissions() -> PermissionStatus {
    platform::permissions()
}

/// Prompt for the selected permissions and return the resulting state.
pub fn request_permissions(screen: bool, camera: bool, microphone: bool) -> PermissionStatus {
    platform::request_permissions(screen, camera, microphone)
}

/// Whether global pointer telemetry is permitted, optionally prompting.
pub fn mouse_permission(request: bool) -> bool {
    platform::mouse_permission(request)
}

/// Route native segment commits and runtime errors for one session into the
/// project journal and diagnostics. Install before [`NativeCaptureSession::start`].
pub(crate) fn install_callback_targets(
    diagnostics: Arc<SessionDiagnostics>,
    state_machine: Arc<SessionStateMachine>,
    epoch: SessionEpoch,
    journal: Option<Arc<ProjectJournal>>,
    project_root: Option<PathBuf>,
) {
    platform::install_callback_targets(diagnostics, state_machine, epoch, journal, project_root)
}

/// Move the session clock used by native callbacks once recording has begun.
pub(crate) fn set_callback_epoch(epoch: SessionEpoch) {
    platform::set_callback_epoch(epoch)
}

/// Drain the per-track publication writers after the native session stopped.
pub(crate) fn take_native_segment_writers() -> Vec<TrackSegmentWriter> {
    platform::take_native_segment_writers()
}

/// Clear the callback targets. Safe to call from any thread.
pub(crate) fn clear_callback_targets() {
    platform::clear_callback_targets()
}
