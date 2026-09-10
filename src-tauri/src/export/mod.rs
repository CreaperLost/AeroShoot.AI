//! Immutable-revision export job. Preview and export share one scene evaluator.
use crate::media::audio::{AudioMixer, CHANNELS, CHUNK_FRAMES, SAMPLE_RATE};
mod native;

use crate::media::{
    compare_frames, decode_h264_frame, EncoderGate, VideoFrame, MAX_FRAME_DIM,
    PARITY_MEAN_TOLERANCE, PARITY_REGION_MEAN_TOLERANCE,
};
use crate::project::manifest::TrackType;
use crate::project::reader::{safe_path, SegmentSummary, TrackSummary};
use crate::project::revision::EditDocument;
use crate::render::{Compositor, Scene};
use crate::session::SessionState;
use native::NativeExport;

pub use native::media_duration_us;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use uuid::Uuid;

pub const MAX_EXPORT_FRAMES: u32 = u32::MAX;
pub const ALLOWED_FPS: [u32; 6] = [10, 15, 24, 25, 30, 60];
/// One AAC frame at 48 kHz plus a small encoder-delay allowance.
pub const AUDIO_DURATION_SLACK_US: u64 = 80_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExportSettings {
    pub video_codec: String,
    pub audio_codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    #[serde(default)]
    pub destination: Option<String>,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            video_codec: "h264".into(),
            audio_codec: "aac".into(),
            width: 1920,
            height: 1080,
            fps: 30,
            destination: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExportState {
    Idle,
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExportFailure {
    Collision { message: String },
    Cancelled { message: String },
    InvalidSettings { message: String },
    SourcePath { message: String },
    RecordingActive { message: String },
    EncoderBusy { message: String },
    Native { message: String },
    Io { message: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExportStatus {
    pub job_id: String,
    pub state: ExportState,
    pub captured_revision: u64,
    pub video_codec: String,
    pub audio_codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub progress_numerator: u32,
    pub progress_denominator: u32,
    pub output_path: Option<String>,
    pub failure: Option<ExportFailure>,
    pub diagnostics: Vec<String>,
}

impl ExportStatus {
    pub fn idle() -> Self {
        Self {
            job_id: String::new(),
            state: ExportState::Idle,
            captured_revision: 0,
            video_codec: "h264".into(),
            audio_codec: "aac".into(),
            width: 1920,
            height: 1080,
            fps: 30,
            progress_numerator: 0,
            progress_denominator: 0,
            output_path: None,
            failure: None,
            diagnostics: Vec::new(),
        }
    }
}

struct LiveJob {
    id: String,
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<ExportStatus>>,
    join: Option<JoinHandle<()>>,
}

pub struct ExportOwner {
    job: Option<LiveJob>,
}

impl ExportOwner {
    pub fn new() -> Self {
        Self { job: None }
    }

    pub fn status(&mut self) -> ExportStatus {
        self.reap();
        self.job
            .as_ref()
            .map(|job| job.status.lock().clone())
            .unwrap_or_else(ExportStatus::idle)
    }

    pub fn cancel(&mut self, job_id: &str) -> Result<ExportStatus, String> {
        self.reap();
        let job = self.job.as_mut().ok_or("No export job")?;
        if job.id != job_id {
            return Err("Stale export job id".into());
        }
        job.cancel.store(true, Ordering::SeqCst);
        let mut status = job.status.lock();
        if matches!(status.state, ExportState::Queued | ExportState::Running) {
            status.diagnostics = vec!["Cancellation requested; waiting for cleanup".into()];
        }
        Ok(status.clone())
    }

    pub fn busy(&mut self) -> bool {
        self.reap();
        self.job
            .as_ref()
            .map(|job| {
                job.join.as_ref().is_some_and(|join| !join.is_finished())
                    || matches!(
                        job.status.lock().state,
                        ExportState::Queued | ExportState::Running
                    )
            })
            .unwrap_or(false)
    }

    pub fn install_failed(&mut self, status: ExportStatus) {
        self.reap();
        if self.busy() {
            return;
        }
        self.job = Some(LiveJob {
            id: status.job_id.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
            status: Arc::new(Mutex::new(status)),
            join: None,
        });
    }

    fn reap(&mut self) {
        if let Some(job) = self.job.as_mut() {
            if job
                .join
                .as_ref()
                .map(|handle| handle.is_finished())
                .unwrap_or(true)
            {
                if let Some(handle) = job.join.take() {
                    if handle.join().is_err() {
                        let mut status = job.status.lock();
                        status.state = ExportState::Failed;
                        status.output_path = None;
                        status.failure = Some(ExportFailure::Native {
                            message: "Export worker terminated unexpectedly".into(),
                        });
                    }
                }
            }
        }
    }
}

impl Default for ExportOwner {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ExportOwner {
    fn drop(&mut self) {
        if let Some(job) = self.job.as_mut() {
            job.cancel.store(true, Ordering::SeqCst);
            if let Some(handle) = job.join.take() {
                let _ = handle.join();
            }
        }
    }
}

#[derive(Clone)]
pub struct CapturedExport {
    pub job_id: String,
    pub root: PathBuf,
    pub document: EditDocument,
    pub tracks: Vec<(TrackSummary, Vec<SegmentSummary>)>,
    pub dest: PathBuf,
    pub temp: PathBuf,
    pub settings: ExportSettings,
    pub lease: Arc<File>,
}

pub struct SceneEvaluator {
    root: PathBuf,
    document: EditDocument,
    tracks: Vec<(TrackSummary, Vec<SegmentSummary>)>,
    compositor: Option<Compositor>,
    width: u32,
    height: u32,
}

impl SceneEvaluator {
    pub fn new(
        root: PathBuf,
        document: EditDocument,
        tracks: Vec<(TrackSummary, Vec<SegmentSummary>)>,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        Ok(Self {
            root,
            document,
            tracks,
            compositor: Some(Compositor::new()?),
            width,
            height,
        })
    }

    /// CPU-only evaluator for contract tests that must not require a GPU adapter.
    pub fn new_cpu(
        root: PathBuf,
        document: EditDocument,
        tracks: Vec<(TrackSummary, Vec<SegmentSummary>)>,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        Ok(Self {
            root,
            document,
            tracks,
            compositor: None,
            width,
            height,
        })
    }

    pub fn preview_at(&mut self, edited_us: u64) -> Result<VideoFrame, String> {
        let scene = self.scene_at(edited_us)?;
        let mut frame = match self.compositor.as_mut() {
            Some(compositor) => compositor.composite(&scene)?,
            None => Compositor::composite_cpu(&scene)?,
        };
        frame.pts_us = edited_us;
        Ok(frame)
    }

    pub fn scene_at(&self, edited_us: u64) -> Result<Scene, String> {
        let mapper = self.document.mapper()?;
        let duration_us = mapper.total_edited_duration_us();
        let ended = edited_us >= duration_us;
        let source_us = mapper.edited_to_source_us(edited_us);
        let mut screen = None;
        let mut webcam = None;
        for (track, segments) in &self.tracks {
            if ended {
                return Err("The exclusive edited end is not a video sample".into());
            }
            let source = source_us.ok_or("No source sample at edited position")?;
            let containing = segments
                .iter()
                .find(|s| s.start_us <= source && source < s.end_us && s.available);
            match track.descriptor.track_type {
                TrackType::Screen => {
                    let candidate = containing.map(|s| (s, source)).or_else(|| {
                        // A gap holds the last retained source picture, independent of seek history.
                        self.document
                            .retained_intervals
                            .iter()
                            .rev()
                            .find_map(|interval| {
                                segments.iter().rev().find_map(|s| {
                                    let end = source.min(interval.end_us).min(s.end_us);
                                    (s.available && end > interval.start_us.max(s.start_us))
                                        .then_some((s, end.saturating_sub(1)))
                                })
                            })
                    });
                    screen = candidate
                        .map(|(segment, time)| decode_layer(&self.root, segment, time))
                        .transpose()?;
                }
                TrackType::Webcam => {
                    webcam = containing
                        .map(|segment| decode_layer(&self.root, segment, source))
                        .transpose()?;
                }
                TrackType::MicAudio | TrackType::SystemAudio => {}
            }
        }

        let has_screen = screen.is_some();
        let wallpaper = crate::render::load_wallpaper_frame(
            &self.root,
            &self.document.layout,
            self.width,
            self.height,
        )?;
        let mut scene = Scene::from_layout_with_wallpaper(
            self.width,
            self.height,
            &self.document.layout,
            screen,
            webcam,
            wallpaper,
        )?;
        let zooms = self.document.zoom_suggestions();
        let config = crate::zoom::eval_config_for(&self.document.zooms);
        let camera = crate::zoom::evaluate_at_edited(&zooms, &mapper, edited_us, &config)
            .unwrap_or_else(crate::zoom::CameraTransform::identity);
        if has_screen {
            let (uv_x, uv_y, uv_w, uv_h) = camera.uv_rect();
            scene.apply_screen_uv(uv_x, uv_y, uv_w, uv_h);
        }
        Ok(scene)
    }
}

fn decode_layer(
    root: &Path,
    segment: &SegmentSummary,
    source_us: u64,
) -> Result<VideoFrame, String> {
    let path = safe_path(root, &segment.relative_path)?;
    // Each independently written segment has an AVAsset timeline beginning at zero.
    // Journal source/host/media anchors describe recording time, not the asset time.
    let local_us = source_us
        .checked_sub(segment.start_us)
        .ok_or("Invalid segment timestamp")?;
    decode_h264_frame(&path, local_us).map_err(|error| {
        format!(
            "{} at local {}us: {}",
            segment.relative_path, local_us, error
        )
    })
}

pub fn validate_settings(settings: &ExportSettings) -> Result<(), ExportFailure> {
    let codec = settings.video_codec.to_ascii_lowercase();
    if codec != "h264" {
        return Err(ExportFailure::InvalidSettings {
            message: format!("Unsupported video codec: {}", settings.video_codec),
        });
    }
    if settings.audio_codec.to_ascii_lowercase() != "aac" {
        return Err(ExportFailure::InvalidSettings {
            message: format!("Unsupported audio codec: {}", settings.audio_codec),
        });
    }
    if !ALLOWED_FPS.contains(&settings.fps) {
        return Err(ExportFailure::InvalidSettings {
            message: format!("Unsupported export frame rate: {}", settings.fps),
        });
    }
    if settings.width < 16
        || settings.height < 16
        || settings.width > MAX_FRAME_DIM
        || settings.height > MAX_FRAME_DIM
        || settings.width % 2 != 0
        || settings.height % 2 != 0
    {
        return Err(ExportFailure::InvalidSettings {
            message: "Export canvas must be even, 16..=4096".into(),
        });
    }
    Ok(())
}

pub fn default_export_filename(project_name: &str) -> String {
    let mut out = String::new();
    for ch in project_name.chars() {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            continue;
        }
        out.push(ch);
    }
    let out = out.trim().trim_matches('.').to_string();
    let stem = if out.is_empty() || out == "." || out == ".." {
        "Untitled"
    } else if let Some(stripped) = out
        .strip_suffix(".mp4")
        .or_else(|| out.strip_suffix(".MP4"))
    {
        stripped
    } else {
        &out
    };
    format!("{stem}.mp4")
}

pub fn is_inside_bundle(path: &Path, bundle_root: Option<&Path>) -> bool {
    if let Some(bundle) = bundle_root {
        if let Ok(canonical_bundle) = fs::canonicalize(bundle) {
            if let Some(parent) = path.parent() {
                if let Ok(canonical_parent) = fs::canonicalize(parent) {
                    if canonical_parent.starts_with(&canonical_bundle) {
                        return true;
                    }
                }
            }
            if let Ok(canonical_path) = fs::canonicalize(path) {
                if canonical_path.starts_with(&canonical_bundle) {
                    return true;
                }
            }
        }
        if path.starts_with(bundle) {
            return true;
        }
        if let Some(parent) = path.parent() {
            if parent.starts_with(bundle) {
                return true;
            }
        }
    }

    if let Some(parent) = path.parent() {
        for component in parent.components() {
            let name = component.as_os_str().to_string_lossy();
            if name.to_ascii_lowercase().ends_with(".aero") {
                return true;
            }
        }
    }

    if path
        .to_string_lossy()
        .to_ascii_lowercase()
        .ends_with(".aero")
    {
        return true;
    }

    false
}

pub fn default_destination(project_root: &Path, project_name: &str, _revision: u64) -> PathBuf {
    let parent = project_root.parent().unwrap_or(project_root);
    parent.join(default_export_filename(project_name))
}

pub fn resolve_destination(
    project_root: &Path,
    project_name: &str,
    revision: u64,
    requested: Option<&str>,
    tracks: &[(TrackSummary, Vec<SegmentSummary>)],
) -> Result<PathBuf, ExportFailure> {
    let dest = match requested {
        Some(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => default_destination(project_root, project_name, revision),
    };
    let project_root = fs::canonicalize(project_root).map_err(|e| ExportFailure::Io {
        message: e.to_string(),
    })?;
    if dest.as_os_str().is_empty() {
        return Err(ExportFailure::InvalidSettings {
            message: "Export destination is empty".into(),
        });
    }
    let parent = dest.parent().ok_or_else(|| ExportFailure::Io {
        message: "Export destination has no parent directory".into(),
    })?;
    if !parent.exists() {
        return Err(ExportFailure::Io {
            message: "Export destination directory does not exist".into(),
        });
    }
    if parent.is_file() {
        return Err(ExportFailure::Io {
            message: "Export destination parent is not a directory".into(),
        });
    }
    let canonical_parent = fs::canonicalize(parent).map_err(|e| ExportFailure::Io {
        message: e.to_string(),
    })?;
    let file_name = dest.file_name().ok_or_else(|| ExportFailure::Io {
        message: "Export destination is missing a file name".into(),
    })?;
    let resolved = canonical_parent.join(file_name);
    if resolved.starts_with(&project_root) || is_inside_bundle(&resolved, Some(&project_root)) {
        return Err(ExportFailure::SourcePath {
            message: "Export destination cannot be inside the project bundle".into(),
        });
    }
    for (_track, segments) in tracks {
        for segment in segments {
            if let Ok(path) = safe_path(&project_root, &segment.relative_path) {
                if let (Ok(src), Ok(out)) = (fs::canonicalize(&path), fs::canonicalize(&resolved)) {
                    if src == out {
                        return Err(ExportFailure::SourcePath {
                            message: "Export destination cannot overwrite a source track".into(),
                        });
                    }
                }
                if path == resolved {
                    return Err(ExportFailure::SourcePath {
                        message: "Export destination cannot overwrite a source track".into(),
                    });
                }
            }
        }
    }
    Ok(resolved)
}

pub fn frame_count(duration_us: u64, fps: u32) -> Result<u32, ExportFailure> {
    if duration_us == 0 {
        return Err(ExportFailure::InvalidSettings {
            message: "Edited timeline is empty".into(),
        });
    }
    if !ALLOWED_FPS.contains(&fps) {
        return Err(ExportFailure::InvalidSettings {
            message: "Unsupported frame rate".into(),
        });
    }
    let count =
        u32::try_from((duration_us as u128 * fps as u128).div_ceil(1_000_000)).map_err(|_| {
            ExportFailure::InvalidSettings {
                message: "Timeline exceeds output timestamp bounds".into(),
            }
        })?;
    Ok(count)
}

pub fn frame_time_us(index: u32, fps: u32) -> u64 {
    (index as u128 * 1_000_000 / fps as u128) as u64
}

pub fn recording_blocks_export(state: SessionState) -> bool {
    matches!(
        state,
        SessionState::Preparing
            | SessionState::Recording
            | SessionState::Paused
            | SessionState::Stopping
    )
}

fn status_from(
    job_id: &str,
    document: &EditDocument,
    settings: &ExportSettings,
    state: ExportState,
    failure: Option<ExportFailure>,
) -> ExportStatus {
    ExportStatus {
        job_id: job_id.into(),
        state,
        captured_revision: document.revision,
        video_codec: settings.video_codec.clone(),
        audio_codec: settings.audio_codec.clone(),
        width: settings.width,
        height: settings.height,
        fps: settings.fps,
        progress_numerator: 0,
        progress_denominator: 0,
        output_path: None,
        failure,
        diagnostics: Vec::new(),
    }
}

pub fn prepare_job(
    session_state: SessionState,
    root: &Path,
    project_name: &str,
    document: EditDocument,
    tracks: Vec<(TrackSummary, Vec<SegmentSummary>)>,
    settings: ExportSettings,
    owner: &mut ExportOwner,
) -> Result<CapturedExport, ExportStatus> {
    let job_id = Uuid::new_v4().to_string();
    if recording_blocks_export(session_state) {
        return Err(status_from(
            &job_id,
            &document,
            &settings,
            ExportState::Failed,
            Some(ExportFailure::RecordingActive {
                message: "Export is deferred while a recording session is active".into(),
            }),
        ));
    }
    if owner.busy() {
        return Err(status_from(
            &job_id,
            &document,
            &settings,
            ExportState::Failed,
            Some(ExportFailure::EncoderBusy {
                message: "An export job is already running".into(),
            }),
        ));
    }
    if let Err(failure) = validate_settings(&settings) {
        return Err(status_from(
            &job_id,
            &document,
            &settings,
            ExportState::Failed,
            Some(failure),
        ));
    }
    let mut settings = settings;
    match document.layout.fit_export_size(settings.width, settings.height) {
        Ok((width, height)) => {
            settings.width = width;
            settings.height = height;
        }
        Err(message) => {
            return Err(status_from(
                &job_id,
                &document,
                &settings,
                ExportState::Failed,
                Some(ExportFailure::InvalidSettings { message }),
            ));
        }
    }
    let dest = match resolve_destination(
        root,
        project_name,
        document.revision,
        settings.destination.as_deref(),
        &tracks,
    ) {
        Ok(path) => path,
        Err(failure) => {
            return Err(status_from(
                &job_id,
                &document,
                &settings,
                ExportState::Failed,
                Some(failure),
            ));
        }
    };
    if dest.exists() {
        return Err(status_from(
            &job_id,
            &document,
            &settings,
            ExportState::Failed,
            Some(ExportFailure::Collision {
                message: "Export destination already exists".into(),
            }),
        ));
    }
    let duration = match document.edited_duration_us() {
        Ok(value) => value,
        Err(message) => {
            return Err(status_from(
                &job_id,
                &document,
                &settings,
                ExportState::Failed,
                Some(ExportFailure::InvalidSettings { message }),
            ));
        }
    };
    if let Err(failure) = frame_count(duration, settings.fps) {
        return Err(status_from(
            &job_id,
            &document,
            &settings,
            ExportState::Failed,
            Some(failure),
        ));
    }
    let temp = dest.with_file_name(format!(
        ".{}-aeroshoot-partial-{}.mp4",
        dest.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("export"),
        &job_id[..8.min(job_id.len())]
    ));
    let lease = crate::project::reader::acquire_read_lease(root).map_err(|message| {
        status_from(
            &job_id,
            &document,
            &settings,
            ExportState::Failed,
            Some(ExportFailure::Io { message }),
        )
    })?;
    Ok(CapturedExport {
        lease: Arc::new(lease),
        job_id,
        root: root.to_path_buf(),
        document,
        tracks,
        dest,
        temp,
        settings,
    })
}

pub fn spawn_job(
    captured: CapturedExport,
    owner: &mut ExportOwner,
    gate: Arc<EncoderGate>,
) -> ExportStatus {
    let cancel = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(status_from(
        &captured.job_id,
        &captured.document,
        &captured.settings,
        ExportState::Queued,
        None,
    )));
    let status_clone = Arc::clone(&status);
    let cancel_clone = Arc::clone(&cancel);
    let id = captured.job_id.clone();
    let join = std::thread::spawn(move || {
        run_job(captured, cancel_clone, status_clone, &gate);
    });
    owner.job = Some(LiveJob {
        id,
        cancel,
        status: Arc::clone(&status),
        join: Some(join),
    });
    let snapshot = status.lock().clone();
    snapshot
}

pub fn run_job(
    captured: CapturedExport,
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<ExportStatus>>,
    gate: &EncoderGate,
) {
    {
        let mut slot = status.lock();
        slot.state = ExportState::Running;
    }
    match run_export(
        &captured,
        &cancel,
        |done, total| {
            let mut slot = status.lock();
            slot.progress_numerator = done;
            slot.progress_denominator = total;
            slot.state = ExportState::Running;
        },
        gate,
    ) {
        Ok(path) => {
            let mut slot = status.lock();
            slot.state = ExportState::Completed;
            slot.output_path = Some(path.to_string_lossy().into());
            slot.progress_numerator = slot.progress_denominator.max(1);
            slot.failure = None;
        }
        Err(failure) => {
            let cancelled = matches!(failure, ExportFailure::Cancelled { .. });
            let mut slot = status.lock();
            slot.state = if cancelled {
                ExportState::Cancelled
            } else {
                ExportState::Failed
            };
            slot.failure = Some(failure);
            slot.output_path = None;
        }
    }
}

struct TempGuard {
    path: PathBuf,
    keep: bool,
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn run_export(
    captured: &CapturedExport,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u32, u32),
    gate: &EncoderGate,
) -> Result<PathBuf, ExportFailure> {
    let _slot = gate
        .try_acquire()
        .map_err(|message| ExportFailure::EncoderBusy { message })?;
    if cancel.load(Ordering::SeqCst) {
        return Err(ExportFailure::Cancelled {
            message: "Export cancelled".into(),
        });
    }
    if captured.dest.exists() {
        return Err(ExportFailure::Collision {
            message: "Export destination already exists".into(),
        });
    }
    if fs::symlink_metadata(&captured.temp).is_ok() {
        return Err(ExportFailure::Collision {
            message: "Temporary export path exists".into(),
        });
    }
    let mut temp = TempGuard {
        path: captured.temp.clone(),
        keep: false,
    };
    let duration_us = captured
        .document
        .edited_duration_us()
        .map_err(|message| ExportFailure::InvalidSettings { message })?;
    let frames = frame_count(duration_us, captured.settings.fps)?;
    let mixer = AudioMixer::new(&captured.root, &captured.document, &captured.tracks)
        .map_err(|message| ExportFailure::Io { message })?;
    let (sample_rate, channels) = if mixer.has_audio() {
        (SAMPLE_RATE, CHANNELS)
    } else {
        (0, 0)
    };
    let session = NativeExport::begin(
        &captured.temp,
        captured.settings.width,
        captured.settings.height,
        captured.settings.fps,
        sample_rate,
        channels,
    )
    .map_err(|message| ExportFailure::Native { message })?;
    let mut evaluator = SceneEvaluator::new(
        captured.root.clone(),
        captured.document.clone(),
        captured.tracks.clone(),
        captured.settings.width,
        captured.settings.height,
    )
    .map_err(|message| ExportFailure::Native { message })?;

    let mut audio_frame = 0u64;
    for index in 0..frames {
        if cancel.load(Ordering::SeqCst) {
            drop(session);
            return Err(ExportFailure::Cancelled {
                message: "Export cancelled".into(),
            });
        }
        let pts_us = frame_time_us(index, captured.settings.fps);
        if pts_us >= duration_us {
            break;
        }
        let frame = evaluator
            .preview_at(pts_us)
            .map_err(|message| ExportFailure::Native { message })?;
        session
            .write_video(pts_us, &frame)
            .map_err(|message| ExportFailure::Native { message })?;
        if channels > 0 {
            let end =
                ((index as u128 + 1) * SAMPLE_RATE as u128 / captured.settings.fps as u128) as u64;
            let end = end.min(mixer.total_frames);
            while audio_frame < end {
                if cancel.load(Ordering::SeqCst) {
                    return Err(ExportFailure::Cancelled {
                        message: "Export cancelled".into(),
                    });
                }
                let count = CHUNK_FRAMES.min((end - audio_frame) as usize);
                let chunk = mixer
                    .read_frames(audio_frame, count)
                    .map_err(|message| ExportFailure::Io { message })?;
                let pts = (audio_frame as u128 * 1_000_000 / SAMPLE_RATE as u128) as u64;
                session
                    .write_audio(pts, &chunk, count as u32, CHANNELS)
                    .map_err(|message| ExportFailure::Native { message })?;
                audio_frame += count as u64;
            }
        }
        on_progress(index + 1, frames);
    }
    session
        .finish(duration_us)
        .map_err(|message| ExportFailure::Native { message })?;
    if cancel.load(Ordering::SeqCst) {
        return Err(ExportFailure::Cancelled {
            message: "Export cancelled".into(),
        });
    }
    let actual_duration =
        media_duration_us(&captured.temp).map_err(|message| ExportFailure::Native { message })?;
    if actual_duration.abs_diff(duration_us) > AUDIO_DURATION_SLACK_US {
        return Err(ExportFailure::Native {
            message: format!(
                "Output duration {} differs from {}",
                actual_duration, duration_us
            ),
        });
    }
    for pts in [0, frame_time_us(frames - 1, captured.settings.fps)] {
        let decoded = decode_h264_frame(&captured.temp, pts)
            .map_err(|message| ExportFailure::Native { message })?;
        if (decoded.width, decoded.height) != (captured.settings.width, captured.settings.height) {
            return Err(ExportFailure::Native {
                message: "Output dimensions do not match settings".into(),
            });
        }
    }
    if cancel.load(Ordering::SeqCst) {
        return Err(ExportFailure::Cancelled {
            message: "Export cancelled".into(),
        });
    }
    publish_output(&captured.temp, &captured.dest)?;
    temp.keep = true;
    Ok(captured.dest.clone())
}

fn publish_output(temp: &Path, dest: &Path) -> Result<(), ExportFailure> {
    if dest.exists() {
        let _ = fs::remove_file(temp);
        return Err(ExportFailure::Collision {
            message: "Export destination already exists".into(),
        });
    }
    {
        let file = File::open(temp).map_err(|e| ExportFailure::Io {
            message: e.to_string(),
        })?;
        file.sync_all().map_err(|e| ExportFailure::Io {
            message: e.to_string(),
        })?;
    }
    match fs::hard_link(temp, dest) {
        Ok(()) => {
            let _ = fs::remove_file(temp);
            Ok(())
        }
        Err(_) if dest.exists() => {
            let _ = fs::remove_file(temp);
            Err(ExportFailure::Collision {
                message: "Export destination already exists".into(),
            })
        }
        Err(_err) => {
            if dest.exists() {
                let _ = fs::remove_file(temp);
                return Err(ExportFailure::Collision {
                    message: "Export destination already exists".into(),
                });
            }
            #[cfg(target_os = "macos")]
            {
                use std::os::unix::ffi::OsStrExt;
                let from = std::ffi::CString::new(temp.as_os_str().as_bytes()).map_err(|e| {
                    ExportFailure::Io {
                        message: e.to_string(),
                    }
                })?;
                let to = std::ffi::CString::new(dest.as_os_str().as_bytes()).map_err(|e| {
                    ExportFailure::Io {
                        message: e.to_string(),
                    }
                })?;
                if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
                    return Ok(());
                }
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    return Err(ExportFailure::Collision {
                        message: "Export destination already exists".into(),
                    });
                }
                Err(ExportFailure::Io {
                    message: error.to_string(),
                })
            }
            #[cfg(not(target_os = "macos"))]
            Err(ExportFailure::Io {
                message: _err.to_string(),
            })
        }
    }
}

pub fn compare_preview_and_export(
    preview: &VideoFrame,
    exported: &VideoFrame,
) -> Result<(u8, f32, f32, bool), String> {
    let (max_abs_delta, mean_abs_delta) = compare_frames(preview, exported)?;
    let region = crate::media::region_mean_delta(preview, exported);
    let matched = mean_abs_delta <= PARITY_MEAN_TOLERANCE && region <= PARITY_REGION_MEAN_TOLERANCE;
    Ok((max_abs_delta, mean_abs_delta, region, matched))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_prores_and_odd_dimensions() {
        let mut settings = ExportSettings::default();
        settings.video_codec = "prores".into();
        assert!(matches!(
            validate_settings(&settings),
            Err(ExportFailure::InvalidSettings { .. })
        ));
        settings.video_codec = "h264".into();
        settings.width = 63;
        assert!(matches!(
            validate_settings(&settings),
            Err(ExportFailure::InvalidSettings { .. })
        ));
    }

    #[test]
    fn exclusive_end_is_not_an_output_sample() {
        assert_eq!(frame_count(200_000, 10).unwrap(), 2);
        assert_eq!(frame_count(250_000, 10).unwrap(), 3);
        assert_eq!(frame_count(1, 30).unwrap(), 1);
        assert!(frame_count(u64::MAX, 60).is_err());
        assert_eq!(frame_time_us(0, 10), 0);
        assert_eq!(frame_time_us(1, 10), 100_000);
        assert!(frame_time_us(2, 10) >= 200_000);
    }

    #[test]
    fn status_json_omits_pixels() {
        let json = serde_json::to_value(ExportStatus::idle()).unwrap();
        assert!(json.get("pixels").is_none());
        assert!(json.get("samples").is_none());
        assert_eq!(json["state"], "idle");
    }
}
