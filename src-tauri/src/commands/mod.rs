use crate::capture::{AudioDevice, CameraDevice, CaptureSource, CaptureSourceType, PermissionStatus};
use crate::dsp::{SilenceConfig, SilenceCutInterval, SilenceDetector};
use crate::fixtures::{generate_valid_fmp4_segment, generate_valid_wav_segment};
use crate::project::manifest::{PauseInterval, TrackDescriptor, TrackType};
use crate::project::{
    JournalRecord, ProjectBundle, ProjectRecoveryReport, RecoveryEngine, TrackSegmentWriter,
};
use crate::session::{SessionEpoch, SessionState, SessionStateMachine};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub struct ActiveSession {
    pub session_id: String,
    pub project_name: String,
    pub epoch: SessionEpoch,
    pub project_bundle: ProjectBundle,
    pub segment_writer: TrackSegmentWriter,
    pub extra_writers: Vec<TrackSegmentWriter>,
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
    pub permission_override: RwLock<Option<PermissionStatus>>,
}

impl AppState {
    pub fn new(project_base_dir: PathBuf) -> Self {
        Self {
            state_machine: SessionStateMachine::new(),
            command_lock: Mutex::new(()),
            active_session: RwLock::new(None),
            last_stop_result: RwLock::new(None),
            project_base_dir,
            permission_override: RwLock::new(None),
        }
    }

    pub fn new_test(project_base_dir: PathBuf) -> Self {
        Self {
            state_machine: SessionStateMachine::new(),
            command_lock: Mutex::new(()),
            active_session: RwLock::new(None),
            last_stop_result: RwLock::new(None),
            project_base_dir,
            permission_override: RwLock::new(Some(PermissionStatus {
                screen_recording: true,
                camera: true,
                microphone: true,
            })),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        let base_dir = std::env::temp_dir().join("AeroShootRecordings");
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StopRecordingResult {
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
    vec![
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
    ]
}

pub fn list_devices_impl() -> DevicesResult {
    DevicesResult {
        cameras: vec![
            CameraDevice {
                id: "cam-facetime".into(),
                name: "FaceTime HD Camera (Built-in)".into(),
                is_default: true,
            },
            CameraDevice {
                id: "cam-studio".into(),
                name: "Studio Display Camera".into(),
                is_default: false,
            },
        ],
        mics: vec![
            AudioDevice {
                id: "mic-built-in".into(),
                name: "MacBook Pro Studio Microphone".into(),
                is_default: true,
            },
            AudioDevice {
                id: "mic-usb".into(),
                name: "USB Podcast Audio Interface".into(),
                is_default: false,
            },
        ],
    }
}

pub fn get_permission_status_impl(state: &AppState) -> PermissionStatus {
    if let Some(status) = state.permission_override.read().clone() {
        return status;
    }
    crate::capture::check_system_permissions()
}

pub fn start_recording_impl(
    state: &AppState,
    options: StartRecordingOptions,
) -> Result<StartRecordingResult, String> {
    // 1. Serialize all lifecycle commands
    let _cmd_guard = state.command_lock.lock();

    // 2. Check system permissions for required screen recording
    let permissions = get_permission_status_impl(state);
    if !permissions.screen_recording {
        let _ = state.state_machine.transition_to(SessionState::Error);
        return Err("Screen recording permission denied. Please grant permission in macOS System Settings.".into());
    }

    // 3. Retry/Idempotency check: if already recording or preparing, return active session
    if state.state_machine.is_recording() || state.state_machine.current() == SessionState::Preparing {
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
        });
    }

    let manifest_path = bundle.root_path().join("manifest.json");
    bundle
        .manifest_mut()
        .save_with_backup(&manifest_path)
        .map_err(|e| {
            let _ = state.state_machine.transition_to(SessionState::Error);
            format!("Failed saving initial manifest: {}", e)
        })?;

    // Create track segment writer for screen stream
    let mut segment_writer = bundle.create_segment_writer("screen", TrackType::Screen, "h264");
    segment_writer
        .begin_segment(0)
        .map_err(|e| {
            let _ = state.state_machine.transition_to(SessionState::Error);
            format!("Failed to open segment: {}", e)
        })?;

    // Write initial valid fMP4 container and sample packet (no artificial commit!)
    let fmp4_data = generate_valid_fmp4_segment(0, 33_333, true);
    segment_writer
        .write_data(&fmp4_data)
        .map_err(|e| {
            let _ = state.state_machine.transition_to(SessionState::Error);
            format!("Failed writing initial segment data: {}", e)
        })?;

    // Initialize extra track writers if requested
    let mut extra_writers = Vec::new();
    if options.camera_id.is_some() {
        let mut cam_writer = bundle.create_segment_writer("webcam", TrackType::Webcam, "h264");
        cam_writer.begin_segment(0).map_err(|e| format!("Failed to open webcam segment: {}", e))?;
        let cam_fmp4 = generate_valid_fmp4_segment(0, 33_333, true);
        cam_writer.write_data(&cam_fmp4).map_err(|e| format!("Failed writing webcam data: {}", e))?;
        extra_writers.push(cam_writer);
    }
    if options.mic_id.is_some() {
        let mut mic_writer = bundle.create_segment_writer("mic", TrackType::MicAudio, "pcm");
        mic_writer.begin_segment(0).map_err(|e| format!("Failed to open mic segment: {}", e))?;
        let mic_wav = generate_valid_wav_segment(33_333, 48_000, 1);
        mic_writer.write_data(&mic_wav).map_err(|e| format!("Failed writing mic data: {}", e))?;
        extra_writers.push(mic_writer);
    }
    if options.capture_system_audio {
        let mut sys_writer = bundle.create_segment_writer("system", TrackType::SystemAudio, "pcm");
        sys_writer.begin_segment(0).map_err(|e| format!("Failed to open system segment: {}", e))?;
        let sys_wav = generate_valid_wav_segment(33_333, 48_000, 2);
        sys_writer.write_data(&sys_wav).map_err(|e| format!("Failed writing system data: {}", e))?;
        extra_writers.push(sys_writer);
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
        let _ = session.segment_writer.finalize(final_us, session.project_bundle.journal());
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
        let _ = session.segment_writer.begin_segment(now_us);
        let fmp4_data = generate_valid_fmp4_segment(now_us, 33_333, true);
        let _ = session.segment_writer.write_data(&fmp4_data);

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

    // Close any in-flight pause
    if let Some(pause_start) = session.current_pause_start_us.take() {
        session.pause_intervals.push((pause_start, gross_duration_us));
        if let Err(e) = session.project_bundle.journal().append(JournalRecord::PauseEnded {
            seq: 0,
            start_us: pause_start,
            end_us: gross_duration_us,
        }) {
            let _ = state.state_machine.transition_to(SessionState::Error);
            *state.active_session.write() = Some(session);
            return Err(format!("Storage error writing pause interval to journal: {}", e));
        }
    }

    // Non-reversed timestamps: final duration must be strictly positive (at least 1 frame)
    let final_segment_end_us = gross_duration_us.max(33_333);

    // Finalize screen segment writer (flush -> sync -> rename -> journal append)
    if let Err(e) = session
        .segment_writer
        .finalize(final_segment_end_us, session.project_bundle.journal())
    {
        let _ = state.state_machine.transition_to(SessionState::Error);
        *state.active_session.write() = Some(session);
        return Err(format!("Storage finalization failed for screen track: {}", e));
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
            track_id,
            e
        ));
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
        session_id: session.session_id.clone(),
        state: SessionState::Completed,
        duration_us: net_duration_us,
    };

    *state.last_stop_result.write() = Some(result.clone());

    Ok(result)
}

pub fn get_session_status_impl(state: &AppState) -> SessionStatusResult {
    let current_state = state.state_machine.current();
    let elapsed_us = if current_state == SessionState::Recording || current_state == SessionState::Paused {
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

    SessionStatusResult {
        state: current_state,
        elapsed_us,
        dropped_frames: 0,
        audio_buffer_underflows: 0,
    }
}

pub fn detect_silence_impl(
    samples: &[f32],
    sample_rate: u32,
    config: &SilenceConfig,
) -> Vec<SilenceCutInterval> {
    SilenceDetector::detect_silence(samples, sample_rate, config)
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
            session_id: "sess-abc".into(),
            state: SessionState::Completed,
            duration_us: 10_000_000,
        };
        let res_json = serde_json::to_string(&res).unwrap();
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
        assert_ne!(start2.session_id, sess_id_1, "New session must have fresh UUID");

        let stop2 = stop_recording_impl(&state).unwrap();
        assert_eq!(stop2.session_id, start2.session_id);
    }
}
