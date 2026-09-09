use crate::project::journal::{JournalRecord, ProjectJournal};
use crate::project::lock::{LockError, ProjectLock};
use crate::project::manifest::{PauseInterval, ProjectManifest, TrackDescriptor, TrackType};
use crate::project::media_validator::MediaValidator;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RecoveryError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Lock error: {0}")]
    Lock(#[from] LockError),
    #[error("Project directory does not exist: {0}")]
    MissingProjectDir(PathBuf),
    #[error("Manifest error: {0}")]
    Manifest(String),
    #[error("Journal error: {0}")]
    Journal(String),
    #[error("Invalid path in project: {0}")]
    InvalidPath(String),
}

#[derive(Debug, Clone)]
pub struct TrackRecoveryReport {
    pub track_id: String,
    pub valid_segments: usize,
    pub unindexed_recovered_segments: usize,
    pub missing_segments: usize,
    pub total_recoverable_bytes: u64,
    pub max_timestamp_us: u64,
}

#[derive(Debug, Clone)]
pub struct ProjectRecoveryReport {
    pub session_id: String,
    pub total_journal_entries: usize,
    pub track_reports: HashMap<String, TrackRecoveryReport>,
    pub recoverable_duration_us: u64,
    pub active_duration_us: u64,
    pub pause_intervals: Vec<PauseInterval>,
    pub recovered_manifest: ProjectManifest,
}

pub struct RecoveryEngine;

impl RecoveryEngine {
    /// Scans an `.aero` project bundle directory, parses journal.jsonl,
    /// repairs any truncated journal tail from crash termination,
    /// verifies each referenced media segment on disk with media structure validation,
    /// discovers unindexed committed files while strictly enforcing path containment,
    /// recovers actual sample/packet timing and persists a usable segment index,
    /// reconstructs pause intervals, and produces a reconciled manifest.
    pub fn scan_and_recover<P: AsRef<Path>>(
        project_dir: P,
    ) -> Result<ProjectRecoveryReport, RecoveryError> {
        let dir = project_dir.as_ref();
        if !dir.exists() {
            return Err(RecoveryError::MissingProjectDir(dir.to_path_buf()));
        }

        let canonical_dir = dir.canonicalize().map_err(|e| RecoveryError::Io(e))?;

        // Acquire exclusive project lock before scanning or rewriting snapshots
        let _lock = ProjectLock::acquire(&canonical_dir)?;

        // Read and strictly validate baseline manifest if present
        let manifest_path = canonical_dir.join("manifest.json");
        let manifest: ProjectManifest = if manifest_path.exists() {
            let data = fs::read_to_string(&manifest_path)?;
            let m: ProjectManifest = serde_json::from_str(&data)
                .map_err(|e| RecoveryError::Manifest(format!("JSON parse error: {}", e)))?;
            m.validate()
                .map_err(|e| RecoveryError::Manifest(format!("Validation error: {}", e)))?;
            m
        } else {
            ProjectManifest::new(
                "recovered-session".into(),
                canonical_dir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
            )
        };

        // Repair truncated tail in journal before reading records
        let journal_path = canonical_dir.join("journal.jsonl");
        if journal_path.exists() {
            let _ = ProjectJournal::repair_truncated_tail(&journal_path);
        }

        // Read journal records; propagate errors rather than silently swallowing corruption
        let journal_records = if journal_path.exists() {
            ProjectJournal::read_records_from_path(&journal_path)
                .map_err(|e| RecoveryError::Journal(e.to_string()))?
        } else {
            Vec::new()
        };

        let mut track_reports: HashMap<String, TrackRecoveryReport> = HashMap::new();
        let mut indexed_relative_paths = HashSet::new();
        let mut global_max_us: u64 = 0;
        let mut pause_intervals = Vec::new();

        // 1. Process journal records
        for record in &journal_records {
            match record {
                JournalRecord::SegmentCommitted {
                    track_id,
                    relative_path,
                    end_us,
                    size_bytes,
                    ..
                } => {
                    // Security check: ensure path stays within project root
                    ProjectManifest::validate_path_in_root(&canonical_dir, relative_path)
                        .map_err(|e| RecoveryError::InvalidPath(e.to_string()))?;

                    indexed_relative_paths.insert(relative_path.clone());

                    let report = track_reports.entry(track_id.clone()).or_insert_with(|| {
                        TrackRecoveryReport {
                            track_id: track_id.clone(),
                            valid_segments: 0,
                            unindexed_recovered_segments: 0,
                            missing_segments: 0,
                            total_recoverable_bytes: 0,
                            max_timestamp_us: 0,
                        }
                    });

                    let segment_file = canonical_dir.join(relative_path);
                    if segment_file.exists() && segment_file.is_file() {
                        let actual_len = segment_file.metadata().map(|m| m.len()).unwrap_or(0);
                        let track_type = determine_track_type(track_id, relative_path);

                        // Validate binary container structure (reject zero-filled dummies & corrupt boxes)
                        if actual_len >= *size_bytes
                            && actual_len > 0
                            && MediaValidator::validate(&segment_file, track_type).is_ok()
                        {
                            report.valid_segments += 1;
                            report.total_recoverable_bytes += actual_len;
                            if *end_us > report.max_timestamp_us {
                                report.max_timestamp_us = *end_us;
                            }
                            if *end_us > global_max_us {
                                global_max_us = *end_us;
                            }
                        } else {
                            report.missing_segments += 1;
                        }
                    } else {
                        report.missing_segments += 1;
                    }
                }
                JournalRecord::PauseEnded {
                    start_us, end_us, ..
                } => {
                    pause_intervals.push(PauseInterval {
                        start_us: *start_us,
                        end_us: *end_us,
                    });
                }
                _ => {}
            }
        }

        // 2. Scan filesystem for unindexed committed segment files (crash between rename and journal append)
        // Strictly verify containment: reject symlinks and external escapes before opening
        let media_dir = canonical_dir.join("media");
        let mut recovered_unindexed_records = Vec::new();

        if media_dir.exists() && media_dir.is_dir() {
            // Check that media_dir itself is not a symlink
            let is_media_symlink = fs::symlink_metadata(&media_dir)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);

            if !is_media_symlink {
                if let Ok(entries) = fs::read_dir(&media_dir) {
                    for track_entry in entries.flatten() {
                        let track_path = track_entry.path();

                        // Containment check: reject symlinked track directories
                        let is_track_symlink = fs::symlink_metadata(&track_path)
                            .map(|m| m.file_type().is_symlink())
                            .unwrap_or(false);
                        if is_track_symlink {
                            continue;
                        }

                        if let Ok(canonical_track) = track_path.canonicalize() {
                            if !canonical_track.starts_with(&canonical_dir) {
                                continue;
                            }
                        } else {
                            continue;
                        }

                        if track_path.is_dir() {
                            let track_name = track_entry.file_name().to_string_lossy().to_string();
                            if let Ok(segment_entries) = fs::read_dir(&track_path) {
                                for seg in segment_entries.flatten() {
                                    let seg_path = seg.path();

                                    // Containment check: reject symlinked segment files
                                    let is_seg_symlink = fs::symlink_metadata(&seg_path)
                                        .map(|m| m.file_type().is_symlink())
                                        .unwrap_or(false);
                                    if is_seg_symlink {
                                        continue;
                                    }

                                    if let Ok(canonical_seg) = seg_path.canonicalize() {
                                        if !canonical_seg.starts_with(&canonical_dir) {
                                            continue;
                                        }
                                    } else {
                                        continue;
                                    }

                                    let ext = seg_path
                                        .extension()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .to_lowercase();
                                    // Detect committed segments vs. temp segments. A `*.tmp` file
                                    // is a half-written segment from a crashed capture session;
                                    // a `*.mp4` / `*.wav` is a previously committed file. We
                                    // salvage both, but temp files get renamed to a stable
                                    // committed name after they pass validation.
                                    let lower_file_name = seg_path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_default();
                                    let is_temp = lower_file_name.ends_with(".tmp");

                                    if ext == "mp4" || ext == "wav" {
                                        let file_name = seg_path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy();
                                        let rel_path =
                                            format!("media/{}/{}", track_name, file_name);

                                        // Path security validation
                                        if ProjectManifest::validate_path_in_root(
                                            &canonical_dir,
                                            &rel_path,
                                        )
                                        .is_err()
                                        {
                                            continue;
                                        }

                                        // If not in journal, we discovered an unindexed committed file!
                                        if !indexed_relative_paths.contains(&rel_path) {
                                            let track_type =
                                                determine_track_type(&track_name, &rel_path);
                                            if let Ok(info) =
                                                MediaValidator::validate(&seg_path, track_type)
                                            {
                                                let report = track_reports
                                                    .entry(track_name.clone())
                                                    .or_insert_with(|| TrackRecoveryReport {
                                                        track_id: track_name.clone(),
                                                        valid_segments: 0,
                                                        unindexed_recovered_segments: 0,
                                                        missing_segments: 0,
                                                        total_recoverable_bytes: 0,
                                                        max_timestamp_us: 0,
                                                    });

                                                report.unindexed_recovered_segments += 1;
                                                report.total_recoverable_bytes += info.size_bytes;

                                                // Recover packet/sample timing instead of inventing 2 seconds
                                                let seg_start_us = if info.start_us > 0 {
                                                    info.start_us
                                                } else {
                                                    report.max_timestamp_us
                                                };
                                                let seg_end_us = seg_start_us + info.duration_us;
                                                report.max_timestamp_us = seg_end_us;
                                                if seg_end_us > global_max_us {
                                                    global_max_us = seg_end_us;
                                                }

                                                recovered_unindexed_records.push((
                                                    track_name.clone(),
                                                    track_type,
                                                    rel_path,
                                                    seg_start_us,
                                                    seg_end_us,
                                                    info.size_bytes,
                                                    info.media_timescale,
                                                    info.media_start_value,
                                                    info.host_anchor_us,
                                                ));
                                            }
                                        }
                                    } else if is_temp {
                                        // `*.mp4.tmp` or `*.wav.tmp` — half-written segment
                                        // from a crashed capture session. The MediaValidator is
                                        // strong enough to reject demux failures, container
                                        // corruption, zero-filled dummies, and non-keyframe
                                        // starts, so any file that passes is salvageable.
                                        let base_stem = lower_file_name
                                            .strip_suffix(".tmp")
                                            .unwrap_or(&lower_file_name)
                                            .to_string();
                                        // base_stem is e.g. "000003.mp4" or "000003.wav";
                                        // derive the committed file name by stripping the
                                        // container extension.
                                        let container_ext = if base_stem.ends_with(".mp4") {
                                            "mp4"
                                        } else if base_stem.ends_with(".wav") {
                                            "wav"
                                        } else {
                                            // Unknown container suffix; skip rather than guess.
                                            continue;
                                        };
                                        let seq_str = base_stem
                                            .strip_suffix(&format!(".{}", container_ext))
                                            .unwrap_or(&base_stem);
                                        let new_filename = format!("{}.{}", seq_str, container_ext);
                                        let new_rel_path =
                                            format!("media/{}/{}", track_name, new_filename);

                                        if ProjectManifest::validate_path_in_root(
                                            &canonical_dir,
                                            &new_rel_path,
                                        )
                                        .is_err()
                                        {
                                            continue;
                                        }

                                        let track_type =
                                            determine_track_type(&track_name, &new_rel_path);
                                        let validated =
                                            MediaValidator::validate(&seg_path, track_type);
                                        let info = match validated {
                                            Ok(info) => info,
                                            Err(err) => {
                                                // Not salvageable; delete the temp file to
                                                // prevent it from re-poisoning the next
                                                // recovery attempt. Containers that fail
                                                // validation cannot be remuxed on the fly
                                                // and must not be journaled.
                                                eprintln!(
                                                    "recovery: rejecting temp segment {:?}: {}",
                                                    seg_path, err
                                                );
                                                let _ = fs::remove_file(&seg_path);
                                                continue;
                                            }
                                        };

                                        // Atomically rename into a committed name.
                                        let dest_path = canonical_dir.join(&new_rel_path);
                                        if dest_path.exists() {
                                            // A committed file with the same name already
                                            // exists. Don't overwrite; the journal is the
                                            // source of truth. The temp is dropped.
                                            eprintln!(
                                                "recovery: committed file already exists at {:?}, dropping temp {:?}",
                                                dest_path, seg_path
                                            );
                                            let _ = fs::remove_file(&seg_path);
                                            continue;
                                        }
                                        if let Err(e) = fs::rename(&seg_path, &dest_path) {
                                            eprintln!(
                                                "recovery: failed to rename temp segment {:?} -> {:?}: {}",
                                                seg_path, dest_path, e
                                            );
                                            continue;
                                        }
                                        // fsync the parent directory so the rename survives a crash.
                                        if let Some(parent) = dest_path.parent() {
                                            if let Ok(dir) = fs::File::open(parent) {
                                                let _ = dir.sync_all();
                                            }
                                        }

                                        let report = track_reports
                                            .entry(track_name.clone())
                                            .or_insert_with(|| TrackRecoveryReport {
                                                track_id: track_name.clone(),
                                                valid_segments: 0,
                                                unindexed_recovered_segments: 0,
                                                missing_segments: 0,
                                                total_recoverable_bytes: 0,
                                                max_timestamp_us: 0,
                                            });
                                        report.unindexed_recovered_segments += 1;
                                        report.total_recoverable_bytes += info.size_bytes;

                                        let seg_start_us = if info.start_us > 0 {
                                            info.start_us
                                        } else {
                                            report.max_timestamp_us
                                        };
                                        let seg_end_us = seg_start_us + info.duration_us;
                                        report.max_timestamp_us = seg_end_us;
                                        if seg_end_us > global_max_us {
                                            global_max_us = seg_end_us;
                                        }

                                        recovered_unindexed_records.push((
                                            track_name.clone(),
                                            track_type,
                                            new_rel_path,
                                            seg_start_us,
                                            seg_end_us,
                                            info.size_bytes,
                                            info.media_timescale,
                                            info.media_start_value,
                                            info.host_anchor_us,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Rebuild and persist segment index into journal
        if !recovered_unindexed_records.is_empty() {
            if let Ok(journal) = ProjectJournal::open_or_create(&canonical_dir) {
                for (
                    track_id,
                    _,
                    rel_path,
                    s_us,
                    e_us,
                    bytes,
                    media_timescale,
                    media_start_value,
                    host_anchor_us,
                ) in &recovered_unindexed_records
                {
                    let _ = journal.append(JournalRecord::UnindexedSegmentRecovered {
                        seq: 0,
                        track_id: track_id.clone(),
                        relative_path: rel_path.clone(),
                        start_us: *s_us,
                        end_us: *e_us,
                        size_bytes: *bytes,
                        media_timescale: *media_timescale,
                        media_start_value: *media_start_value,
                        host_anchor_us: *host_anchor_us,
                    });
                }
            }
        }

        // Calculate net active duration minus pauses
        let total_paused_us: u64 = pause_intervals
            .iter()
            .map(|p| p.end_us.saturating_sub(p.start_us))
            .sum();
        let active_duration_us = global_max_us.saturating_sub(total_paused_us);

        let mut recovered_manifest = manifest;
        recovered_manifest.duration_us = global_max_us;
        recovered_manifest.active_duration_us = active_duration_us;
        recovered_manifest.pause_intervals = pause_intervals.clone();

        // Ensure recovered tracks are registered in manifest
        for (track_id, track_type, rel_path, _, _, _, media_timescale, _, _) in
            &recovered_unindexed_records
        {
            if !recovered_manifest.tracks.iter().any(|t| &t.id == track_id) {
                recovered_manifest.tracks.push(TrackDescriptor {
                    id: track_id.clone(),
                    track_type: *track_type,
                    codec: match track_type {
                        TrackType::Screen | TrackType::Webcam => "h264".into(),
                        TrackType::SystemAudio | TrackType::MicAudio => "pcm".into(),
                    },
                    relative_path: rel_path.clone(),
                    width: if *track_type == TrackType::Screen {
                        Some(1920)
                    } else {
                        None
                    },
                    height: if *track_type == TrackType::Screen {
                        Some(1080)
                    } else {
                        None
                    },
                    fps: if *track_type == TrackType::Screen {
                        Some(30)
                    } else {
                        None
                    },
                    sample_rate: if *track_type == TrackType::MicAudio
                        || *track_type == TrackType::SystemAudio
                    {
                        Some(48000)
                    } else {
                        None
                    },
                    channels: if *track_type == TrackType::MicAudio {
                        Some(1)
                    } else if *track_type == TrackType::SystemAudio {
                        Some(2)
                    } else {
                        None
                    },
                    gaps_total: 0,
                    media_timescale: if *media_timescale > 0 {
                        Some(*media_timescale)
                    } else {
                        None
                    },
                });
            }
        }

        // Atomically save recovered manifest with backup
        recovered_manifest
            .save_with_backup(&manifest_path)
            .map_err(|e| RecoveryError::Manifest(e.to_string()))?;

        let total_journal_entries = journal_records.len() + recovered_unindexed_records.len();

        Ok(ProjectRecoveryReport {
            session_id: recovered_manifest.session_id.clone(),
            total_journal_entries,
            track_reports,
            recoverable_duration_us: global_max_us,
            active_duration_us,
            pause_intervals,
            recovered_manifest,
        })
    }
}

fn determine_track_type(track_id: &str, relative_path: &str) -> TrackType {
    if track_id.contains("webcam") || relative_path.contains("webcam") {
        TrackType::Webcam
    } else if track_id.contains("screen") || relative_path.contains("screen") {
        TrackType::Screen
    } else if track_id.contains("system") || relative_path.contains("system") {
        TrackType::SystemAudio
    } else {
        TrackType::MicAudio
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_valid_fmp4_header(path: &Path) {
        let data = crate::fixtures::generate_valid_fmp4_segment(0, 2_000_000, true);
        fs::write(path, data).unwrap();
    }

    #[test]
    fn test_recovery_validates_media_and_discovers_unindexed() {
        let dir = tempdir().unwrap();
        let project_dir = dir.path();

        let screen_dir = project_dir.join("media").join("screen");
        fs::create_dir_all(&screen_dir).unwrap();

        // Segment 1: indexed and valid
        let seg1 = screen_dir.join("000001.mp4");
        write_valid_fmp4_header(&seg1);

        // Segment 2: unindexed on disk (crash before journal append)
        let seg2 = screen_dir.join("000002.mp4");
        write_valid_fmp4_header(&seg2);

        // Segment 3: zero-filled dummy (must be rejected)
        let seg3 = screen_dir.join("000003.mp4");
        fs::write(&seg3, vec![0u8; 1024]).unwrap();

        let journal = ProjectJournal::open_or_create(project_dir).unwrap();
        journal
            .append(JournalRecord::SegmentCommitted {
                seq: 0,
                track_id: "screen".into(),
                relative_path: "media/screen/000001.mp4".into(),
                start_us: 0,
                end_us: 2_000_000,
                size_bytes: fs::metadata(&seg1).unwrap().len(),
                is_keyframe_start: true,
                media_timescale: 90_000,
                media_start_value: 0,
                host_anchor_us: 0,
            })
            .unwrap();

        journal
            .append(JournalRecord::SegmentCommitted {
                seq: 1,
                track_id: "screen".into(),
                relative_path: "media/screen/000003.mp4".into(),
                start_us: 2_000_000,
                end_us: 4_000_000,
                size_bytes: 1024,
                is_keyframe_start: true,
                media_timescale: 90_000,
                media_start_value: 0,
                host_anchor_us: 0,
            })
            .unwrap();

        let report = RecoveryEngine::scan_and_recover(project_dir).unwrap();
        let screen_rep = &report.track_reports["screen"];

        assert_eq!(screen_rep.valid_segments, 1, "seg1 is valid");
        assert_eq!(screen_rep.missing_segments, 1, "seg3 zero dummy rejected");
        assert_eq!(
            screen_rep.unindexed_recovered_segments, 1,
            "seg2 discovered"
        );
    }

    #[test]
    fn test_recovery_rejects_path_traversal() {
        let dir = tempdir().unwrap();
        let journal = ProjectJournal::open_or_create(dir.path()).unwrap();
        journal
            .append(JournalRecord::SegmentCommitted {
                seq: 0,
                track_id: "screen".into(),
                relative_path: "../../../etc/passwd".into(),
                start_us: 0,
                end_us: 1000,
                size_bytes: 10,
                is_keyframe_start: true,
                media_timescale: 0,
                media_start_value: 0,
                host_anchor_us: 0,
            })
            .unwrap();

        let res = RecoveryEngine::scan_and_recover(dir.path());
        assert!(matches!(res, Err(RecoveryError::InvalidPath(_))));
    }

    #[test]
    fn test_recovery_salvages_valid_temp_segment() {
        let dir = tempdir().unwrap();
        let project_dir = dir.path();

        let screen_dir = project_dir.join("media").join("screen");
        fs::create_dir_all(&screen_dir).unwrap();

        // Half-written segment from a crashed session
        let temp_seg = screen_dir.join("000007.mp4.tmp");
        write_valid_fmp4_header(&temp_seg);
        assert!(temp_seg.exists());

        let report = RecoveryEngine::scan_and_recover(project_dir).unwrap();
        let screen_rep = &report
            .track_reports
            .get("screen")
            .expect("screen track should appear in recovery report");

        // The valid temp file must be salvaged as one unindexed segment.
        assert_eq!(
            screen_rep.unindexed_recovered_segments, 1,
            "valid temp segment must be salvaged"
        );

        // The temp file must be renamed away to a committed name.
        assert!(!temp_seg.exists(), "temp file should be renamed");
        let committed = screen_dir.join("000007.mp4");
        assert!(committed.exists(), "renamed committed file must exist");

        // A journal record for the recovered segment must be appended.
        let journal = ProjectJournal::open_or_create(project_dir).unwrap();
        let records = journal.read_all().unwrap();
        assert!(
            records.iter().any(|r| matches!(
                r,
                JournalRecord::UnindexedSegmentRecovered { relative_path, .. }
                    if relative_path == "media/screen/000007.mp4"
            )),
            "journal should contain the salvaged segment record"
        );
    }

    #[test]
    fn test_recovery_rejects_invalid_temp_segment() {
        let dir = tempdir().unwrap();
        let project_dir = dir.path();

        let screen_dir = project_dir.join("media").join("screen");
        fs::create_dir_all(&screen_dir).unwrap();

        // Corrupt temp segment (zero-filled) — must be deleted, not journaled.
        let bad_temp = screen_dir.join("000009.mp4.tmp");
        fs::write(&bad_temp, vec![0u8; 1024]).unwrap();

        // Pre-existing committed file with the same name should not be overwritten.
        let good_committed = screen_dir.join("000010.mp4");
        write_valid_fmp4_header(&good_committed);
        let dup_temp = screen_dir.join("000010.mp4.tmp");
        write_valid_fmp4_header(&dup_temp);

        let report = RecoveryEngine::scan_and_recover(project_dir).unwrap();

        // The bad temp must be deleted, not journaled.
        assert!(!bad_temp.exists(), "invalid temp must be deleted");

        // The duplicate temp must be deleted because a committed file with the
        // same name already exists.
        assert!(!dup_temp.exists(), "duplicate temp must be deleted");

        // The committed file must survive untouched.
        assert!(good_committed.exists());

        // The good committed file is not in the journal, so it is
        // discovered as one unindexed recovery. The bad temp and the
        // duplicate temp do not contribute to this counter.
        let screen_rep = report
            .track_reports
            .get("screen")
            .expect("screen track should be present");
        assert_eq!(
            screen_rep.unindexed_recovered_segments, 1,
            "only the pre-existing good committed file is recovered"
        );
        // No temp file should have been promoted to a journal record.
        let journal = ProjectJournal::open_or_create(project_dir).unwrap();
        let records = journal.read_all().unwrap();
        for record in &records {
            if let JournalRecord::UnindexedSegmentRecovered { relative_path, .. } = record {
                assert_ne!(
                    relative_path, "media/screen/000009.mp4",
                    "bad temp must not be journaled"
                );
            }
        }
    }
}
