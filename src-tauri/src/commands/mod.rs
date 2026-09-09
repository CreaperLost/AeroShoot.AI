use crate::capture::{
    AudioDevice, CameraDevice, CaptureSource, CaptureSourceType, FitMode, PermissionState,
    PermissionStatus, SourceGeometry,
};
use crate::dsp::{SilenceConfig, SilenceCutInterval, SilenceDetector};
use crate::fixtures::{generate_valid_fmp4_segment, generate_valid_wav_segment};
use crate::media::{EncoderGate, MediaInteropStatus, MediaParityReport};
use crate::playback::{
    self, PlaybackOwner, PlaybackStatus, PreviewHitMode, PreviewOwner, PreviewStatus,
    PreviewViewport,
};
use crate::project::manifest::{PauseInterval, TrackDescriptor, TrackType};
use crate::project::{
    JournalRecord, OpenedProject, ProjectBundle, ProjectReader, ProjectRecoveryReport,
    RecoveryEngine, SegmentPage, TrackSegmentWriter, WaveformPage, WaveformTrackContext,
};
use crate::session::{
    RuntimeErrorRecord, SessionDiagnostics, SessionEpoch, SessionState, SessionStateMachine,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
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
    pub pause_intervals: Vec<(u64, u64)>,
    pub current_pause_start_us: Option<u64>,
    pub started_at_us: i64,
}

pub struct AppState {
    pub state_machine: SessionStateMachine,
    pub command_lock: Mutex<()>,
    pub active_session: RwLock<Option<ActiveSession>>,
    pub last_stop_result: RwLock<Option<StopRecordingResult>>,
    pub project_base_dir: PathBuf,
    pub opened_project: Mutex<Option<crate::project::ProjectReader>>,
    pub playback: Mutex<PlaybackOwner>,
    pub playback_shutdown: std::sync::atomic::AtomicBool,
    pub preview: Mutex<PreviewOwner>,
    pub encoder_gate: Arc<EncoderGate>,
    pub export: Mutex<crate::export::ExportOwner>,
    pub waveform_epoch: AtomicU64,
    pub waveform_generations: Mutex<HashMap<String, u64>>,
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
            opened_project: Mutex::new(None),
            playback: Mutex::new(PlaybackOwner::closed()),
            playback_shutdown: std::sync::atomic::AtomicBool::new(false),
            preview: Mutex::new(PreviewOwner::new()),
            encoder_gate: Arc::new(EncoderGate::new()),
            export: Mutex::new(crate::export::ExportOwner::new()),
            waveform_epoch: AtomicU64::new(0),
            waveform_generations: Mutex::new(HashMap::new()),
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
            opened_project: Mutex::new(None),
            playback: Mutex::new(PlaybackOwner::closed()),
            playback_shutdown: std::sync::atomic::AtomicBool::new(false),
            preview: Mutex::new(PreviewOwner::new()),
            encoder_gate: Arc::new(EncoderGate::new()),
            export: Mutex::new(crate::export::ExportOwner::new()),
            waveform_epoch: AtomicU64::new(0),
            waveform_generations: Mutex::new(HashMap::new()),
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
    #[serde(default)]
    pub camera_id: Option<String>,
    #[serde(default)]
    pub mic_id: Option<String>,
    #[serde(default)]
    pub capture_system_audio: bool,
    pub fps: u32,
    pub resolution: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StartRecordingResult {
    pub session_id: String,
    pub state: SessionState,
    pub started_at_us: i64,
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
    /// Aggregate number of gaps / discontinuities observed across every
    /// track in the active session. Mirrors the journal's `Discontinuity`
    /// records so the UI can show "N gaps" without re-scanning disk.
    #[serde(default)]
    pub gaps_total: u64,
    #[serde(default)]
    pub timestamp_records_dropped: u64,
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
            Some("ScreenCapture") => "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            Some("Camera") => "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera",
            Some("Microphone") => "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
            Some("Accessibility") => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            Some("InputMonitoring") | Some("ListenEvent") => "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
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

pub fn start_recording_impl(
    state: &AppState,
    options: StartRecordingOptions,
) -> Result<StartRecordingResult, String> {
    // 1. Serialize all lifecycle commands
    let _cmd_guard = state.command_lock.lock();

    if state.export.lock().busy() { return Err("Wait for the current export to finish or cancel it before recording".into()); }

    // 2. Check system permissions for required screen recording. Camera and
    // microphone are optional tracks: if those TCC grants are missing, drop
    // them and still record the screen instead of failing the whole session.
    let permissions = get_permission_status_impl(state);
    if !permissions.screen_recording.is_authorized() {
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

    // 3. Retry/Idempotency check: if already recording or preparing, return active session
    if state.state_machine.is_recording()
        || state.state_machine.current() == SessionState::Preparing
    {
        if let Some(session) = state.active_session.read().as_ref() {
            return Ok(StartRecordingResult {
                session_id: session.session_id.clone(),
                state: SessionState::Recording,
                started_at_us: session.started_at_us,
            });
        }
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
    let epoch = SessionEpoch::now();
    let started_at_us = epoch.start_wall_time_us();

    // Create real project bundle
    let mut bundle = ProjectBundle::create_new(
        &state.project_base_dir,
        &session_id,
        "AeroShoot Studio Session",
    )
    .map_err(|e| {
        let _ = state.state_machine.transition_to(SessionState::Error);
        format!("Failed to create project bundle: {}", e)
    })?;

    // Parse recording options
    let (width, height) = match options.resolution.as_str() {
        "4k" | "4K" => (3840, 2160),
        "720p" => (1280, 720),
        _ => (1920, 1080),
    };
    let fps = if options.fps > 0 { options.fps } else { 30 };

    // 1. Primary screen track
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
    bundle.manifest_mut().source_geometry = Some(geometry);
    bundle.manifest_mut().cursor_mode = Some("baked".into());

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

            let started_at = std::time::Instant::now();
            native_session = Some(
                crate::capture::macos::MacCaptureSession::start(
                    crate::capture::macos::NativeRecordingConfig {
                        source_id: &options.source_id,
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
                        session_offset_us: epoch.current_elapsed_us(),
                    },
                )
                .map_err(|error| {
                    // Tear down the callback targets so a follow-up
                    // session starts from a clean slate.
                    crate::capture::macos::clear_callback_targets();
                    let _ = state.state_machine.transition_to(SessionState::Error);
                    format!("Failed to start native macOS capture: {error}")
                })?,
            );

            // Defensive 5 s startup budget: if the macOS capture session
            // returned a handle but never produced a callback (i.e. the
            // `startCaptureHandler` completion is silently lost), we
            // still succeed at the Rust level so the UI can show a
            // "recording" state. The runtime-error callback is the
            // canonical signal; if it never fires, the user can stop
            // manually. A future Swift update will signal
            // startCaptureHandler completion via the runtime-error
            // channel with a reserved code; when that lands, this
            // budget becomes a hard timeout enforced by a side channel.
            let _ = started_at; // captured for the future timeout enforcement
        }
    } else {
        let mut writer = bundle.create_segment_writer("screen", TrackType::Screen, "h264");
        writer
            .begin_segment(0)
            .map_err(|e| format!("Failed to open segment: {e}"))?;
        writer
            .write_data(&generate_valid_fmp4_segment(0, 33_333, true))
            .map_err(|e| format!("Failed writing initial segment data: {e}"))?;
        segment_writer = Some(writer);

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

    *state.active_session.write() = Some(ActiveSession {
        session_id: session_id.clone(),
        project_name: "AeroShoot Studio Session".into(),
        epoch,
        project_bundle: bundle,
        segment_writer,
        extra_writers,
        #[cfg(target_os = "macos")]
        native_session,
        pause_intervals: Vec::new(),
        current_pause_start_us: None,
        started_at_us,
    });

    Ok(StartRecordingResult {
        session_id,
        state: SessionState::Recording,
        started_at_us,
    })
}

pub fn pause_recording_impl(state: &AppState) -> Result<SessionStateResult, String> {
    let _cmd_guard = state.command_lock.lock();

    if state.state_machine.is_paused() {
        return Ok(SessionStateResult {
            state: SessionState::Paused,
        });
    }

    state
        .state_machine
        .transition_to(SessionState::Paused)
        .map_err(|e| e.to_string())?;

    let mut session_guard = state.active_session.write();
    if let Some(session) = session_guard.as_mut() {
        let now_us = session.epoch.current_elapsed_us();
        session.current_pause_start_us = Some(now_us);
        session
            .project_bundle
            .journal()
            .append(JournalRecord::PauseStarted {
                seq: 0,
                t_us: now_us,
            })
            .map_err(|e| format!("Failed to append PauseStarted to journal: {}", e))?;

        // Durably close/finalize active segment up to pause timestamp
        let final_us = now_us.max(33_333);
        if let Some(writer) = session.segment_writer.as_mut() {
            let _ = writer.finalize(final_us, session.project_bundle.journal());
        }
        #[cfg(target_os = "macos")]
        if let Some(native) = session.native_session.as_ref() {
            native.set_paused(true);
        }
        for writer in &mut session.extra_writers {
            let _ = writer.finalize(final_us, session.project_bundle.journal());
        }
    }

    Ok(SessionStateResult {
        state: SessionState::Paused,
    })
}

pub fn resume_recording_impl(state: &AppState) -> Result<SessionStateResult, String> {
    let _cmd_guard = state.command_lock.lock();

    if state.state_machine.is_recording() {
        return Ok(SessionStateResult {
            state: SessionState::Recording,
        });
    }

    state
        .state_machine
        .transition_to(SessionState::Recording)
        .map_err(|e| e.to_string())?;

    let mut session_guard = state.active_session.write();
    if let Some(session) = session_guard.as_mut() {
        let now_us = session.epoch.current_elapsed_us();
        if let Some(pause_start) = session.current_pause_start_us.take() {
            session.pause_intervals.push((pause_start, now_us));
            session
                .project_bundle
                .journal()
                .append(JournalRecord::PauseEnded {
                    seq: 0,
                    start_us: pause_start,
                    end_us: now_us,
                })
                .map_err(|e| format!("Failed to append PauseEnded to journal: {}", e))?;
        }

        // Reopen new continuous segments starting at resume timestamp
        if let Some(writer) = session.segment_writer.as_mut() {
            let _ = writer.begin_segment(now_us);
            let fmp4_data = generate_valid_fmp4_segment(now_us, 33_333, true);
            let _ = writer.write_data(&fmp4_data);
        }
        #[cfg(target_os = "macos")]
        if let Some(native) = session.native_session.as_ref() {
            native.set_paused(false);
        }

        for writer in &mut session.extra_writers {
            let _ = writer.begin_segment(now_us);
            if writer.track_type() == TrackType::Webcam {
                let d = generate_valid_fmp4_segment(now_us, 33_333, true);
                let _ = writer.write_data(&d);
            } else {
                let d = generate_valid_wav_segment(33_333, 48_000, 2);
                let _ = writer.write_data(&d);
            }
        }
    }

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
    let was_native = session.native_session.is_some();
    #[cfg(not(target_os = "macos"))]
    let was_native = false;
    #[cfg(target_os = "macos")]
    let native_stop_result: Option<Result<(), (i32, String)>> =
        if let Some(native) = session.native_session.take() {
            // Use the typed stop so we can refuse to commit failed media.
            // The legacy `stop` is reserved for the Drop impl, which is a
            // last-resort cleanup path.
            Some(native.stop_with_result())
        } else {
            None
        };

    // Clear the global callback targets now that the session is over;
    // any late callback after this point must be a no-op.
    #[cfg(target_os = "macos")]
    {
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
            let _ = state.state_machine.transition_to(SessionState::Error);
            *state.active_session.write() = Some(session);
            return Err(format!(
                "Storage finalization failed for screen track: {}",
                e
            ));
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
        let _ = state.state_machine.transition_to(SessionState::Error);
        *state.active_session.write() = Some(session);
        return Err(format!(
            "Storage finalization failed for track {}: {}",
            track_id, e
        ));
    }

    if was_native {
        // If the native stop reported a non-OK status, do NOT commit any
        // media: a failed or timed-out writer must not produce a
        // successful journal record. The temp file is left in place so
        // recovery (Task 2) can later re-validate and salvage it.
        if let Some(Err((code, message))) = native_stop_result {
            let _ = state.state_machine.transition_to(SessionState::Error);
            return Err(format!(
                "Native capture stopped with error (code {code}): {message}. \
                 Previously committed segments remain available for recovery."
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

    if let Err(e) = session
        .project_bundle
        .manifest()
        .save_with_backup(&manifest_path)
    {
        let _ = state.state_machine.transition_to(SessionState::Error);
        *state.active_session.write() = Some(session);
        return Err(format!("Storage error saving manifest backup: {}", e));
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
    let (dropped_frames, audio_buffer_underflows, native_gaps, timestamp_records_dropped) = state
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
            )
        })
        .unwrap_or((0, 0, 0, 0));
    #[cfg(not(target_os = "macos"))]
    let (dropped_frames, audio_buffer_underflows, native_gaps, timestamp_records_dropped) =
        (0, 0, 0, 0);

    SessionStatusResult {
        state: current_state,
        elapsed_us,
        dropped_frames,
        audio_buffer_underflows,
        last_runtime_error: state.diagnostics.last_runtime_error(),
        gaps_total: state.diagnostics.gaps_total().max(native_gaps),
        timestamp_records_dropped,
    }
}

pub fn detect_silence_impl(
    samples: &[f32],
    sample_rate: u32,
    config: &SilenceConfig,
) -> Vec<SilenceCutInterval> {
    SilenceDetector::detect_silence(samples, sample_rate, config)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_ipc_serialization_roundtrip_camel_case() {
        let opt = StartRecordingOptions {
            source_id: "src-1".into(),
            camera_id: Some("cam-1".into()),
            mic_id: Some("mic-1".into()),
            capture_system_audio: true,
            fps: 60,
            resolution: "1080p".into(),
        };

        let json = serde_json::to_string(&opt).unwrap();
        assert!(json.contains("\"sourceId\":\"src-1\""));
        assert!(json.contains("\"captureSystemAudio\":true"));

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

        let opts = StartRecordingOptions {
            source_id: "screen-main".into(),
            camera_id: None,
            mic_id: None,
            capture_system_audio: false,
            fps: 30,
            resolution: "1080p".into(),
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
        assert!(path.ends_with("Documents/AeroShootRec") || path.ends_with("Documents\\AeroShootRec") || path.ends_with("AeroShootRec"));
        assert_eq!(get_default_projects_dir_impl(), path.to_string_lossy());
    }
}

pub fn open_project_impl(state: &AppState, path: String) -> Result<OpenedProject, String> {
    let _guard = state.command_lock.lock();
    let reader = ProjectReader::open(std::path::Path::new(&path))?;
    let summary = reader.summary.clone();
    let tracks = playback::tracks_from_reader(&reader);
    let document = reader.history().current.clone();
    let mut owner = PlaybackOwner::open(
        summary.project_handle.clone(),
        reader.root().to_path_buf(),
        &document,
        tracks,
    )?;
    *state.opened_project.lock() = Some(reader);
    owner.native_enabled = state.native_capture_enabled;
    *state.playback.lock() = owner;
    state.waveform_epoch.fetch_add(1, Ordering::SeqCst);
    state.waveform_generations.lock().clear();
    Ok(summary)
}

pub fn close_project_impl(state: &AppState, project_handle: String) -> Result<(), String> {
    let _guard = state.command_lock.lock();
    let mut opened = state.opened_project.lock();
    if let Some(reader) = opened.as_ref() {
        if reader.summary.project_handle != project_handle {
            return Err("Stale project handle".into());
        }
    }
    *opened = None;
    state.playback.lock().close();
    state.waveform_epoch.fetch_add(1, Ordering::SeqCst);
    state.waveform_generations.lock().clear();
    Ok(())
}

pub fn project_segments_impl(
    state: &AppState,
    project_handle: String,
    track_id: String,
    offset: usize,
    limit: usize,
) -> Result<SegmentPage, String> {
    let opened = state.opened_project.lock();
    let reader = opened.as_ref().ok_or("No opened project")?;
    if reader.summary.project_handle != project_handle {
        return Err("Stale project handle".into());
    }
    reader.page(&track_id, offset, limit)
}

pub fn project_waveform_impl(
    state: &AppState,
    project_handle: String,
    track_id: String,
    start_us: u64,
    end_us: u64,
    bucket_count: usize,
) -> Result<WaveformPage, String> {
    let epoch = state.waveform_epoch.load(Ordering::SeqCst);
    let generation = {
        let mut generations = state.waveform_generations.lock();
        let slot = generations.entry(track_id.clone()).or_insert(0);
        *slot += 1;
        *slot
    };
    let ctx = {
        let opened = state.opened_project.lock();
        let reader = opened.as_ref().ok_or("No opened project")?;
        if reader.summary.project_handle != project_handle {
            return Err("Stale project handle".into());
        }
        let track = reader
            .summary
            .tracks
            .iter()
            .find(|track| track.descriptor.id == track_id)
            .ok_or("Unknown track")?;
        WaveformTrackContext {
            root: reader.root().to_path_buf(),
            track_id: track_id.clone(),
            track_type: track.descriptor.track_type,
            segments: reader
                .segments_for(&track_id)
                .ok_or("Unknown track")?
                .to_vec(),
            retained: reader.summary.retained_intervals.clone(),
            edited_duration_us: reader.summary.edited_duration_us,
        }
    };
    crate::project::waveform::query_waveform(&ctx, start_us, end_us, bucket_count, &|| {
        if state.waveform_epoch.load(Ordering::SeqCst) != epoch {
            return true;
        }
        state
            .waveform_generations
            .lock()
            .get(&track_id)
            .copied()
            .unwrap_or(0)
            != generation
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EditCut {
    pub start_us: u64,
    pub end_us: u64,
}

fn require_handle(reader: &ProjectReader, project_handle: &str) -> Result<(), String> {
    if reader.summary.project_handle != project_handle {
        return Err("Stale project handle".into());
    }
    Ok(())
}

pub fn project_ripple_cuts_impl(
    state: &AppState,
    project_handle: String,
    expected_revision: u64,
    cuts: Vec<EditCut>,
) -> Result<OpenedProject, String> {
    let _guard = state.command_lock.lock();
    let mut opened = state.opened_project.lock();
    let reader = opened.as_mut().ok_or("No opened project")?;
    require_handle(reader, &project_handle)?;
    let ranges: Vec<(u64, u64)> = cuts
        .into_iter()
        .map(|cut| (cut.start_us, cut.end_us))
        .collect();
    let summary = reader.ripple_cuts(expected_revision, &ranges)?;
    state
        .playback
        .lock()
        .apply_document(&reader.history().current)?;
    state.waveform_epoch.fetch_add(1, Ordering::SeqCst);
    Ok(summary)
}

pub fn project_undo_impl(
    state: &AppState,
    project_handle: String,
    expected_revision: u64,
) -> Result<OpenedProject, String> {
    let _guard = state.command_lock.lock();
    let mut opened = state.opened_project.lock();
    let reader = opened.as_mut().ok_or("No opened project")?;
    require_handle(reader, &project_handle)?;
    let summary = reader.undo(expected_revision)?;
    state
        .playback
        .lock()
        .apply_document(&reader.history().current)?;
    state.waveform_epoch.fetch_add(1, Ordering::SeqCst);
    Ok(summary)
}

pub fn project_redo_impl(
    state: &AppState,
    project_handle: String,
    expected_revision: u64,
) -> Result<OpenedProject, String> {
    let _guard = state.command_lock.lock();
    let mut opened = state.opened_project.lock();
    let reader = opened.as_mut().ok_or("No opened project")?;
    require_handle(reader, &project_handle)?;
    let summary = reader.redo(expected_revision)?;
    state
        .playback
        .lock()
        .apply_document(&reader.history().current)?;
    state.waveform_epoch.fetch_add(1, Ordering::SeqCst);
    Ok(summary)
}

pub fn playback_status_impl(
    state: &AppState,
    project_handle: String,
) -> Result<PlaybackStatus, String> {
    let mut playback = state.playback.lock();
    let status = playback.status()?;
    if status.state == crate::playback::PlaybackState::Closed {
        return Err("Playback is closed".into());
    }
    if status.project_handle != project_handle {
        return Err("Stale project handle".into());
    }
    Ok(status)
}

pub fn playback_play_impl(
    state: &AppState,
    project_handle: String,
) -> Result<PlaybackStatus, String> {
    let mut playback = state.playback.lock();
    if playback.status()?.project_handle != project_handle {
        return Err("Stale project handle".into());
    }
    playback.play()
}

pub fn playback_pause_impl(
    state: &AppState,
    project_handle: String,
) -> Result<PlaybackStatus, String> {
    let mut playback = state.playback.lock();
    if playback.status()?.project_handle != project_handle {
        return Err("Stale project handle".into());
    }
    playback.pause()
}

pub fn playback_seek_impl(
    state: &AppState,
    project_handle: String,
    edited_us: u64,
) -> Result<PlaybackStatus, String> {
    let mut playback = state.playback.lock();
    if playback.status()?.project_handle != project_handle {
        return Err("Stale project handle".into());
    }
    playback.seek(edited_us)
}

pub fn preview_attach_impl(
    state: &AppState,
    window_label: String,
    hit_mode: PreviewHitMode,
    native_window: Option<*mut std::ffi::c_void>,
) -> Result<PreviewStatus, String> {
    if native_window.is_none() {
        return Err("Native preview requires a desktop window".into());
    }
    state
        .preview
        .lock()
        .attach(window_label, hit_mode, native_window)
}

pub fn preview_layout_impl(
    state: &AppState,
    viewport: PreviewViewport,
) -> Result<PreviewStatus, String> {
    if viewport.generation == 0 { return Err("Preview generation is required".into()); }
    state.preview.lock().layout(viewport)
}

pub fn preview_present_fixed_impl(
    state: &AppState,
    r: f32,
    g: f32,
    b: f32,
    generation: u64,
) -> Result<PreviewStatus, String> {
    state.preview.lock().present_fixed(r, g, b, generation)
}

pub fn preview_present_fixture_impl(
    state: &AppState,
    path: String,
    generation: u64,
) -> Result<PreviewStatus, String> {
    state.preview.lock().present_fixture(&path, generation)
}

pub fn preview_status_impl(state: &AppState) -> PreviewStatus {
    state.preview.lock().status()
}

pub fn preview_hit_test_impl(state: &AppState, x: f64, y: f64) -> bool {
    state.preview.lock().hit_test(x, y)
}

pub fn preview_detach_impl(
    state: &AppState,
    window_label: String,
) -> Result<PreviewStatus, String> {
    let mut preview = state.preview.lock();
    if preview.status().attached
        && preview.status().window_label.as_deref() != Some(window_label.as_str())
    {
        return Err("Stale preview window label".into());
    }
    preview.detach();
    Ok(preview.status())
}

pub fn media_interop_status_impl(state: &AppState) -> MediaInteropStatus {
    let _ = state;
    crate::media::interop_status(
        None,
        vec![
            "FFmpeg is not pinned; VideoToolbox implements the decoder/encoder contract".into(),
            "WGPU uses a CPU upload/readback fallback; Metal texture interop is untested".into(),
        ],
    )
}

pub fn media_run_parity_impl(state: &AppState) -> Result<MediaParityReport, String> {
    let dir = std::env::temp_dir().join(format!("aeroshoot-f2-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let report = crate::render::run_parity(&dir, &state.encoder_gate);
    let _ = std::fs::remove_dir_all(&dir);
    report
}

pub fn export_start_impl(
    state: &AppState,
    project_handle: String,
    settings: crate::export::ExportSettings,
) -> Result<crate::export::ExportStatus, String> {
    let _guard = state.command_lock.lock();
    let session_state = state.state_machine.current();
    let opened = state.opened_project.lock();
    let reader = opened.as_ref().ok_or("No opened project")?;
    require_handle(reader, &project_handle)?;
    let document = reader.history().current.clone();
    let tracks = playback::tracks_from_reader(reader);
    let root = reader.root().to_path_buf();
    let name = reader.summary.manifest.project_name.clone();
    drop(opened);
    let gate = Arc::clone(&state.encoder_gate);
    let mut owner = state.export.lock();
    match crate::export::prepare_job(
        session_state,
        &root,
        &name,
        document,
        tracks,
        settings,
        &mut owner,
    ) {
        Ok(captured) => Ok(crate::export::spawn_job(captured, &mut owner, gate)),
        Err(status) => {
            owner.install_failed(status.clone());
            Ok(status)
        }
    }
}

pub fn export_status_impl(
    state: &AppState,
    job_id: Option<String>,
) -> Result<crate::export::ExportStatus, String> {
    let mut owner = state.export.lock();
    let status = owner.status();
    if let Some(id) = job_id {
        if !status.job_id.is_empty() && status.job_id != id {
            return Err("Stale export job id".into());
        }
    }
    Ok(status)
}

pub fn export_cancel_impl(
    state: &AppState,
    job_id: String,
) -> Result<crate::export::ExportStatus, String> {
    state.export.lock().cancel(&job_id)
}
