//! Routes a native recorder's finished segments and runtime errors into the
//! active session. Platform bridges call in from their own capture threads;
//! the targets are installed per session by `capture::backend`.
use crate::project::journal::ProjectJournal;
use crate::project::manifest::TrackType;
use crate::project::segment_writer::TrackSegmentWriter;
use crate::session::{
    SessionDiagnostics, SessionEpoch, SessionEvent, SessionState, SessionStateMachine,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

static RUNTIME_ERROR_TARGET: Mutex<Option<RuntimeErrorTarget>> = Mutex::new(None);
static SEGMENT_TARGET: Mutex<Option<SegmentTarget>> = Mutex::new(None);

#[derive(Clone)]
struct RuntimeErrorTarget {
    diagnostics: Arc<SessionDiagnostics>,
    state_machine: Arc<SessionStateMachine>,
    epoch: SessionEpoch,
}

#[derive(Clone)]
struct SegmentTarget {
    diagnostics: Arc<SessionDiagnostics>,
    epoch: SessionEpoch,
    journal: Option<Arc<ProjectJournal>>,
    project_root: Option<PathBuf>,
    /// Long-lived per-track writers so a journal failure after publish can
    /// be retried. A throwaway writer would drop `pending_publication`.
    writers: Arc<Mutex<HashMap<String, TrackSegmentWriter>>>,
    closed: Arc<AtomicBool>,
}

/// Install the callback targets for the duration of a session, before the
/// first segment can be committed.
pub(crate) fn install_callback_targets(
    diagnostics: Arc<SessionDiagnostics>,
    state_machine: Arc<SessionStateMachine>,
    epoch: SessionEpoch,
    journal: Option<Arc<ProjectJournal>>,
    project_root: Option<PathBuf>,
) {
    *RUNTIME_ERROR_TARGET
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(RuntimeErrorTarget {
        diagnostics: diagnostics.clone(),
        state_machine: state_machine.clone(),
        epoch: epoch.clone(),
    });
    *SEGMENT_TARGET
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(SegmentTarget {
        diagnostics,
        epoch,
        journal,
        project_root,
        writers: Arc::new(Mutex::new(HashMap::new())),
        closed: Arc::new(AtomicBool::new(false)),
    });
}

/// Move the session clock used by native callbacks once recording has begun.
pub(crate) fn set_callback_epoch(epoch: SessionEpoch) {
    if let Some(target) = RUNTIME_ERROR_TARGET
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        target.epoch = epoch.clone();
    }
    if let Some(target) = SEGMENT_TARGET
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        target.epoch = epoch;
    }
}

/// Drain native publication writers before clearing callback targets.
/// Sets `closed` first so a racing callback cannot recreate a throwaway writer.
pub(crate) fn take_native_segment_writers() -> Vec<TrackSegmentWriter> {
    let guard = SEGMENT_TARGET
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let Some(target) = guard.as_ref() else {
        return Vec::new();
    };
    target.closed.store(true, Ordering::SeqCst);
    let mut writers = target
        .writers
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    writers.drain().map(|(_, writer)| writer).collect()
}

/// Clear the callback targets. Safe to call from any thread.
pub(crate) fn clear_callback_targets() {
    *RUNTIME_ERROR_TARGET
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = None;
    *SEGMENT_TARGET
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = None;
}

/// Validate, publish, and journal one finalized native segment at
/// `media/<track>/<index + 1>.<ext>.tmp`. Called synchronously by the
/// recorder after its container is closed; `Err` means the recorder must
/// treat the track as failed rather than as committed.
pub(crate) fn publish_segment(
    track_id: &str,
    index: u32,
    host_anchor_us: i64,
    timescale: u32,
    media_start_value: i64,
    path: &Path,
) -> Result<(), ()> {
    let target = SEGMENT_TARGET
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let Some(target) = target else {
        return Err(());
    };
    if target.closed.load(Ordering::SeqCst) {
        return Err(());
    }
    let (Some(journal), Some(root)) = (&target.journal, &target.project_root) else {
        return Err(());
    };
    let kind = match track_id {
        "screen" => TrackType::Screen,
        "webcam" => TrackType::Webcam,
        "system" => TrackType::SystemAudio,
        "mic" => TrackType::MicAudio,
        _ => return Err(()),
    };
    let mut writers = target
        .writers
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if target.closed.load(Ordering::SeqCst) {
        return Err(());
    }
    let writer = writers.entry(track_id.to_string()).or_insert_with(|| {
        TrackSegmentWriter::new(root, track_id.to_string(), kind, String::new())
    });
    match writer.commit_native_segment(
        path,
        index,
        host_anchor_us,
        timescale,
        media_start_value,
        target.epoch.current_elapsed_us(),
        journal,
    ) {
        Ok(_) => {
            drop(writers);
            target.diagnostics.apply(&SessionEvent::SegmentRotated {
                track_id: track_id.to_string(),
                segment_index: index,
                host_anchor_us,
                media_timescale: timescale,
                media_start_value,
            });
            Ok(())
        }
        Err(error) => {
            drop(writers);
            target.diagnostics.apply(&SessionEvent::RuntimeError {
                track_id: track_id.to_string(),
                error_code: -600,
                message: error.to_string(),
                t_us: target.epoch.current_elapsed_us(),
                recoverable: true,
            });
            Err(())
        }
    }
}

/// Record a native runtime error. Negative codes are non-recoverable and move
/// the session to `Error`.
pub(crate) fn report_runtime_error(track_id: String, error_code: i32, message: String) {
    let target = RUNTIME_ERROR_TARGET
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(target) = target {
        let event = SessionEvent::RuntimeError {
            track_id,
            error_code,
            message,
            t_us: target.epoch.current_elapsed_us(),
            recoverable: error_code >= 0,
        };
        if let Some(record) = target.diagnostics.apply(&event) {
            if !record.recoverable {
                let _ = target.state_machine.transition_to(SessionState::Error);
            }
        }
    }
}

/// Record a recoverable problem without changing session state.
#[cfg(target_os = "macos")]
pub(crate) fn report_recoverable_error(track_id: &str, error_code: i32, message: String) {
    if let Some(target) = RUNTIME_ERROR_TARGET
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
    {
        target.diagnostics.apply(&SessionEvent::RuntimeError {
            track_id: track_id.into(),
            error_code,
            message,
            t_us: target.epoch.current_elapsed_us(),
            recoverable: true,
        });
    }
}
