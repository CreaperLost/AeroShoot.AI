use crate::capture::{
    AudioDevice, CameraDevice, CaptureSource, CaptureSourceType, FitMode, PermissionState,
    PermissionStatus, SourceGeometry,
};
use crate::fixtures::{generate_valid_fmp4_segment, generate_valid_wav_segment};
use crate::hud::{PreviewHitMode, PreviewOwner, PreviewStatus, PreviewViewport};
use crate::project::manifest::{PauseInterval, TrackDescriptor, TrackType};
use crate::project::{
    display_name_from_input, EditDocument, EditLayout, JournalRecord, ProjectBundle,
    ProjectRecoveryReport, RecoveryEngine, RetainedInterval, TrackSegmentWriter,
};
use crate::session::{
    RuntimeErrorRecord, SessionDiagnostics, SessionEpoch, SessionEvent, SessionState,
    SessionStateMachine,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

pub struct ActiveSession {
    pub session_id: String,
    pub project_name: String,
    pub epoch: SessionEpoch,
    pub project_bundle: ProjectBundle,
    pub segment_writer: Option<TrackSegmentWriter>,
    pub extra_writers: Vec<TrackSegmentWriter>,
    #[cfg(target_os = "macos")]
    pub native_session: Option<crate::capture::macos::MacCaptureSession>,
    /// Sticky native-capture obligation. Survives `native_session.take()` so a
    /// failed Stop cannot be retried as a non-native success.
    #[cfg(target_os = "macos")]
    pub native_outcome: NativeCaptureOutcome,
    pub pause_intervals: Vec<(u64, u64)>,
    pub current_pause_start_us: Option<u64>,
    pub started_at_us: i64,
    pub initial_layout: EditLayout,
    /// Native pause finalized containers but Rust has not journaled
    /// `PauseStarted` / entered Paused. Resume must not no-op as live capture.
    pub native_pause_unacked: bool,
}

/// Native capture finalization state. `Unused` is the synthetic test path.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeCaptureOutcome {
    Unused,
    Live,
    PrepareFailed { message: String },
    StopFailed { code: i32, message: String },
    StopSucceeded,
    MissingOrInvalidMedia { message: String },
}

pub struct AppState {
    pub state_machine: SessionStateMachine,
    pub command_lock: Mutex<()>,
    pub active_session: RwLock<Option<ActiveSession>>,
    pub last_stop_result: RwLock<Option<StopRecordingResult>>,
    pub project_base_dir: PathBuf,
    /// Record-scene preview surface in the main window.
    pub studio_preview: Mutex<PreviewOwner>,
    pub permission_override: RwLock<Option<PermissionStatus>>,
    pub native_capture_enabled: bool,
    /// Diagnostics bag for the active session: most-recent runtime error
    /// from the native capture pipeline, gaps_total counter, and
    /// anything else `get_session_status` should expose to the UI.
    pub diagnostics: Arc<SessionDiagnostics>,
}

impl AppState {
    pub fn new(project_base_dir: PathBuf) -> Self {
        Self {
            state_machine: SessionStateMachine::new(),
            command_lock: Mutex::new(()),
            active_session: RwLock::new(None),
            last_stop_result: RwLock::new(None),
            project_base_dir,
            studio_preview: Mutex::new(PreviewOwner::new()),
            permission_override: RwLock::new(None),
            native_capture_enabled: cfg!(target_os = "macos"),
            diagnostics: Arc::new(SessionDiagnostics::new()),
        }
    }

    pub fn new_test(project_base_dir: PathBuf) -> Self {
        Self {
            state_machine: SessionStateMachine::new(),
            command_lock: Mutex::new(()),
            active_session: RwLock::new(None),
            last_stop_result: RwLock::new(None),
            project_base_dir,
            studio_preview: Mutex::new(PreviewOwner::new()),
            permission_override: RwLock::new(Some(PermissionStatus {
                screen_recording: PermissionState::Authorized,
                camera: PermissionState::Authorized,
                microphone: PermissionState::Authorized,
            })),
            native_capture_enabled: false,
            diagnostics: Arc::new(SessionDiagnostics::new()),
        }
    }
}

/// Returns the cross-platform default storage directory for AeroShoot recordings and projects:
/// `Documents/AeroShootRec/` on macOS, Windows, and Linux.
pub fn default_projects_dir() -> PathBuf {
    let docs_dir = dirs::document_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Documents")))
        .unwrap_or_else(std::env::temp_dir);
    docs_dir.join("AeroShootRec")
}

pub fn get_default_projects_dir_impl() -> String {
    default_projects_dir().to_string_lossy().into_owned()
}

fn looks_like_project_bundle(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "aero") || path.join("manifest.json").is_file()
}

/// Parent folder for a new `.aero` bundle. User-supplied paths must already exist.
pub fn resolve_project_parent(requested: Option<&str>, default: &Path) -> Result<PathBuf, String> {
    let supplied = requested.map(str::trim).filter(|value| !value.is_empty());
    let parent = match supplied {
        Some(value) => PathBuf::from(value),
        None => default.to_path_buf(),
    };
    if parent.as_os_str().as_encoded_bytes().contains(&0) {
        return Err("Project location is invalid".into());
    }
    if !parent.is_absolute() {
        return Err("Project location must be an absolute folder".into());
    }
    if parent
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("Project location cannot contain '..'".into());
    }
    if !parent.exists() {
        if supplied.is_none() {
            fs::create_dir_all(&parent)
                .map_err(|error| format!("Failed to create project folder: {error}"))?;
        } else {
            return Err("Project location does not exist".into());
        }
    }
    let meta = fs::symlink_metadata(&parent).map_err(|error| error.to_string())?;
    if meta.file_type().is_symlink() {
        return Err("Project location cannot be a symbolic link".into());
    }
    if !meta.is_dir() {
        return Err("Project location must be a folder".into());
    }
    if looks_like_project_bundle(&parent) {
        return Err("Choose a folder, not an existing .aero project".into());
    }
    Ok(parent)
}

impl Default for AppState {
    fn default() -> Self {
        let base_dir = default_projects_dir();
        let _ = fs::create_dir_all(&base_dir);
        Self::new(base_dir)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StartRecordingOptions {
    pub source_id: String,
    #[serde(default = "default_true")]
    pub capture_screen: bool,
    #[serde(default)]
    pub camera_id: Option<String>,
    #[serde(default)]
    pub mic_id: Option<String>,
    #[serde(default)]
    pub capture_system_audio: bool,
    pub fps: u32,
    pub resolution: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<EditLayout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_dir: Option<String>,
    /// Microphone gain in decibels applied to the mic track before it is
    /// written to the WAV segment. `None` and `0.0` both mean unity gain
    /// (no change). Range is clamped on the native side to a sane window
    /// (typically ±24 dB) to avoid runaway amplification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mic_gain_db: Option<f32>,
    /// Average bitrate for the screen video track in bits per second. `None`
    /// keeps the automatic rate derived from resolution and frame rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_bitrate_bps: Option<u32>,
    /// Log pointer motion and clicks beside the screen track. On by default.
    #[serde(default = "default_true")]
    pub capture_mouse: bool,
    /// Countdown in milliseconds before recording begins (at most 10 s).
    /// Every source starts at once and warms up during it.
    #[serde(default)]
    pub start_delay_ms: u32,
}

fn default_true() -> bool {
    true
}

/// Bounds for a user-selected screen bitrate. The recorder offers 10, 20 and
/// 30 Mbps; anything outside that range is clamped.
const MIN_VIDEO_BITRATE_BPS: u32 = 10_000_000;
const MAX_VIDEO_BITRATE_BPS: u32 = 30_000_000;

/// Pointer telemetry needs a source with trackable geometry (a display or a
/// window) and Input Monitoring permission.
fn mouse_tracking_available(source_id: &str) -> bool {
    let trackable = source_id.starts_with("display:") || source_id.starts_with("window:");
    #[cfg(target_os = "macos")]
    {
        trackable && crate::capture::macos::mouse_permission(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = trackable;
        false
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StartRecordingResult {
    pub session_id: String,
    pub state: SessionState,
    pub started_at_us: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionStateResult {
    pub state: SessionState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusResult {
    pub state: SessionState,
    pub elapsed_us: u64,
    pub dropped_frames: u64,
    pub audio_buffer_underflows: u64,
    /// Most recent runtime error reported by the native capture pipeline
    /// (e.g. SCStream failure, AVAssetWriter error). `None` when the
    /// session is clean.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_runtime_error: Option<RuntimeErrorRecord>,
    /// First non-recoverable native error. Unlike `last_runtime_error`, this
    /// remains stable so a short-lived later event cannot hide the root cause.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_terminal_error: Option<RuntimeErrorRecord>,
    /// Aggregate number of gaps / discontinuities observed across every
    /// track in the active session. Mirrors the journal's `Discontinuity`
    /// records so the UI can show "N gaps" without re-scanning disk.
    #[serde(default)]
    pub gaps_total: u64,
    #[serde(default)]
    pub timestamp_records_dropped: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    #[serde(default)]
    pub screen_samples: u64,
    #[serde(default)]
    pub camera_samples: u64,
    #[serde(default)]
    pub system_audio_samples: u64,
    #[serde(default)]
    pub mic_samples: u64,
    pub system_audio_peak_db: Option<f64>,
    pub mic_peak_db: Option<f64>,
    pub screen_last_sample_age_ms: Option<u64>,
    pub camera_last_sample_age_ms: Option<u64>,
    pub system_audio_last_sample_age_ms: Option<u64>,
    pub mic_last_sample_age_ms: Option<u64>,
    pub screen_segments: u64,
    pub camera_segments: u64,
    pub system_audio_segments: u64,
    pub mic_segments: u64,
}

/// Result envelope for the new `compute_source_geometry` Tauri command.
/// Mirrors the active capture source plus the geometry math applied.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceGeometryResult {
    pub source: CaptureSource,
    pub geometry: SourceGeometry,
    pub dest_width: u32,
    pub dest_height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StopRecordingResult {
    pub project_path: String,
    pub session_id: String,
    pub state: SessionState,
    pub duration_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DevicesResult {
    pub cameras: Vec<CameraDevice>,
    pub mics: Vec<AudioDevice>,
}

pub fn list_capture_sources_impl() -> Vec<CaptureSource> {
    #[cfg(target_os = "macos")]
    if let Ok(sources) = crate::capture::macos::capture_sources() {
        if !sources.is_empty() {
            return sources;
        }
    }

    #[cfg(any(not(target_os = "macos"), test))]
    return vec![
        CaptureSource {
            id: "screen-main".into(),
            name: "Main Display (Apple Silicon / Retina)".into(),
            source_type: CaptureSourceType::Display,
            width: 2560,
            height: 1440,
        },
        CaptureSource {
            id: "screen-secondary".into(),
            name: "Secondary Liquid Retina Display".into(),
            source_type: CaptureSourceType::Display,
            width: 1920,
            height: 1080,
        },
        CaptureSource {
            id: "win-active".into(),
            name: "Active Code Workspace".into(),
            source_type: CaptureSourceType::Window,
            width: 1920,
            height: 1080,
        },
    ];

    #[cfg(not(any(not(target_os = "macos"), test)))]
    Vec::new()
}

pub fn list_devices_impl() -> DevicesResult {
    #[cfg(target_os = "macos")]
    if let Ok((cameras, mics)) = crate::capture::macos::devices() {
        if !cameras.is_empty() || !mics.is_empty() {
            return DevicesResult { cameras, mics };
        }
    }

    #[cfg(any(not(target_os = "macos"), test))]
    return DevicesResult {
        cameras: vec![CameraDevice {
            id: "cam-facetime".into(),
            name: "FaceTime HD Camera (Built-in)".into(),
            is_default: true,
        }],
        mics: vec![
            AudioDevice {
                id: "mic-macbook".into(),
                name: "MacBook Pro Microphone".into(),
                is_default: true,
            },
            AudioDevice {
                id: "mic-studio".into(),
                name: "Studio Display Microphone Array".into(),
                is_default: false,
            },
        ],
    };

    #[cfg(not(any(not(target_os = "macos"), test)))]
    DevicesResult {
        cameras: Vec::new(),
        mics: Vec::new(),
    }
}

pub fn request_permissions_impl(screen: bool, camera: bool, microphone: bool) -> PermissionStatus {
    #[cfg(target_os = "macos")]
    return crate::capture::macos::request_permissions(screen, camera, microphone);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (screen, camera, microphone);
        crate::capture::check_system_permissions()
    }
}

pub fn get_permission_status_impl(state: &AppState) -> PermissionStatus {
    if let Some(status) = state.permission_override.read().clone() {
        return status;
    }
    crate::capture::check_system_permissions()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSettingsResult {
    pub opened: bool,
}

pub fn open_system_privacy_settings_impl(pane: Option<String>) -> OpenSettingsResult {
    #[cfg(target_os = "macos")]
    {
        let url = match pane.as_deref() {
            Some("ScreenCapture") => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
            Some("Camera") => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera"
            }
            Some("Microphone") => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
            }
            Some("Accessibility") => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            Some("InputMonitoring") | Some("ListenEvent") => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
            }
            _ => "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
        };
        let status = std::process::Command::new("open").arg(url).status();
        OpenSettingsResult {
            opened: status.map(|s| s.success()).unwrap_or(false),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pane;
        OpenSettingsResult { opened: false }
    }
}

pub fn show_in_finder_impl(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("Path does not exist: {}", path));
    }
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .status()
            .map_err(|e| format!("Failed to run open -R: {e}"))?;
        if !status.success() {
            return Err(format!("open -R failed with exit status: {status}"));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(target_os = "windows")]
        {
            let status = std::process::Command::new("explorer")
                .arg(format!("/select,{}", path))
                .status()
                .map_err(|e| format!("Failed to run explorer: {e}"))?;
            if !status.success() {
                return Err(format!("explorer failed with exit status: {status}"));
            }
            Ok(())
        }
        #[cfg(target_os = "linux")]
        {
            let target = if p.is_dir() {
                p
            } else {
                p.parent().unwrap_or(p)
            };
            let status = std::process::Command::new("xdg-open")
                .arg(target)
                .status()
                .map_err(|e| format!("Failed to run xdg-open: {e}"))?;
            if !status.success() {
                return Err(format!("xdg-open failed with exit status: {status}"));
            }
            Ok(())
        }
        #[cfg(all(
            not(target_os = "macos"),
            not(target_os = "windows"),
            not(target_os = "linux")
        ))]
        Ok(())
    }
}

pub fn start_recording_impl(
    state: &AppState,
    options: StartRecordingOptions,
) -> Result<StartRecordingResult, String> {
    // 1. Serialize all lifecycle commands
    let _cmd_guard = state.command_lock.lock();

    // Native start stops the preview capture itself, taking over its running
    // camera and microphone so they do not restart. The studio preview view
    // stays attached: the recording session feeds the same mailbox. Never
    // touch the AppKit view from here — this runs on a blocking worker, and the
    // Swift adapter's `DispatchQueue.main.sync` while holding the
    // `studio_preview` lock deadlocks against main-thread layout/detach commands.

    // 2. Check system permissions for required screen recording. Camera and
    // microphone are optional tracks: if those TCC grants are missing, drop
    // them and still record the screen instead of failing the whole session.
    let permissions = get_permission_status_impl(state);
    if (options.capture_screen || options.capture_system_audio)
        && !permissions.screen_recording.is_authorized()
    {
        let _ = state.state_machine.transition_to(SessionState::Error);
        return Err(
            "Screen recording permission denied. Please grant permission in macOS System Settings."
                .into(),
        );
    }
    let mut options = options;
    if !permissions.camera.is_authorized() {
        options.camera_id = None;
    }
    if !permissions.microphone.is_authorized() {
        options.mic_id = None;
    }
    if !options.capture_screen
        && options.camera_id.is_none()
        && options.mic_id.is_none()
        && !options.capture_system_audio
    {
        return Err("Select at least one media source before recording".into());
    }
    let initial_layout = options.layout.clone().unwrap_or_default();
    initial_layout.validate()?;

    // 3. Retry/Idempotency check: if already recording or preparing, return active session
    if state.state_machine.is_recording()
        || state.state_machine.current() == SessionState::Preparing
    {
        if let Some(session) = state.active_session.read().as_ref() {
            return Ok(StartRecordingResult {
                session_id: session.session_id.clone(),
                state: SessionState::Recording,
                started_at_us: session.started_at_us,
                project_path: Some(
                    session
                        .project_bundle
                        .root_path()
                        .to_string_lossy()
                        .into_owned(),
                ),
            });
        }
    }

    if state.active_session.read().is_some() {
        return Err(
            "A recoverable session still owns the project; Stop it before starting another recording"
                .into(),
        );
    }

    // 3. Reset from Completed, Error, or Idle into Preparing
    state
        .state_machine
        .transition_to(SessionState::Preparing)
        .map_err(|e| e.to_string())?;

    // Reset diagnostics so a fresh session does not inherit the previous
    // session's runtime errors. The native callback targets are wired
    // later, but we want the UI to see a clean baseline from the moment
    // `start_recording` returns.
    state.diagnostics.reset();

    let session_id = uuid::Uuid::new_v4().to_string();
    // The countdown includes the time spent preparing the project below.
    const MAX_START_DELAY_MS: u32 = 10_000;
    let recording_due = std::time::Instant::now()
        + std::time::Duration::from_millis(options.start_delay_ms.min(MAX_START_DELAY_MS).into());
    // Moved to the moment native capture begins recording, after the countdown.
    let mut epoch = SessionEpoch::now();
    let mut started_at_us = epoch.start_wall_time_us();

    let explicit_name = options
        .project_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let display_name = display_name_from_input(explicit_name.unwrap_or(""));
    let parent = resolve_project_parent(options.project_dir.as_deref(), &state.project_base_dir)?;
    let disambiguate = explicit_name.is_none();

    let mut bundle = ProjectBundle::create_named(&parent, &session_id, &display_name, disambiguate)
        .map_err(|e| {
            let _ = state.state_machine.transition_to(SessionState::Error);
            format!("Failed to create project bundle: {}", e)
        })?;

    // Parse recording options
    let (width, height) = match options.resolution.as_str() {
        "4k" | "4K" => (3840, 2160),
        "1440p" => (2560, 1440),
        "720p" => (1280, 720),
        _ => (1920, 1080),
    };
    let fps = if options.fps > 0 { options.fps } else { 30 };
    let video_bitrate_bps = options
        .video_bitrate_bps
        .map(|bps| bps.clamp(MIN_VIDEO_BITRATE_BPS, MAX_VIDEO_BITRATE_BPS));

    // 1. Primary screen track
    if options.capture_screen {
        bundle.manifest_mut().tracks.push(TrackDescriptor {
            id: "screen".into(),
            track_type: TrackType::Screen,
            codec: "h264".into(),
            relative_path: "media/screen/000001.mp4".into(),
            width: Some(width),
            height: Some(height),
            fps: Some(fps),
            sample_rate: None,
            channels: None,
            gaps_total: 0,
            media_timescale: None,
        });
    }

    // 2. Conditional webcam track
    if options.camera_id.is_some() {
        bundle.manifest_mut().tracks.push(TrackDescriptor {
            id: "webcam".into(),
            track_type: TrackType::Webcam,
            codec: "h264".into(),
            relative_path: "media/webcam/000001.mp4".into(),
            width: Some(1280),
            height: Some(720),
            fps: Some(fps),
            sample_rate: None,
            channels: None,
            gaps_total: 0,
            media_timescale: None,
        });
    }

    // 3. Conditional mic audio track
    if options.mic_id.is_some() {
        bundle.manifest_mut().tracks.push(TrackDescriptor {
            id: "mic".into(),
            track_type: TrackType::MicAudio,
            codec: "pcm".into(),
            relative_path: "media/mic/000001.wav".into(),
            width: None,
            height: None,
            fps: None,
            sample_rate: Some(48000),
            channels: Some(1),
            gaps_total: 0,
            media_timescale: None,
        });
    }

    // 4. Conditional system audio track
    if options.capture_system_audio {
        bundle.manifest_mut().tracks.push(TrackDescriptor {
            id: "system".into(),
            track_type: TrackType::SystemAudio,
            codec: "pcm".into(),
            relative_path: "media/system/000001.wav".into(),
            width: None,
            height: None,
            fps: None,
            sample_rate: Some(48000),
            channels: Some(2),
            gaps_total: 0,
            media_timescale: None,
        });
    }

    // Compute source geometry before saving initial manifest so crash recovery
    // and downstream tools can re-derive the active rect.
    #[cfg(all(target_os = "macos", not(test)))]
    let source_list = crate::capture::macos::capture_sources().unwrap_or_default();
    #[cfg(any(not(target_os = "macos"), test))]
    let source_list = list_capture_sources_impl();

    let resolved_source = source_list
        .iter()
        .find(|s| s.id == options.source_id)
        .cloned()
        .unwrap_or_else(|| CaptureSource {
            id: options.source_id.clone(),
            name: format!("Capture Source ({})", options.source_id),
            source_type: CaptureSourceType::Display,
            width,
            height,
        });
    let geometry =
        crate::capture::compute_source_geometry(&resolved_source, width, height, FitMode::Fit);
    bundle.manifest_mut().source_geometry = options.capture_screen.then_some(geometry);
    // Hide the cursor from the screen video only when pointer tracking can
    // record it, so the editor redraws it (`replace`). Otherwise the cursor is
    // baked into the video and the editor must not draw a second one.
    let hide_cursor = state.native_capture_enabled
        && options.capture_screen
        && options.capture_mouse
        && mouse_tracking_available(&options.source_id);
    bundle.manifest_mut().cursor_mode = options.capture_screen.then(|| {
        if hide_cursor {
            crate::telemetry::native::CURSOR_MODE_REPLACE.into()
        } else {
            crate::telemetry::native::CURSOR_MODE_BAKED.into()
        }
    });

    let manifest_path = bundle.root_path().join("manifest.json");
    bundle
        .manifest_mut()
        .save_with_backup(&manifest_path)
        .map_err(|e| {
            let _ = state.state_machine.transition_to(SessionState::Error);
            format!("Failed saving initial manifest: {}", e)
        })?;

    let mut segment_writer = None;
    let mut extra_writers = Vec::new();
    #[cfg(target_os = "macos")]
    let mut native_session = None;

    if state.native_capture_enabled {
        #[cfg(target_os = "macos")]
        {
            // Wire the global C FFI callback targets before Swift
            // starts producing callbacks. Both targets share the same
            // diagnostics bag; the runtime-error target also gets a
            // reference to the state machine for Failed transitions.
            let sm_arc = Arc::new(state.state_machine.clone());
            let journal_arc = Some(bundle.journal_arc());
            crate::capture::macos::install_callback_targets(
                state.diagnostics.clone(),
                sm_arc,
                epoch.clone(),
                journal_arc,
                Some(bundle.root_path().to_path_buf()),
            );

            match crate::capture::macos::MacCaptureSession::start(
                crate::capture::macos::NativeRecordingConfig {
                    source_id: &options.source_id,
                    capture_screen: options.capture_screen,
                    camera_id: options.camera_id.as_deref(),
                    mic_id: options.mic_id.as_deref(),
                    capture_system_audio: options.capture_system_audio,
                    fps,
                    width,
                    height,
                    source_rect: geometry.source_rect,
                    destination_rect: geometry.dest_rect,
                    preserves_aspect_ratio: geometry.preserves_aspect_ratio,
                    project_path: bundle.root_path(),
                    // Native session time zero is when recording begins.
                    session_offset_us: 0,
                    mic_gain_db: options.mic_gain_db,
                    video_bitrate_bps,
                    capture_mouse: options.capture_mouse,
                    hide_cursor,
                    start_delay_ms: u32::try_from(
                        recording_due
                            .saturating_duration_since(std::time::Instant::now())
                            .as_millis(),
                    )
                    .unwrap_or(u32::MAX),
                },
            ) {
                Ok(session) => {
                    let ready = session.wait_until_ready(
                        options.capture_screen,
                        options.camera_id.is_some(),
                        options.mic_id.is_some(),
                        std::time::Duration::from_secs(5),
                    );
                    if let Err(error) = ready {
                        drop(session);
                        crate::capture::macos::clear_callback_targets();
                        let failed = ActiveSession {
                            session_id: session_id.clone(),
                            project_name: display_name,
                            epoch,
                            project_bundle: bundle,
                            segment_writer: None,
                            extra_writers: Vec::new(),
                            native_session: None,
                            native_outcome: NativeCaptureOutcome::PrepareFailed {
                                message: error.clone(),
                            },
                            pause_intervals: Vec::new(),
                            current_pause_start_us: None,
                            started_at_us,
                            initial_layout: initial_layout.clone(),
                            native_pause_unacked: false,
                        };
                        return Err(retain_failed_session(
                            state,
                            failed,
                            "session",
                            LIFECYCLE_PREPARE,
                            format!("Native capture did not become ready: {error}"),
                        ));
                    }
                    // Session time zero is when native capture began
                    // recording, after the countdown: move the session clock there.
                    if let Some(ago_us) = session.stats().recording_started_ago_us {
                        epoch = SessionEpoch::started_ago(std::time::Duration::from_micros(ago_us));
                        started_at_us = epoch.start_wall_time_us();
                        crate::capture::macos::set_callback_epoch(epoch.clone());
                    }
                    native_session = Some(session);
                }
                Err(error) => {
                    crate::capture::macos::clear_callback_targets();
                    let failed = ActiveSession {
                        session_id: session_id.clone(),
                        project_name: display_name,
                        epoch,
                        project_bundle: bundle,
                        segment_writer: None,
                        extra_writers: Vec::new(),
                        native_session: None,
                        native_outcome: NativeCaptureOutcome::PrepareFailed {
                            message: error.clone(),
                        },
                        pause_intervals: Vec::new(),
                        current_pause_start_us: None,
                        started_at_us,
                        initial_layout: initial_layout.clone(),
                        native_pause_unacked: false,
                    };
                    return Err(retain_failed_session(
                        state,
                        failed,
                        "session",
                        LIFECYCLE_PREPARE,
                        format!("Failed to start native macOS capture: {error}"),
                    ));
                }
            }
        }
    } else {
        if options.capture_screen {
            let mut writer = bundle.create_segment_writer("screen", TrackType::Screen, "h264");
            writer
                .begin_segment(0)
                .map_err(|e| format!("Failed to open segment: {e}"))?;
            writer
                .write_data(&generate_valid_fmp4_segment(0, 33_333, true))
                .map_err(|e| format!("Failed writing initial segment data: {e}"))?;
            segment_writer = Some(writer);
        }

        if options.camera_id.is_some() {
            let mut writer = bundle.create_segment_writer("webcam", TrackType::Webcam, "h264");
            writer
                .begin_segment(0)
                .map_err(|e| format!("Failed to open webcam segment: {e}"))?;
            writer
                .write_data(&generate_valid_fmp4_segment(0, 33_333, true))
                .map_err(|e| e.to_string())?;
            extra_writers.push(writer);
        }
        if options.mic_id.is_some() {
            let mut writer = bundle.create_segment_writer("mic", TrackType::MicAudio, "pcm");
            writer
                .begin_segment(0)
                .map_err(|e| format!("Failed to open mic segment: {e}"))?;
            writer
                .write_data(&generate_valid_wav_segment(33_333, 48_000, 1))
                .map_err(|e| e.to_string())?;
            extra_writers.push(writer);
        }
        if options.capture_system_audio {
            let mut writer = bundle.create_segment_writer("system", TrackType::SystemAudio, "pcm");
            writer
                .begin_segment(0)
                .map_err(|e| format!("Failed to open system segment: {e}"))?;
            writer
                .write_data(&generate_valid_wav_segment(33_333, 48_000, 2))
                .map_err(|e| e.to_string())?;
            extra_writers.push(writer);
        }
    }

    // Transition to Recording
    state
        .state_machine
        .transition_to(SessionState::Recording)
        .map_err(|e| e.to_string())?;

    let project_path = bundle.root_path().to_string_lossy().into_owned();

    #[cfg(target_os = "macos")]
    let native_outcome = if native_session.is_some() {
        NativeCaptureOutcome::Live
    } else {
        NativeCaptureOutcome::Unused
    };

    *state.active_session.write() = Some(ActiveSession {
        session_id: session_id.clone(),
        project_name: display_name,
        epoch,
        project_bundle: bundle,
        segment_writer,
        extra_writers,
        #[cfg(target_os = "macos")]
        native_session,
        #[cfg(target_os = "macos")]
        native_outcome,
        pause_intervals: Vec::new(),
        current_pause_start_us: None,
        started_at_us,
        initial_layout,
        native_pause_unacked: false,
    });

    Ok(StartRecordingResult {
        session_id,
        state: SessionState::Recording,
        started_at_us,
        project_path: Some(project_path),
    })
}

const LIFECYCLE_PREPARE: i32 = -601;
const LIFECYCLE_FINALIZE: i32 = -610;
const LIFECYCLE_JOURNAL: i32 = -611;
const LIFECYCLE_NATIVE_PAUSE: i32 = -612;

fn record_lifecycle_error(state: &AppState, track_id: &str, code: i32, message: &str, t_us: u64) {
    state.diagnostics.apply(&SessionEvent::RuntimeError {
        track_id: track_id.to_string(),
        error_code: code,
        message: message.to_string(),
        t_us,
        recoverable: true,
    });
}

fn retain_failed_session(
    state: &AppState,
    session: ActiveSession,
    track_id: &str,
    code: i32,
    message: String,
) -> String {
    let t_us = session.epoch.current_elapsed_us();
    record_lifecycle_error(state, track_id, code, &message, t_us);
    let _ = state.state_machine.transition_to(SessionState::Error);
    *state.active_session.write() = Some(session);
    message
}

#[cfg(target_os = "macos")]
fn absorb_native_segment_writers(session: &mut ActiveSession, writers: Vec<TrackSegmentWriter>) {
    for writer in writers {
        if writer.track_id() == "screen" && session.segment_writer.is_none() {
            session.segment_writer = Some(writer);
        } else {
            session.extra_writers.push(writer);
        }
    }
}

#[cfg(target_os = "macos")]
fn native_requires_honest_stop(session: &ActiveSession) -> bool {
    !matches!(session.native_outcome, NativeCaptureOutcome::Unused)
}

#[cfg(target_os = "macos")]
fn sticky_native_stop_error(session: &ActiveSession) -> Option<(i32, String)> {
    match &session.native_outcome {
        NativeCaptureOutcome::PrepareFailed { message } => Some((
            LIFECYCLE_PREPARE,
            format!(
                "Native capture never started: {message}. The project has been retained for diagnostics."
            ),
        )),
        NativeCaptureOutcome::StopFailed { code, message } => Some((
            *code,
            format!(
                "Native capture stopped with error (code {code}): {message}. \
                 Previously committed segments remain available for recovery."
            ),
        )),
        NativeCaptureOutcome::MissingOrInvalidMedia { message } => Some((
            LIFECYCLE_FINALIZE,
            format!("{message}. The project has been retained for diagnostics."),
        )),
        _ => None,
    }
}

pub fn pause_recording_impl(state: &AppState) -> Result<SessionStateResult, String> {
    let _cmd_guard = state.command_lock.lock();

    if state.state_machine.is_paused() {
        return Ok(SessionStateResult {
            state: SessionState::Paused,
        });
    }

    if !state.state_machine.is_recording() {
        return Err(format!(
            "Cannot pause from {:?}",
            state.state_machine.current()
        ));
    }

    let mut session_guard = state.active_session.write();
    let session = session_guard
        .as_mut()
        .ok_or_else(|| "No active recording session to pause".to_string())?;

    let now_us = session.epoch.current_elapsed_us();
    let final_us = now_us.max(33_333);

    // Finalize writers (and native AVAssetWriter containers) before any
    // Paused acknowledgment. `let _ = finalize(...)` is not success.
    let mut finalize_err: Option<(String, i32, String)> = None;
    if let Some(writer) = session.segment_writer.as_mut() {
        if let Err(e) = writer.finalize(final_us, session.project_bundle.journal()) {
            finalize_err = Some(("screen".into(), LIFECYCLE_FINALIZE, e.to_string()));
        }
    }
    if finalize_err.is_none() {
        for writer in &mut session.extra_writers {
            if let Err(e) = writer.finalize(final_us, session.project_bundle.journal()) {
                finalize_err = Some((
                    writer.track_id().to_string(),
                    LIFECYCLE_FINALIZE,
                    e.to_string(),
                ));
                break;
            }
        }
    }
    #[cfg(target_os = "macos")]
    if finalize_err.is_none() {
        if session.native_pause_unacked {
            // Native containers already finalized; retry only the journal ack.
        } else if let Some(native) = session.native_session.as_ref() {
            match native.pause_and_finalize() {
                Ok(()) => session.native_pause_unacked = true,
                Err((code, message)) => {
                    session.native_pause_unacked = false;
                    let code = if code == 0 {
                        LIFECYCLE_NATIVE_PAUSE
                    } else {
                        code
                    };
                    finalize_err = Some(("native".into(), code, message));
                }
            }
        }
    }

    if let Some((track_id, code, message)) = finalize_err {
        record_lifecycle_error(state, &track_id, code, &message, now_us);
        return Err(format!(
            "Pause finalization failed for {track_id}: {message}"
        ));
    }

    session
        .project_bundle
        .journal()
        .append(JournalRecord::PauseStarted {
            seq: 0,
            t_us: now_us,
        })
        .map_err(|e| {
            let msg = format!("Failed to append PauseStarted to journal: {e}");
            record_lifecycle_error(state, "session", LIFECYCLE_JOURNAL, &msg, now_us);
            msg
        })?;

    session.native_pause_unacked = false;
    session.current_pause_start_us = Some(now_us);
    drop(session_guard);

    state
        .state_machine
        .transition_to(SessionState::Paused)
        .map_err(|e| e.to_string())?;

    Ok(SessionStateResult {
        state: SessionState::Paused,
    })
}

pub fn resume_recording_impl(state: &AppState) -> Result<SessionStateResult, String> {
    let _cmd_guard = state.command_lock.lock();

    if state.state_machine.is_recording() {
        let unacked = state
            .active_session
            .read()
            .as_ref()
            .map(|s| s.native_pause_unacked)
            .unwrap_or(false);
        if unacked {
            return Err("Pause finalization failed; retry Pause or Stop".into());
        }
        return Ok(SessionStateResult {
            state: SessionState::Recording,
        });
    }

    if !state.state_machine.is_paused() {
        return Err(format!(
            "Cannot resume from {:?}",
            state.state_machine.current()
        ));
    }

    let mut session_guard = state.active_session.write();
    let session = session_guard
        .as_mut()
        .ok_or_else(|| "No active recording session to resume".to_string())?;
    let now_us = session.epoch.current_elapsed_us();
    if let Some(pause_start) = session.current_pause_start_us {
        session
            .project_bundle
            .journal()
            .append(JournalRecord::PauseEnded {
                seq: 0,
                start_us: pause_start,
                end_us: now_us,
            })
            .map_err(|e| {
                let msg = format!("Failed to append PauseEnded to journal: {e}");
                record_lifecycle_error(state, "session", LIFECYCLE_JOURNAL, &msg, now_us);
                msg
            })?;
        session.current_pause_start_us.take();
        session.pause_intervals.push((pause_start, now_us));
    }

    // Resume must open a fresh segment interval, never append to a closed file.
    if let Some(writer) = session.segment_writer.as_mut() {
        writer
            .begin_segment(now_us)
            .map_err(|e| format!("Failed to open screen segment on resume: {e}"))?;
        let fmp4_data = generate_valid_fmp4_segment(now_us, 33_333, true);
        writer
            .write_data(&fmp4_data)
            .map_err(|e| format!("Failed writing screen segment on resume: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    if let Some(native) = session.native_session.as_ref() {
        native.set_paused(false);
        session.native_pause_unacked = false;
    }

    for writer in &mut session.extra_writers {
        writer.begin_segment(now_us).map_err(|e| {
            format!(
                "Failed to open {} segment on resume: {e}",
                writer.track_id()
            )
        })?;
        if writer.track_type() == TrackType::Webcam {
            let d = generate_valid_fmp4_segment(now_us, 33_333, true);
            writer
                .write_data(&d)
                .map_err(|e| format!("Failed writing webcam segment on resume: {e}"))?;
        } else {
            let d = generate_valid_wav_segment(33_333, 48_000, 2);
            writer
                .write_data(&d)
                .map_err(|e| format!("Failed writing audio segment on resume: {e}"))?;
        }
    }
    drop(session_guard);

    state
        .state_machine
        .transition_to(SessionState::Recording)
        .map_err(|e| e.to_string())?;

    Ok(SessionStateResult {
        state: SessionState::Recording,
    })
}

pub fn stop_recording_impl(state: &AppState) -> Result<StopRecordingResult, String> {
    let _cmd_guard = state.command_lock.lock();

    // Idempotent retry: if already completed, return cached result
    if state.state_machine.current() == SessionState::Completed {
        if let Some(res) = state.last_stop_result.read().as_ref() {
            return Ok(res.clone());
        }
    }

    state
        .state_machine
        .transition_to(SessionState::Stopping)
        .map_err(|e| e.to_string())?;

    let session_opt = state.active_session.write().take();
    let mut session = match session_opt {
        Some(s) => s,
        None => {
            let _ = state.state_machine.transition_to(SessionState::Error);
            return Err("No active recording session to stop".to_string());
        }
    };

    let gross_duration_us = session.epoch.current_elapsed_us();

    #[cfg(target_os = "macos")]
    {
        if let Some(native) = session.native_session.take() {
            match native.stop_with_result() {
                Ok(()) => session.native_outcome = NativeCaptureOutcome::StopSucceeded,
                Err((code, message)) => {
                    session.native_outcome = NativeCaptureOutcome::StopFailed { code, message };
                }
            }
        }
        absorb_native_segment_writers(
            &mut session,
            crate::capture::macos::take_native_segment_writers(),
        );
        crate::capture::macos::clear_callback_targets();
    }

    // Close any in-flight pause
    if let Some(pause_start) = session.current_pause_start_us.take() {
        session
            .pause_intervals
            .push((pause_start, gross_duration_us));
        if let Err(e) = session
            .project_bundle
            .journal()
            .append(JournalRecord::PauseEnded {
                seq: 0,
                start_us: pause_start,
                end_us: gross_duration_us,
            })
        {
            let _ = state.state_machine.transition_to(SessionState::Error);
            *state.active_session.write() = Some(session);
            return Err(format!(
                "Storage error writing pause interval to journal: {}",
                e
            ));
        }
    }

    // Non-reversed timestamps: final duration must be strictly positive (at least 1 frame)
    let final_segment_end_us = gross_duration_us.max(33_333);

    // Finalize screen segment writer (flush -> sync -> rename -> journal append)
    if let Some(writer) = session.segment_writer.as_mut() {
        if let Err(e) = writer.finalize(final_segment_end_us, session.project_bundle.journal()) {
            let msg = format!("Storage finalization failed for screen track: {e}");
            record_lifecycle_error(state, "screen", LIFECYCLE_FINALIZE, &msg, gross_duration_us);
            let _ = state.state_machine.transition_to(SessionState::Error);
            *state.active_session.write() = Some(session);
            return Err(msg);
        }
    }

    // Finalize extra writers
    let mut extra_writer_err = None;
    for writer in &mut session.extra_writers {
        if let Err(e) = writer.finalize(final_segment_end_us, session.project_bundle.journal()) {
            extra_writer_err = Some((writer.track_id().to_string(), e));
            break;
        }
    }
    if let Some((track_id, e)) = extra_writer_err {
        let msg = format!("Storage finalization failed for track {track_id}: {e}");
        record_lifecycle_error(
            state,
            &track_id,
            LIFECYCLE_FINALIZE,
            &msg,
            gross_duration_us,
        );
        let _ = state.state_machine.transition_to(SessionState::Error);
        *state.active_session.write() = Some(session);
        return Err(msg);
    }

    #[cfg(target_os = "macos")]
    if let Some((code, msg)) = sticky_native_stop_error(&session) {
        record_lifecycle_error(state, "native", code, &msg, gross_duration_us);
        let _ = state.state_machine.transition_to(SessionState::Error);
        *state.active_session.write() = Some(session);
        return Err(msg);
    }

    #[cfg(target_os = "macos")]
    if native_requires_honest_stop(&session) {
        let records = match session.project_bundle.journal().read_all() {
            Ok(records) => records,
            Err(e) => {
                let msg = e.to_string();
                record_lifecycle_error(
                    state,
                    "session",
                    LIFECYCLE_JOURNAL,
                    &msg,
                    gross_duration_us,
                );
                let _ = state.state_machine.transition_to(SessionState::Error);
                *state.active_session.write() = Some(session);
                return Err(msg);
            }
        };
        let expected_tracks: Vec<String> = session
            .project_bundle
            .manifest()
            .tracks
            .iter()
            .map(|track| track.id.clone())
            .collect();
        let mut committed_tracks = std::collections::HashSet::new();
        let mut oversized_segments = Vec::new();
        for record in &records {
            if let JournalRecord::SegmentCommitted {
                track_id,
                relative_path,
                start_us,
                end_us,
                size_bytes,
                ..
            } = record
            {
                if *size_bytes > 0 {
                    committed_tracks.insert(track_id.as_str());
                }
                if end_us.saturating_sub(*start_us) > 60_000_000 {
                    oversized_segments.push(relative_path.as_str());
                }
            }
        }
        let missing_tracks: Vec<&str> = expected_tracks
            .iter()
            .map(String::as_str)
            .filter(|track_id| !committed_tracks.contains(track_id))
            .collect();
        if !missing_tracks.is_empty() || !oversized_segments.is_empty() {
            let mut problems = Vec::new();
            if !missing_tracks.is_empty() {
                problems.push(format!(
                    "No valid committed media was saved for {}",
                    missing_tracks.join(", ")
                ));
            }
            if !oversized_segments.is_empty() {
                problems.push(format!(
                    "Segments exceeded the 60-second limit: {}",
                    oversized_segments.join(", ")
                ));
            }
            let msg = problems.join("; ");
            session.native_outcome = NativeCaptureOutcome::MissingOrInvalidMedia {
                message: msg.clone(),
            };
            record_lifecycle_error(
                state,
                "session",
                LIFECYCLE_FINALIZE,
                &msg,
                gross_duration_us,
            );
            let _ = state.state_machine.transition_to(SessionState::Error);
            *state.active_session.write() = Some(session);
            return Err(format!(
                "{msg}. The project has been retained for diagnostics."
            ));
        }
    }

    // Calculate net duration excluding pauses
    let total_paused_us: u64 = session
        .pause_intervals
        .iter()
        .map(|(s, e)| e.saturating_sub(*s))
        .sum();
    let net_duration_us = gross_duration_us.saturating_sub(total_paused_us);

    // Update manifest with final durations and pause intervals
    let manifest_path = session.project_bundle.root_path().join("manifest.json");
    session.project_bundle.manifest_mut().duration_us = gross_duration_us;
    session.project_bundle.manifest_mut().active_duration_us = net_duration_us;
    session.project_bundle.manifest_mut().pause_intervals = session
        .pause_intervals
        .iter()
        .map(|(s, e)| PauseInterval {
            start_us: *s,
            end_us: *e,
        })
        .collect();

    let final_records = match session.project_bundle.journal().read_all() {
        Ok(records) => records,
        Err(error) => {
            let msg = format!("Storage error reading final journal: {error}");
            record_lifecycle_error(state, "session", LIFECYCLE_JOURNAL, &msg, gross_duration_us);
            let _ = state.state_machine.transition_to(SessionState::Error);
            *state.active_session.write() = Some(session);
            return Err(msg);
        }
    };
    let mut gaps_by_track = std::collections::HashMap::<String, u64>::new();
    for record in &final_records {
        if let JournalRecord::Discontinuity { track_id, .. } = record {
            let count = gaps_by_track.entry(track_id.clone()).or_default();
            *count = count.saturating_add(1);
        }
    }
    for track in &mut session.project_bundle.manifest_mut().tracks {
        track.gaps_total = gaps_by_track.get(&track.id).copied().unwrap_or(0);
    }
    session.project_bundle.manifest_mut().gaps_total = gaps_by_track.values().copied().sum();

    if let Err(e) = session
        .project_bundle
        .manifest()
        .save_with_backup(&manifest_path)
    {
        let _ = state.state_machine.transition_to(SessionState::Error);
        *state.active_session.write() = Some(session);
        return Err(format!("Storage error saving manifest backup: {}", e));
    }

    // Telemetry is written only when mouse tracking was on for a screen recording.
    let telemetry_recorded = session
        .project_bundle
        .root_path()
        .join("telemetry/events.jsonl")
        .exists();
    let mouse_stream = if session.project_bundle.manifest().cursor_mode.is_some() && telemetry_recorded {
        match crate::telemetry::reader::read_telemetry(session.project_bundle.root_path()) {
            Ok(stream) => Some(stream),
            Err(error) => {
                let _ = state.state_machine.transition_to(SessionState::Error);
                *state.active_session.write() = Some(session);
                return Err(format!("Mouse telemetry qualification failed: {error}"));
            }
        }
    } else {
        None
    };
    let qualification = crate::project::build_capture_qualification_report(
        session.project_bundle.manifest(),
        &final_records,
        mouse_stream.as_ref(),
    );
    if let Err(error) = crate::project::save_capture_qualification_report(
        session.project_bundle.root_path(),
        &qualification,
    ) {
        let _ = state.state_machine.transition_to(SessionState::Error);
        *state.active_session.write() = Some(session);
        return Err(format!(
            "Storage error saving capture qualification report: {error}"
        ));
    }

    // Seed the non-destructive edit document with the canvas and camera layout
    // that the user saw while recording.
    let mut retained = Vec::new();
    let mut cursor = 0;
    for (pause_start, pause_end) in &session.pause_intervals {
        if cursor < *pause_start {
            retained.push(RetainedInterval {
                start_us: cursor,
                end_us: *pause_start,
            });
        }
        cursor = *pause_end;
    }
    if cursor < gross_duration_us {
        retained.push(RetainedInterval {
            start_us: cursor,
            end_us: gross_duration_us,
        });
    }
    let mut document = match EditDocument::from_retained(retained) {
        Ok(document) => document,
        Err(error) => {
            let _ = state.state_machine.transition_to(SessionState::Error);
            *state.active_session.write() = Some(session);
            return Err(format!("Failed creating initial edit layout: {error}"));
        }
    };
    document.layout = session.initial_layout.clone();
    if let Err(error) =
        crate::project::revision::save_edit_document(session.project_bundle.root_path(), &document)
    {
        let _ = state.state_machine.transition_to(SessionState::Error);
        *state.active_session.write() = Some(session);
        return Err(format!("Storage error saving initial edit layout: {error}"));
    }

    state
        .state_machine
        .transition_to(SessionState::Completed)
        .map_err(|e| e.to_string())?;

    let result = StopRecordingResult {
        project_path: session
            .project_bundle
            .root_path()
            .to_string_lossy()
            .into_owned(),
        session_id: session.session_id.clone(),
        state: SessionState::Completed,
        duration_us: net_duration_us,
    };

    *state.last_stop_result.write() = Some(result.clone());

    Ok(result)
}

pub fn get_session_status_impl(state: &AppState) -> SessionStatusResult {
    let current_state = state.state_machine.current();
    let elapsed_us =
        if current_state == SessionState::Recording || current_state == SessionState::Paused {
            if let Some(session) = state.active_session.read().as_ref() {
                let gross = session.epoch.current_elapsed_us();
                let mut paused: u64 = session
                    .pause_intervals
                    .iter()
                    .map(|(s, e)| e.saturating_sub(*s))
                    .sum();
                if let Some(p_start) = session.current_pause_start_us {
                    paused += gross.saturating_sub(p_start);
                }
                gross.saturating_sub(paused)
            } else {
                0
            }
        } else {
            0
        };

    #[cfg(target_os = "macos")]
    let (
        dropped_frames,
        audio_buffer_underflows,
        native_gaps,
        timestamp_records_dropped,
        screen_samples,
        camera_samples,
        system_audio_samples,
        mic_samples,
        system_audio_peak_db,
        mic_peak_db,
        screen_last_sample_age_ms,
        camera_last_sample_age_ms,
        system_audio_last_sample_age_ms,
        mic_last_sample_age_ms,
    ) = state
        .active_session
        .read()
        .as_ref()
        .and_then(|session| session.native_session.as_ref())
        .map(|session| {
            let stats = session.stats();
            (
                stats.dropped_frames,
                stats.audio_buffer_underflows,
                stats.gaps_total,
                stats.timestamp_records_dropped,
                stats.screen_samples,
                stats.camera_samples,
                stats.system_audio_samples,
                stats.mic_samples,
                stats.system_audio_peak_db,
                stats.mic_peak_db,
                stats.screen_last_sample_age_ms,
                stats.camera_last_sample_age_ms,
                stats.system_audio_last_sample_age_ms,
                stats.mic_last_sample_age_ms,
            )
        })
        .unwrap_or((0, 0, 0, 0, 0, 0, 0, 0, None, None, None, None, None, None));
    #[cfg(not(target_os = "macos"))]
    let (
        dropped_frames,
        audio_buffer_underflows,
        native_gaps,
        timestamp_records_dropped,
        screen_samples,
        camera_samples,
        system_audio_samples,
        mic_samples,
        system_audio_peak_db,
        mic_peak_db,
        screen_last_sample_age_ms,
        camera_last_sample_age_ms,
        system_audio_last_sample_age_ms,
        mic_last_sample_age_ms,
    ) = (0, 0, 0, 0, 0, 0, 0, 0, None, None, None, None, None, None);

    let project_path = state
        .active_session
        .read()
        .as_ref()
        .map(|s| s.project_bundle.root_path().to_string_lossy().into_owned());

    SessionStatusResult {
        state: current_state,
        elapsed_us,
        dropped_frames,
        audio_buffer_underflows,
        last_runtime_error: state.diagnostics.last_runtime_error(),
        first_terminal_error: state.diagnostics.first_terminal_error(),
        gaps_total: state.diagnostics.gaps_total().max(native_gaps),
        timestamp_records_dropped,
        project_path,
        screen_samples,
        camera_samples,
        system_audio_samples,
        mic_samples,
        system_audio_peak_db,
        mic_peak_db,
        screen_last_sample_age_ms,
        camera_last_sample_age_ms,
        system_audio_last_sample_age_ms,
        mic_last_sample_age_ms,
        screen_segments: state.diagnostics.segment_count("screen"),
        camera_segments: state.diagnostics.segment_count("webcam"),
        system_audio_segments: state.diagnostics.segment_count("system"),
        mic_segments: state.diagnostics.segment_count("mic"),
    }
}

/// Computes the active source / destination geometry for the given
/// `source_id`. The Frontend calls this to preview the active rect before
/// the user starts a recording, and the editor uses the same math to
/// re-derive the geometry revision from a manifest.
///
/// Defaults to a 1920×1080 dest with `Fit` mode; the caller can request
/// other dimensions by passing them explicitly.
pub fn compute_source_geometry_impl(
    source_id: String,
    dest_width: Option<u32>,
    dest_height: Option<u32>,
    fit_mode: Option<FitMode>,
) -> Result<SourceGeometryResult, String> {
    let sources = list_capture_sources_impl();
    let source = sources
        .into_iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| format!("Unknown capture source id: {source_id}"))?;

    let dest_width = dest_width.unwrap_or(1920);
    let dest_height = dest_height.unwrap_or(1080);
    let fit_mode = fit_mode.unwrap_or(FitMode::Fit);
    if dest_width == 0 || dest_height == 0 {
        return Err("dest_width and dest_height must be > 0".into());
    }

    let geometry =
        crate::capture::compute_source_geometry(&source, dest_width, dest_height, fit_mode);
    Ok(SourceGeometryResult {
        source,
        geometry,
        dest_width,
        dest_height,
    })
}

pub fn recover_project_impl(project_dir: PathBuf) -> Result<ProjectRecoveryReport, String> {
    RecoveryEngine::scan_and_recover(project_dir).map_err(|e| e.to_string())
}

// --- Studio preview surface -------------------------------------------------
//
// The record scene preview is a native child view of the `main` window.

pub fn studio_preview_attach_impl(
    state: &AppState,
    window_label: String,
    hit_mode: PreviewHitMode,
    native_window: Option<*mut std::ffi::c_void>,
) -> Result<PreviewStatus, String> {
    state
        .studio_preview
        .lock()
        .attach(window_label, hit_mode, native_window)
}

pub fn studio_preview_layout_impl(
    state: &AppState,
    viewport: PreviewViewport,
) -> Result<PreviewStatus, String> {
    state.studio_preview.lock().layout(viewport)
}

pub fn studio_preview_status_impl(state: &AppState) -> PreviewStatus {
    state.studio_preview.lock().status()
}

pub fn studio_preview_detach_impl(state: &AppState) -> Result<PreviewStatus, String> {
    state.studio_preview.lock().detach();
    Ok(state.studio_preview.lock().status())
}

/// Formats the window title for recording scenes or active sessions.
/// Defaults to a dated Untitled name (e.g. "Untitled 9 Sep 2026") when `project_name` is empty or omitted,
/// producing "AeroShoot — <Project Name>" (e.g. "AeroShoot — Untitled 9 Sep 2026").
pub fn window_title_for_recording(project_name: Option<&str>) -> String {
    let name = project_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(crate::project::default_project_name);
    format!("AeroShoot \u{2014} {}", name)
}

/// Formats the window title for project editing.
/// When a project is open, produces "AeroShoot — <Project Name>" (e.g. "AeroShoot — Launch Demo").
/// When no project is open (None or empty), produces "AeroShoot".
pub fn window_title_for_project(project_name: Option<&str>) -> String {
    match project_name.map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => format!("AeroShoot \u{2014} {}", name),
        None => "AeroShoot".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn mouse_tracking_requires_display_or_window_geometry() {
        // Application captures have no pointer geometry, so the cursor stays baked.
        assert!(!mouse_tracking_available("application:com.example"));
    }

    #[test]
    fn start_options_accept_optional_video_bitrate() {
        let with: StartRecordingOptions = serde_json::from_str(
            r#"{"sourceId":"display:1","fps":60,"resolution":"1440p","videoBitrateBps":20000000}"#,
        )
        .unwrap();
        assert_eq!(with.video_bitrate_bps, Some(20_000_000));
        let without: StartRecordingOptions =
            serde_json::from_str(r#"{"sourceId":"display:1","fps":30,"resolution":"1080p"}"#)
                .unwrap();
        assert_eq!(without.video_bitrate_bps, None);
        assert!(without.capture_mouse, "mouse tracking defaults to on");
    }

    #[test]
    fn test_ipc_serialization_roundtrip_camel_case() {
        let opt = StartRecordingOptions {
            source_id: "src-1".into(),
            capture_screen: true,
            camera_id: Some("cam-1".into()),
            mic_id: Some("mic-1".into()),
            capture_system_audio: true,
            fps: 60,
            resolution: "1080p".into(),
            layout: None,
            project_name: None,
            project_dir: None,
            mic_gain_db: Some(-6.0),
            video_bitrate_bps: None,
            capture_mouse: true,
            start_delay_ms: 0,
        };

        let json = serde_json::to_string(&opt).unwrap();
        assert!(json.contains("\"sourceId\":\"src-1\""));
        assert!(json.contains("\"captureSystemAudio\":true"));
        assert!(json.contains("\"micGainDb\":-6.0"));
        // Skipped when None (frontend contract — `undefined` is the same).
        let opt_no_gain = StartRecordingOptions {
            mic_gain_db: None,
            video_bitrate_bps: None,
            capture_mouse: true,
            start_delay_ms: 0,
            ..opt.clone()
        };
        let json_no_gain = serde_json::to_string(&opt_no_gain).unwrap();
        assert!(!json_no_gain.contains("micGainDb"));

        let res = StopRecordingResult {
            project_path: "/tmp/example.aero".into(),
            session_id: "sess-abc".into(),
            state: SessionState::Completed,
            duration_us: 10_000_000,
        };
        let res_json = serde_json::to_string(&res).unwrap();
        assert!(res_json.contains("\"projectPath\":\"/tmp/example.aero\""));
        assert!(res_json.contains("\"sessionId\":\"sess-abc\""));
        assert!(res_json.contains("\"durationUs\":10000000"));
    }

    #[test]
    fn test_repeated_recordings_and_retries() {
        let dir = tempdir().unwrap();
        let state = AppState::new_test(dir.path().to_path_buf());

        let mut portrait_layout = EditLayout::default();
        portrait_layout.aspect_ratio = "9:16".into();
        let opts = StartRecordingOptions {
            source_id: "screen-main".into(),
            capture_screen: true,
            camera_id: None,
            mic_id: None,
            capture_system_audio: false,
            fps: 30,
            resolution: "1080p".into(),
            layout: Some(portrait_layout),
            project_name: None,
            project_dir: None,
            mic_gain_db: None,
            video_bitrate_bps: None,
            capture_mouse: true,
            start_delay_ms: 0,
        };

        // First recording
        let start1 = start_recording_impl(&state, opts.clone()).unwrap();
        assert_eq!(start1.state, SessionState::Recording);
        let sess_id_1 = start1.session_id.clone();

        // Idempotent start retry
        let start1_retry = start_recording_impl(&state, opts.clone()).unwrap();
        assert_eq!(start1_retry.session_id, sess_id_1);

        // Stop first recording
        let stop1 = stop_recording_impl(&state).unwrap();
        assert_eq!(stop1.session_id, sess_id_1);
        assert_eq!(stop1.state, SessionState::Completed);
        let saved = crate::project::revision::load_edit_document(Path::new(&stop1.project_path))
            .unwrap()
            .unwrap();
        assert_eq!(saved.layout.aspect_ratio, "9:16");

        // Idempotent stop retry
        let stop1_retry = stop_recording_impl(&state).unwrap();
        assert_eq!(stop1_retry.session_id, sess_id_1);

        // Second recording directly (must succeed without error)
        let start2 = start_recording_impl(&state, opts.clone()).unwrap();
        assert_eq!(start2.state, SessionState::Recording);
        assert_ne!(
            start2.session_id, sess_id_1,
            "New session must have fresh UUID"
        );

        let stop2 = stop_recording_impl(&state).unwrap();
        assert_eq!(stop2.session_id, start2.session_id);
    }

    #[test]
    fn test_session_status_exposes_diagnostics_fields() {
        let dir = tempdir().unwrap();
        let state = AppState::new_test(dir.path().to_path_buf());

        // A fresh session must report zero gaps and no runtime error.
        let status = get_session_status_impl(&state);
        assert_eq!(status.gaps_total, 0);
        assert!(status.last_runtime_error.is_none());

        // Simulate a runtime error landing in diagnostics.
        state
            .diagnostics
            .apply(&crate::session::SessionEvent::RuntimeError {
                track_id: "screen".into(),
                error_code: 7,
                message: "encoder dropped frames".into(),
                t_us: 1_000_000,
                recoverable: true,
            });

        let status = get_session_status_impl(&state);
        assert_eq!(status.gaps_total, 1);
        let err = status.last_runtime_error.expect("error must surface");
        assert_eq!(err.track_id, "screen");
        assert_eq!(err.error_code, 7);
    }

    #[test]
    fn test_compute_source_geometry_impl_uses_known_source() {
        let sources = list_capture_sources_impl();
        let target_id = sources
            .iter()
            .find(|s| s.id == "screen-secondary")
            .map(|s| s.id.clone())
            .unwrap_or_else(|| sources[0].id.clone());
        let result =
            compute_source_geometry_impl(target_id, Some(1920), Some(1080), Some(FitMode::Fit))
                .expect("source list should compute for known source");
        assert_eq!(result.dest_width, 1920);
        assert_eq!(result.dest_height, 1080);
        assert_eq!(result.geometry.fit_mode, FitMode::Fit);
    }

    #[test]
    fn test_compute_source_geometry_impl_rejects_unknown_source() {
        let res = compute_source_geometry_impl("does-not-exist".into(), None, None, None);
        assert!(res.is_err());
    }

    #[test]
    fn test_compute_source_geometry_impl_rejects_zero_dimensions() {
        let res = compute_source_geometry_impl("screen-main".into(), Some(0), Some(0), None);
        assert!(res.is_err());
    }

    #[test]
    fn test_default_projects_dir_resolves_documents_aeroshootrec() {
        let path = default_projects_dir();
        assert!(
            path.ends_with("Documents/AeroShootRec")
                || path.ends_with("Documents\\AeroShootRec")
                || path.ends_with("AeroShootRec")
        );
        assert_eq!(get_default_projects_dir_impl(), path.to_string_lossy());
    }

    #[test]
    fn named_recording_uses_chosen_folder_and_rejects_collisions() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("Takes");
        fs::create_dir(&nested).unwrap();
        let state = AppState::new_test(dir.path().to_path_buf());

        let first = start_recording_impl(
            &state,
            StartRecordingOptions {
                source_id: "screen-main".into(),
                capture_screen: true,
                camera_id: None,
                mic_id: None,
                capture_system_audio: false,
                fps: 30,
                resolution: "1080p".into(),
                layout: None,
                project_name: Some(" Launch Demo ".into()),
                project_dir: Some(nested.to_string_lossy().into_owned()),
                mic_gain_db: None,
                video_bitrate_bps: None,
                capture_mouse: true,
                start_delay_ms: 0,
            },
        )
        .unwrap();
        stop_recording_impl(&state).unwrap();

        let bundle = nested.join("Launch Demo.aero");
        assert!(bundle.is_dir());
        let manifest: crate::project::ProjectManifest =
            serde_json::from_str(&fs::read_to_string(bundle.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest.project_name, "Launch Demo");
        assert_eq!(manifest.session_id, first.session_id);

        let collision = start_recording_impl(
            &state,
            StartRecordingOptions {
                source_id: "screen-main".into(),
                capture_screen: true,
                camera_id: None,
                mic_id: None,
                capture_system_audio: false,
                fps: 30,
                resolution: "1080p".into(),
                layout: None,
                project_name: Some("Launch Demo".into()),
                project_dir: Some(nested.to_string_lossy().into_owned()),
                mic_gain_db: None,
                video_bitrate_bps: None,
                capture_mouse: true,
                start_delay_ms: 0,
            },
        );
        assert!(collision.unwrap_err().contains("already exists"));

        let untitled = start_recording_impl(
            &state,
            StartRecordingOptions {
                source_id: "screen-main".into(),
                capture_screen: true,
                camera_id: None,
                mic_id: None,
                capture_system_audio: false,
                fps: 30,
                resolution: "1080p".into(),
                layout: None,
                project_name: None,
                project_dir: Some(nested.to_string_lossy().into_owned()),
                mic_gain_db: None,
                video_bitrate_bps: None,
                capture_mouse: true,
                start_delay_ms: 0,
            },
        )
        .unwrap();
        stop_recording_impl(&state).unwrap();
        drop(untitled);
        let default_name = crate::project::default_project_name();
        assert!(nested.join(format!("{default_name}.aero")).is_dir());

        start_recording_impl(
            &state,
            StartRecordingOptions {
                source_id: "screen-main".into(),
                capture_screen: true,
                camera_id: None,
                mic_id: None,
                capture_system_audio: false,
                fps: 30,
                resolution: "1080p".into(),
                layout: None,
                project_name: None,
                project_dir: Some(nested.to_string_lossy().into_owned()),
                mic_gain_db: None,
                video_bitrate_bps: None,
                capture_mouse: true,
                start_delay_ms: 0,
            },
        )
        .unwrap();
        stop_recording_impl(&state).unwrap();
        assert!(nested.join(format!("{default_name} 2.aero")).is_dir());

        let relative = start_recording_impl(
            &state,
            StartRecordingOptions {
                source_id: "screen-main".into(),
                capture_screen: true,
                camera_id: None,
                mic_id: None,
                capture_system_audio: false,
                fps: 30,
                resolution: "1080p".into(),
                layout: None,
                project_name: Some("Nope".into()),
                project_dir: Some("relative/path".into()),
                mic_gain_db: None,
                video_bitrate_bps: None,
                capture_mouse: true,
                start_delay_ms: 0,
            },
        );
        assert!(relative.unwrap_err().contains("absolute"));
    }

    #[test]
    fn project_parent_rejects_bundle_and_parent_dir_components() {
        let dir = tempdir().unwrap();
        let bundle = ProjectBundle::create_new(dir.path(), "sess", "Inside").unwrap();
        let err = resolve_project_parent(Some(&bundle.root_path().to_string_lossy()), dir.path())
            .unwrap_err();
        assert!(err.contains(".aero"));
        assert!(resolve_project_parent(Some("/tmp/aeroshoot/../secret"), dir.path()).is_err());
        assert!(resolve_project_parent(Some("/tmp/does-not-exist-aeroshoot"), dir.path()).is_err());
    }

    #[test]
    fn optional_screen_session_creates_only_selected_tracks() {
        let dir = tempdir().unwrap();
        let state = AppState::new_test(dir.path().to_path_buf());
        let started = start_recording_impl(
            &state,
            StartRecordingOptions {
                source_id: "screen-main".into(),
                capture_screen: false,
                camera_id: Some("camera-1".into()),
                mic_id: Some("mic-1".into()),
                capture_system_audio: false,
                fps: 30,
                resolution: "1080p".into(),
                layout: None,
                project_name: Some("Camera and Mic".into()),
                project_dir: None,
                mic_gain_db: Some(3.0),
                video_bitrate_bps: None,
                capture_mouse: true,
                start_delay_ms: 0,
            },
        )
        .unwrap();
        let manifest = state
            .active_session
            .read()
            .as_ref()
            .unwrap()
            .project_bundle
            .manifest()
            .clone();
        assert!(manifest
            .tracks
            .iter()
            .all(|track| track.track_type != TrackType::Screen));
        assert!(manifest
            .tracks
            .iter()
            .any(|track| track.track_type == TrackType::Webcam));
        assert!(manifest
            .tracks
            .iter()
            .any(|track| track.track_type == TrackType::MicAudio));
        assert!(manifest.source_geometry.is_none());
        assert!(manifest.cursor_mode.is_none());
        assert_eq!(
            stop_recording_impl(&state).unwrap().session_id,
            started.session_id
        );
    }

    #[test]
    fn recording_with_every_media_source_off_is_rejected() {
        let dir = tempdir().unwrap();
        let state = AppState::new_test(dir.path().to_path_buf());
        let result = start_recording_impl(
            &state,
            StartRecordingOptions {
                source_id: "screen-main".into(),
                capture_screen: false,
                camera_id: None,
                mic_id: None,
                capture_system_audio: false,
                fps: 30,
                resolution: "1080p".into(),
                layout: None,
                project_name: None,
                project_dir: None,
                mic_gain_db: None,
                video_bitrate_bps: None,
                capture_mouse: true,
                start_delay_ms: 0,
            },
        );
        assert!(result.unwrap_err().contains("at least one media source"));
        assert!(state.active_session.read().is_none());
    }

    #[test]
    fn test_window_title_formatting() {
        assert_eq!(
            window_title_for_recording(Some("Launch Demo")),
            "AeroShoot \u{2014} Launch Demo"
        );
        assert_eq!(
            window_title_for_recording(Some("  Launch Demo  ")),
            "AeroShoot \u{2014} Launch Demo"
        );
        let default_title = format!(
            "AeroShoot \u{2014} {}",
            crate::project::default_project_name()
        );
        assert_eq!(window_title_for_recording(None), default_title);
        assert_eq!(window_title_for_recording(Some("   ")), default_title);

        assert_eq!(
            window_title_for_project(Some("Launch Demo")),
            "AeroShoot \u{2014} Launch Demo"
        );
        assert_eq!(window_title_for_project(None), "AeroShoot");
        assert_eq!(window_title_for_project(Some("")), "AeroShoot");
        assert_eq!(window_title_for_project(Some("   ")), "AeroShoot");
    }

    #[test]
    fn test_show_in_finder_impl() {
        let non_existent = "/tmp/does-not-exist-aeroshoot-test-finder-12345";
        let err = show_in_finder_impl(non_existent.into()).unwrap_err();
        assert!(err.contains("Path does not exist"));

        let dir = tempdir().unwrap();
        let result = show_in_finder_impl(dir.path().to_string_lossy().into_owned());
        assert!(result.is_ok());
    }
}
