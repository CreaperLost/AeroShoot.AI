use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Domain events that the capture pipeline can publish to the session state
/// machine. Runtime errors are the most critical; a single misbehaving track
/// must not silently corrupt the rest of the project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    /// The native capture pipeline reported an error on a specific track.
    /// `recoverable = true` means the session can continue (e.g. a transient
    /// device hiccup); `false` means the track is no longer trustworthy.
    RuntimeError {
        track_id: String,
        error_code: i32,
        message: String,
        t_us: u64,
        recoverable: bool,
    },
    /// The capture pipeline rotated a segment on disk and the host-clock
    /// anchor is now available. This is informational; downstream
    /// diagnostics consume it to update gaps_total / media_timescale
    /// counters in the manifest.
    SegmentRotated {
        track_id: String,
        segment_index: u32,
        host_anchor_us: i64,
        media_timescale: u32,
        media_start_value: i64,
    },
}

/// Concrete record of the most recent runtime error so that
/// `get_session_status` can surface it to the UI without holding a
/// borrow on the state machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeErrorRecord {
    pub track_id: String,
    pub error_code: i32,
    pub message: String,
    pub t_us: u64,
    pub recoverable: bool,
}

impl From<&SessionEvent> for Option<RuntimeErrorRecord> {
    fn from(event: &SessionEvent) -> Self {
        match event {
            SessionEvent::RuntimeError {
                track_id,
                error_code,
                message,
                t_us,
                recoverable,
            } => Some(RuntimeErrorRecord {
                track_id: track_id.clone(),
                error_code: *error_code,
                message: message.clone(),
                t_us: *t_us,
                recoverable: *recoverable,
            }),
            SessionEvent::SegmentRotated { .. } => None,
        }
    }
}

/// Thread-safe diagnostics bag for the active session. Mirrors fields the
/// Tauri command `get_session_status` exposes alongside the state machine's
/// `SessionState`.
///
/// Cheap to clone (uses `Arc` internally) and designed to be updated from
/// FFI threads — the writer side takes a brief write lock.
#[derive(Debug, Clone, Default)]
pub struct SessionDiagnostics {
    inner: Arc<RwLock<SessionDiagnosticsInner>>,
}

#[derive(Debug, Clone, Default)]
struct SessionDiagnosticsInner {
    last_runtime_error: Option<RuntimeErrorRecord>,
    last_runtime_error_t_us: u64,
    gaps_total: u64,
}

impl SessionDiagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply an event to the diagnostics. Returns the runtime error
    /// record (if any) so the caller can transition the state machine
    /// without taking a second lock.
    pub fn apply(&self, event: &SessionEvent) -> Option<RuntimeErrorRecord> {
        let mut guard = self.inner.write();
        match event {
            SessionEvent::RuntimeError {
                track_id,
                error_code,
                message,
                t_us,
                recoverable,
            } => {
                let record = RuntimeErrorRecord {
                    track_id: track_id.clone(),
                    error_code: *error_code,
                    message: message.clone(),
                    t_us: *t_us,
                    recoverable: *recoverable,
                };
                guard.last_runtime_error = Some(record.clone());
                guard.last_runtime_error_t_us = *t_us;
                guard.gaps_total = guard.gaps_total.saturating_add(1);
                Some(record)
            }
            SessionEvent::SegmentRotated { .. } => None,
        }
    }

    pub fn last_runtime_error(&self) -> Option<RuntimeErrorRecord> {
        self.inner.read().last_runtime_error.clone()
    }

    pub fn last_runtime_error_t_us(&self) -> u64 {
        self.inner.read().last_runtime_error_t_us
    }

    pub fn gaps_total(&self) -> u64 {
        self.inner.read().gaps_total
    }

    /// Reset all diagnostics — used by `start_recording` so a fresh
    /// session does not inherit the previous session's errors.
    pub fn reset(&self) {
        let mut guard = self.inner.write();
        guard.last_runtime_error = None;
        guard.last_runtime_error_t_us = 0;
        guard.gaps_total = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_error_event_round_trip() {
        let diag = SessionDiagnostics::new();
        let event = SessionEvent::RuntimeError {
            track_id: "screen".into(),
            error_code: 42,
            message: "encoder dropped frames".into(),
            t_us: 1_000_000,
            recoverable: true,
        };
        let record = diag.apply(&event).expect("runtime error yields a record");
        assert_eq!(record.track_id, "screen");
        assert_eq!(record.error_code, 42);
        assert!(record.recoverable);

        // Diagnostics expose the most-recent error.
        let got = diag.last_runtime_error().unwrap();
        assert_eq!(got.message, "encoder dropped frames");
        assert_eq!(diag.gaps_total(), 1, "gaps_total increments on every error");
    }

    #[test]
    fn test_segment_rotated_does_not_record_error() {
        let diag = SessionDiagnostics::new();
        let event = SessionEvent::SegmentRotated {
            track_id: "screen".into(),
            segment_index: 7,
            host_anchor_us: 1_500_000,
            media_timescale: 90_000,
            media_start_value: 0,
        };
        assert!(diag.apply(&event).is_none());
        assert!(diag.last_runtime_error().is_none());
        // Segment rotations do not bump the gaps counter.
        assert_eq!(diag.gaps_total(), 0);
    }

    #[test]
    fn test_diagnostics_reset() {
        let diag = SessionDiagnostics::new();
        diag.apply(&SessionEvent::RuntimeError {
            track_id: "mic".into(),
            error_code: 3,
            message: "device unplugged".into(),
            t_us: 5_000,
            recoverable: false,
        });
        assert_eq!(diag.gaps_total(), 1);

        diag.reset();
        assert!(diag.last_runtime_error().is_none());
        assert_eq!(diag.gaps_total(), 0);
    }
}
