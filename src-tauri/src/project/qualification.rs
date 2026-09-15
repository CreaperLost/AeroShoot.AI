//! Durable post-stop capture qualification summary.

use super::{JournalRecord, ProjectManifest, TrackType};
use crate::telemetry::{CanonicalKind, TelemetryStream};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;

pub const QUALIFICATION_SCHEMA_VERSION: u32 = 2;
pub const MAX_SEGMENT_DURATION_US: u64 = 60_000_000;
/// A selected track that ends this far before the active session duration is
/// treated as stalled. This matches the live health UI's two-second timeout.
pub const MAX_TRACK_TAIL_GAP_US: u64 = 2_000_000;
/// Independent tracks must finish within this tolerance of one another.
pub const MAX_TRACK_END_SKEW_US: u64 = 2_000_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QualificationTrackReport {
    pub track_id: String,
    pub track_type: TrackType,
    pub segment_count: u64,
    pub total_bytes: u64,
    pub first_start_us: Option<u64>,
    pub last_end_us: Option<u64>,
    pub maximum_segment_duration_us: u64,
    pub gaps_total: u64,
    pub tail_gap_us: u64,
    pub stalled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QualificationFailure {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QualificationRuntimeError {
    pub track_id: String,
    pub error_code: i32,
    pub message: String,
    pub t_us: u64,
    pub recoverable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureQualificationReport {
    pub schema_version: u32,
    pub generated_at: String,
    pub session_id: String,
    pub duration_us: u64,
    pub active_duration_us: u64,
    pub segment_duration_limit_us: u64,
    pub track_tail_gap_limit_us: u64,
    pub track_end_skew_limit_us: u64,
    pub maximum_track_end_skew_us: u64,
    pub gaps_total: u64,
    pub passed: bool,
    pub failures: Vec<QualificationFailure>,
    pub tracks: Vec<QualificationTrackReport>,
    pub runtime_errors: Vec<QualificationRuntimeError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mouse_telemetry: Option<MouseQualificationReport>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MouseQualificationReport {
    pub usable: bool,
    pub geometry_count: u64,
    pub supported_geometry_count: u64,
    pub event_count: u64,
    pub move_count: u64,
    pub button_transition_count: u64,
    pub scroll_count: u64,
    pub gap_count: u64,
    pub dropped_events: u64,
    pub diagnostics: Vec<String>,
}

#[derive(Default)]
struct TrackAccumulator {
    segment_count: u64,
    total_bytes: u64,
    first_start_us: Option<u64>,
    last_end_us: Option<u64>,
    maximum_segment_duration_us: u64,
}

pub fn build_capture_qualification_report(
    manifest: &ProjectManifest,
    records: &[JournalRecord],
    mouse_stream: Option<&TelemetryStream>,
) -> CaptureQualificationReport {
    let mut media: HashMap<&str, TrackAccumulator> = HashMap::new();
    let mut gaps: HashMap<&str, u64> = HashMap::new();
    let mut runtime_errors = Vec::new();
    for record in records {
        match record {
            JournalRecord::SegmentCommitted {
                track_id,
                start_us,
                end_us,
                size_bytes,
                ..
            } => {
                let entry = media.entry(track_id).or_default();
                entry.segment_count = entry.segment_count.saturating_add(1);
                entry.total_bytes = entry.total_bytes.saturating_add(*size_bytes);
                entry.first_start_us = Some(
                    entry
                        .first_start_us
                        .map_or(*start_us, |current| current.min(*start_us)),
                );
                entry.last_end_us = Some(
                    entry
                        .last_end_us
                        .map_or(*end_us, |current| current.max(*end_us)),
                );
                entry.maximum_segment_duration_us = entry
                    .maximum_segment_duration_us
                    .max(end_us.saturating_sub(*start_us));
            }
            JournalRecord::RuntimeError {
                track_id,
                error_code,
                message,
                t_us,
                recoverable,
                ..
            } => runtime_errors.push(QualificationRuntimeError {
                track_id: track_id.clone(),
                error_code: *error_code,
                message: message.clone(),
                t_us: *t_us,
                recoverable: *recoverable,
            }),
            JournalRecord::Discontinuity { track_id, .. } => {
                let count = gaps.entry(track_id).or_default();
                *count = count.saturating_add(1);
            }
            _ => {}
        }
    }

    let tracks: Vec<_> = manifest
        .tracks
        .iter()
        .map(|track| {
            let captured = media.remove(track.id.as_str()).unwrap_or_default();
            let tail_gap_us = captured
                .last_end_us
                .map_or(manifest.active_duration_us, |end| {
                    manifest.active_duration_us.saturating_sub(end)
                });
            let gaps_total = gaps.remove(track.id.as_str()).unwrap_or(track.gaps_total);
            QualificationTrackReport {
                track_id: track.id.clone(),
                track_type: track.track_type,
                segment_count: captured.segment_count,
                total_bytes: captured.total_bytes,
                first_start_us: captured.first_start_us,
                last_end_us: captured.last_end_us,
                maximum_segment_duration_us: captured.maximum_segment_duration_us,
                gaps_total,
                tail_gap_us,
                stalled: gaps_total > 0 || tail_gap_us > MAX_TRACK_TAIL_GAP_US,
            }
        })
        .collect();
    let ends: Vec<u64> = tracks
        .iter()
        .filter_map(|track| track.last_end_us)
        .collect();
    let maximum_track_end_skew_us = ends
        .iter()
        .max()
        .zip(ends.iter().min())
        .map_or(0, |(maximum, minimum)| maximum.saturating_sub(*minimum));
    let mut failures = Vec::new();
    for track in &tracks {
        if track.segment_count == 0 || track.total_bytes == 0 {
            failures.push(QualificationFailure {
                code: "missing_media".into(),
                message: "Enabled track produced no committed media".into(),
                track_id: Some(track.track_id.clone()),
            });
        }
        if track.maximum_segment_duration_us > MAX_SEGMENT_DURATION_US {
            failures.push(QualificationFailure {
                code: "segment_too_long".into(),
                message: format!("Segment exceeded {} microseconds", MAX_SEGMENT_DURATION_US),
                track_id: Some(track.track_id.clone()),
            });
        }
        if track.stalled {
            failures.push(QualificationFailure {
                code: "track_stalled".into(),
                message: format!(
                    "Track has {} discontinuities and a {} microsecond final tail",
                    track.gaps_total, track.tail_gap_us
                ),
                track_id: Some(track.track_id.clone()),
            });
        }
    }
    for error in &runtime_errors {
        failures.push(QualificationFailure {
            code: "runtime_error".into(),
            message: format!(
                "Native capture error {}: {}",
                error.error_code, error.message
            ),
            track_id: Some(error.track_id.clone()),
        });
    }
    if maximum_track_end_skew_us > MAX_TRACK_END_SKEW_US {
        failures.push(QualificationFailure {
            code: "track_end_skew".into(),
            message: format!(
                "Track end skew of {} microseconds exceeded the {} microsecond limit",
                maximum_track_end_skew_us, MAX_TRACK_END_SKEW_US
            ),
            track_id: None,
        });
    }
    let passed = failures.is_empty();
    let mouse_telemetry = mouse_stream.map(|stream| {
        let mut move_count = 0u64;
        let mut button_transition_count = 0u64;
        let mut scroll_count = 0u64;
        let mut gap_count = 0u64;
        let mut dropped_events = 0u64;
        for event in &stream.events {
            match &event.kind {
                CanonicalKind::Move { .. } => move_count = move_count.saturating_add(1),
                CanonicalKind::ButtonDown { .. } | CanonicalKind::ButtonUp { .. } => {
                    button_transition_count = button_transition_count.saturating_add(1)
                }
                CanonicalKind::Scroll { .. } => scroll_count = scroll_count.saturating_add(1),
                CanonicalKind::Gap {
                    dropped_events: dropped,
                    ..
                } => {
                    gap_count = gap_count.saturating_add(1);
                    dropped_events = dropped_events.saturating_add(*dropped);
                }
                CanonicalKind::Click { .. } => {
                    button_transition_count = button_transition_count.saturating_add(1)
                }
                // Shape changes carry no pointer position or input.
                CanonicalKind::CursorChanged { .. } => {}
            }
        }
        let supported_geometry_count = stream
            .geometries
            .values()
            .filter(|geometry| geometry.supported)
            .count() as u64;
        MouseQualificationReport {
            usable: supported_geometry_count > 0
                && (move_count > 0 || button_transition_count > 0 || scroll_count > 0),
            geometry_count: stream.geometries.len() as u64,
            supported_geometry_count,
            event_count: stream.events.len() as u64,
            move_count,
            button_transition_count,
            scroll_count,
            gap_count,
            dropped_events,
            diagnostics: stream.diagnostics.clone(),
        }
    });

    CaptureQualificationReport {
        schema_version: QUALIFICATION_SCHEMA_VERSION,
        generated_at: chrono::Utc::now().to_rfc3339(),
        session_id: manifest.session_id.clone(),
        duration_us: manifest.duration_us,
        active_duration_us: manifest.active_duration_us,
        segment_duration_limit_us: MAX_SEGMENT_DURATION_US,
        track_tail_gap_limit_us: MAX_TRACK_TAIL_GAP_US,
        track_end_skew_limit_us: MAX_TRACK_END_SKEW_US,
        maximum_track_end_skew_us,
        gaps_total: tracks.iter().map(|track| track.gaps_total).sum(),
        passed,
        failures,
        tracks,
        runtime_errors,
        mouse_telemetry,
    }
}

pub fn save_capture_qualification_report(
    root: &Path,
    report: &CaptureQualificationReport,
) -> Result<(), String> {
    let path = root.join("qualification.json");
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("qualification.json cannot be a symlink".into());
    }
    let temporary = root.join(format!(".qualification.{}.tmp", uuid::Uuid::new_v4()));
    let serialized = serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    if let Err(error) = file.write_all(&serialized).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    drop(file);
    if let Err(error) = fs::rename(&temporary, &path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}
