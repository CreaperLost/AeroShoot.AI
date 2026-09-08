use aeroshoot_lib::capture::{
    AudioDevice, CameraDevice, NativeCaptureSessionHandle, PermissionState, PermissionStatus,
};
use aeroshoot_lib::commands::{
    get_permission_status_impl, pause_recording_impl, resume_recording_impl, start_recording_impl,
    stop_recording_impl, AppState, DevicesResult, SessionStateResult, SessionStatusResult,
    StartRecordingOptions, StartRecordingResult, StopRecordingResult,
};
use aeroshoot_lib::dsp::{SilenceConfig, SilenceCutInterval};
use aeroshoot_lib::fixtures::generate_valid_fmp4_segment;
use aeroshoot_lib::project::lock::ProjectLock;
use aeroshoot_lib::project::media_validator::MediaValidator;
use aeroshoot_lib::project::{
    JournalRecord, LockError, ProjectBundle, ProjectError, ProjectJournal, ProjectManifest,
    RecoveryEngine, RecoveryError, TrackSegmentWriter, TrackType,
};
use aeroshoot_lib::session::{ClockDriftEstimator, SessionState};
use std::fs;
use std::sync::Arc;
use tempfile::tempdir;

fn write_valid_fmp4_segment(path: &std::path::Path, start_us: u64, duration_us: u64, is_keyframe: bool) {
    let data = generate_valid_fmp4_segment(start_us, duration_us, is_keyframe);
    fs::write(path, data).unwrap();
}

#[test]
fn test_ipc_serialization_contracts_match_frontend() {
    // 1. React payload with camelCase keys deserializes into StartRecordingOptions
    let react_json_input = r#"{
        "sourceId": "screen-1",
        "cameraId": "cam-facetime",
        "micId": "mic-builtin",
        "captureSystemAudio": true,
        "fps": 60,
        "resolution": "1080p"
    }"#;
    let opts: StartRecordingOptions = serde_json::from_str(react_json_input).unwrap();
    assert_eq!(opts.source_id, "screen-1");
    assert_eq!(opts.camera_id, Some("cam-facetime".into()));
    assert_eq!(opts.capture_system_audio, true);
    assert_eq!(opts.fps, 60);

    // 2. StartRecordingResult serializes with camelCase
    let start_res = StartRecordingResult {
        session_id: "test-sess".into(),
        state: SessionState::Recording,
        started_at_us: 123456789,
    };
    let start_json = serde_json::to_string(&start_res).unwrap();
    assert!(start_json.contains("\"sessionId\":\"test-sess\""));
    assert!(start_json.contains("\"startedAtUs\":123456789"));
    assert!(start_json.contains("\"state\":\"recording\""));

    // 3. SessionStateResult serializes as { state: "paused" }
    let state_res = SessionStateResult {
        state: SessionState::Paused,
    };
    let state_json = serde_json::to_string(&state_res).unwrap();
    assert_eq!(state_json, r#"{"state":"paused"}"#);

    // 4. StopRecordingResult serializes with durationUs
    let stop_res = StopRecordingResult {
        session_id: "test-sess".into(),
        state: SessionState::Completed,
        duration_us: 5_000_000,
    };
    let stop_json = serde_json::to_string(&stop_res).unwrap();
    assert!(stop_json.contains("\"sessionId\":\"test-sess\""));
    assert!(stop_json.contains("\"durationUs\":5000000"));

    // 5. SessionStatusResult serializes with elapsedUs, droppedFrames
    let status_res = SessionStatusResult {
        state: SessionState::Recording,
        elapsed_us: 2_500_000,
        dropped_frames: 1,
        audio_buffer_underflows: 0,
        last_runtime_error: None,
        gaps_total: 0,
        timestamp_records_dropped: 0,
    };
    let status_json = serde_json::to_string(&status_res).unwrap();
    assert!(status_json.contains("\"elapsedUs\":2500000"));
    assert!(status_json.contains("\"droppedFrames\":1"));
    assert!(status_json.contains("\"timestampRecordsDropped\":0"));

    // 6. DevicesResult and AudioDevice / CameraDevice serialize with isDefault
    let dev_res = DevicesResult {
        cameras: vec![CameraDevice {
            id: "c1".into(),
            name: "Cam".into(),
            is_default: true,
        }],
        mics: vec![AudioDevice {
            id: "m1".into(),
            name: "Mic".into(),
            is_default: false,
        }],
    };
    let dev_json = serde_json::to_string(&dev_res).unwrap();
    assert!(dev_json.contains("\"isDefault\":true"));
    assert!(dev_json.contains("\"isDefault\":false"));

    // 7. SilenceConfig and SilenceCutInterval serialize with camelCase
    let sil_cfg = SilenceConfig {
        threshold_db: -38.0,
        min_duration_ms: 400,
        padding_ms: 50,
    };
    let sil_json = serde_json::to_string(&sil_cfg).unwrap();
    assert!(sil_json.contains("\"thresholdDb\":-38.0"));
    assert!(sil_json.contains("\"minDurationMs\":400"));
    assert!(sil_json.contains("\"paddingMs\":50"));

    let cut = SilenceCutInterval {
        id: "c1".into(),
        start_us: 1000,
        end_us: 2000,
        duration_ms: 1,
        selected: true,
    };
    let cut_json = serde_json::to_string(&cut).unwrap();
    assert!(cut_json.contains("\"startUs\":1000"));
    assert!(cut_json.contains("\"durationMs\":1"));
}

#[test]
fn test_repeated_recording_sessions_and_retries() {
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

    // Cycle 1: Start
    let start1 = start_recording_impl(&state, opts.clone()).unwrap();
    assert_eq!(start1.state, SessionState::Recording);
    let sess_id_1 = start1.session_id.clone();

    // Idempotent Start Retry: must return existing active session ID
    let start1_retry = start_recording_impl(&state, opts.clone()).unwrap();
    assert_eq!(start1_retry.session_id, sess_id_1);

    // Pause
    let p_res = pause_recording_impl(&state).unwrap();
    assert_eq!(p_res.state, SessionState::Paused);

    // Idempotent Pause Retry
    let p_res_retry = pause_recording_impl(&state).unwrap();
    assert_eq!(p_res_retry.state, SessionState::Paused);

    // Resume
    let r_res = resume_recording_impl(&state).unwrap();
    assert_eq!(r_res.state, SessionState::Recording);

    // Stop
    let stop1 = stop_recording_impl(&state).unwrap();
    assert_eq!(stop1.session_id, sess_id_1, "Stop must return original session ID");
    assert_eq!(stop1.state, SessionState::Completed);

    // Idempotent Stop Retry: must return cached result with same session ID
    let stop1_retry = stop_recording_impl(&state).unwrap();
    assert_eq!(stop1_retry.session_id, sess_id_1);

    // Cycle 2: Immediate start from Completed state (reproduced review item P1.3)
    let start2 = start_recording_impl(&state, opts.clone()).unwrap();
    assert_eq!(start2.state, SessionState::Recording);
    assert_ne!(start2.session_id, sess_id_1, "New recording must generate fresh UUID");

    let stop2 = stop_recording_impl(&state).unwrap();
    assert_eq!(stop2.session_id, start2.session_id);
    assert_eq!(stop2.state, SessionState::Completed);
}

#[test]
fn test_segment_writer_pipeline_finish_sync_rename_journal() {
    let dir = tempdir().unwrap();
    let journal = ProjectJournal::open_or_create(dir.path()).unwrap();

    let mut writer = TrackSegmentWriter::new(
        dir.path(),
        "screen".into(),
        TrackType::Screen,
        "mp4".into(),
    );

    // Begin segment
    let temp_path = writer.begin_segment(0).unwrap();
    assert!(temp_path.exists());
    assert!(temp_path.to_string_lossy().ends_with(".tmp"));

    // Write valid media data
    let fmp4_data = generate_valid_fmp4_segment(0, 2_000_000, true);
    writer.write_data(&fmp4_data).unwrap();

    // Commit segment: verifies flush, sync, rename, and journal append
    let commit = writer.commit_segment(2_000_000, true, &journal).unwrap();
    assert_eq!(commit.seq, 1);
    assert_eq!(commit.relative_path, "media/screen/000001.mp4");

    assert!(!temp_path.exists(), "Temporary file must be renamed away");
    let committed = dir.path().join("media/screen/000001.mp4");
    assert!(committed.exists(), "Committed segment file must exist");

    // Media structure validation must pass
    let info = MediaValidator::validate(&committed, TrackType::Screen).unwrap();
    assert_eq!(info.container_format, "mp4");
    assert!(info.sample_count > 0);

    // Verify journal durability
    let records = journal.read_all().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].seq(), 0);
}

#[test]
fn test_media_validator_and_unindexed_committed_segment_recovery() {
    let dir = tempdir().unwrap();
    let project_dir = dir.path();

    // Reject zero-filled dummy files
    let zero_dummy = project_dir.join("zero.mp4");
    fs::write(&zero_dummy, vec![0u8; 1024]).unwrap();
    assert_eq!(
        MediaValidator::validate(&zero_dummy, TrackType::Screen).err(),
        Some(aeroshoot_lib::project::MediaValidationError::ZeroFilledDummy)
    );

    // Setup project media tracks
    let screen_dir = project_dir.join("media").join("screen");
    fs::create_dir_all(&screen_dir).unwrap();

    // Segment 1: indexed in journal and valid on disk
    let seg1 = screen_dir.join("000001.mp4");
    write_valid_fmp4_segment(&seg1, 0, 2_000_000, true);

    // Segment 2: committed to disk but crash happened before journal append (unindexed discovery)
    let seg2 = screen_dir.join("000002.mp4");
    write_valid_fmp4_segment(&seg2, 2_000_000, 3_000_000, true);

    // Segment 3: zero-filled dummy that should be rejected by recovery
    let seg3 = screen_dir.join("000003.mp4");
    fs::write(&seg3, vec![0u8; 512]).unwrap();

    let journal = ProjectJournal::open_or_create(project_dir).unwrap();
    journal
        .append(JournalRecord::SegmentCommitted {
            seq: 1,
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
            seq: 3,
            track_id: "screen".into(),
            relative_path: "media/screen/000003.mp4".into(),
            start_us: 2_000_000,
            end_us: 4_000_000,
            size_bytes: 512,
            is_keyframe_start: true,
            media_timescale: 90_000,
            media_start_value: 0,
            host_anchor_us: 0,
        })
        .unwrap();

    let report = RecoveryEngine::scan_and_recover(project_dir).unwrap();
    let screen_rep = &report.track_reports["screen"];

    assert_eq!(screen_rep.valid_segments, 1, "seg1 is valid");
    assert_eq!(screen_rep.missing_segments, 1, "seg3 dummy was rejected");
    assert_eq!(
        screen_rep.unindexed_recovered_segments, 1,
        "seg2 unindexed file on disk was discovered"
    );
    assert_eq!(screen_rep.max_timestamp_us, 5_000_000);
    assert_eq!(report.recoverable_duration_us, 5_000_000);
}

#[test]
fn test_project_input_validation_and_symlink_escape_rejection() {
    let dir = tempdir().unwrap();

    // 1. Path traversal in journal must be rejected
    let journal = ProjectJournal::open_or_create(dir.path()).unwrap();
    journal
        .append(JournalRecord::SegmentCommitted {
            seq: 0,
            track_id: "screen".into(),
            relative_path: "../../../secret.txt".into(),
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

    // 2. Corrupt manifest JSON must fail recovery rather than silently overwriting
    let dir2 = tempdir().unwrap();
    let bad_manifest_path = dir2.path().join("manifest.json");
    fs::write(bad_manifest_path, b"{ not valid json }").unwrap();

    let res2 = RecoveryEngine::scan_and_recover(dir2.path());
    assert!(matches!(res2, Err(RecoveryError::Manifest(_))));
}

#[test]
fn test_journal_truncated_tail_repair() {
    let dir = tempdir().unwrap();
    let journal_path = dir.path().join("journal.jsonl");

    // Valid record followed by a truncated crash line without trailing newline
    let valid_json = r#"{"type":"checkpoint","seq":0,"t_us":1000,"total_duration_us":1000}"#;
    let truncated_tail = r#"{"type":"segment_committed","seq":1,"track_id":"scr"#;
    fs::write(&journal_path, format!("{}\n{}", valid_json, truncated_tail)).unwrap();

    // Reopening journal repairs the tail
    let journal = ProjectJournal::open_or_create(dir.path()).unwrap();

    // Next append must start on a fresh line and succeed cleanly
    let s1 = journal
        .append(JournalRecord::PauseStarted {
            seq: 0,
            t_us: 2000,
        })
        .unwrap();
    assert_eq!(s1, 1);

    let records = journal.read_all().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].seq(), 0);
    assert_eq!(records[1].seq(), 1);
}

#[test]
fn test_exclusive_project_creation_and_snapshot_backups() {
    let dir = tempdir().unwrap();
    let session_id = "sess-exclusive-1";

    let mut bundle = ProjectBundle::create_new(dir.path(), session_id, "Proj A").unwrap();
    assert!(bundle.root_path().join(".lock").exists());

    // Duplicate creation must be rejected
    let dup = ProjectBundle::create_new(dir.path(), session_id, "Proj A Dup");
    assert!(matches!(dup, Err(ProjectError::AlreadyExists(_))));

    // Updating manifest creates .bak retaining prior revision
    let manifest_path = bundle.root_path().join("manifest.json");
    let bak_path = bundle.root_path().join("manifest.bak");
    assert!(manifest_path.exists());
    assert!(!bak_path.exists());

    let mut new_manifest = bundle.manifest().clone();
    new_manifest.duration_us = 99_000_000;
    bundle.update_manifest(new_manifest).unwrap();

    assert!(bak_path.exists());
    let bak_data = fs::read_to_string(&bak_path).unwrap();
    assert!(bak_data.contains("Proj A"));
}

#[test]
fn test_pause_intervals_persisted_and_net_duration() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());

    let opts = StartRecordingOptions {
        source_id: "screen-1".into(),
        camera_id: None,
        mic_id: None,
        capture_system_audio: false,
        fps: 30,
        resolution: "1080p".into(),
    };

    let _ = start_recording_impl(&state, opts).unwrap();

    // Pause recording
    let p_res = pause_recording_impl(&state).unwrap();
    assert_eq!(p_res.state, SessionState::Paused);

    // Sleep briefly so pause duration > 0
    std::thread::sleep(std::time::Duration::from_millis(30));

    // Resume recording
    let r_res = resume_recording_impl(&state).unwrap();
    assert_eq!(r_res.state, SessionState::Recording);

    // Stop recording
    let stop_res = stop_recording_impl(&state).unwrap();
    assert_eq!(stop_res.state, SessionState::Completed);

    // Check project bundle manifest and journal
    let bundle_path = dir.path().join(format!("Project_Session_{}.aero", stop_res.session_id));
    let recovery_report = RecoveryEngine::scan_and_recover(&bundle_path).unwrap();

    assert_eq!(recovery_report.pause_intervals.len(), 1);
    assert!(recovery_report.pause_intervals[0].end_us > recovery_report.pause_intervals[0].start_us);
    assert!(recovery_report.active_duration_us <= recovery_report.recoverable_duration_us);
}

#[test]
fn test_clock_drift_estimation() {
    let sample_rate = 48_000;
    let mut estimator = ClockDriftEstimator::new(sample_rate, 0);

    // 48,000 samples should take exactly 1,000,000 us
    // If wall clock took 1,000,050 us, drift is ~ -50 PPM
    let drift_ppm = estimator.update(48_000, 1_000_050);
    assert!((drift_ppm - (-50.0)).abs() < 2.0);
}

#[test]
fn test_native_shutdown_handshake() {
    let handle = NativeCaptureSessionHandle::new();
    assert!(handle.is_active());
    assert!(!handle.is_shutdown_complete());

    handle.shutdown();
    assert!(!handle.is_active());
    assert!(handle.is_shutdown_complete());
}

// =========================================================================
// Targeted Integration Tests for Findings 1 to 10
// =========================================================================

#[test]
fn test_p1_finding_1_save_with_backup_symlink_overwrite_prevention() {
    let dir = tempdir().unwrap();
    let project_dir = dir.path().join("proj");
    fs::create_dir_all(&project_dir).unwrap();

    let victim_dir = dir.path().join("victim");
    fs::create_dir_all(&victim_dir).unwrap();
    let sensitive_file = victim_dir.join("external_secret.txt");
    fs::write(&sensitive_file, b"CONFIDENTIAL EXTERNAL DATA").unwrap();

    // Create a symlink manifest.tmp pointing to the external file
    let tmp_symlink = project_dir.join("manifest.tmp");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&sensitive_file, &tmp_symlink).unwrap();

    let manifest = ProjectManifest::new("sess-1".into(), "Symlink Test".into());
    let manifest_path = project_dir.join("manifest.json");

    // Saving with backup should safely replace or refuse to write through the symlink
    let res = manifest.save_with_backup(&manifest_path);
    assert!(res.is_ok(), "save_with_backup should complete safely");

    // The sensitive external file must NOT have been modified or overwritten!
    let victim_content = fs::read_to_string(&sensitive_file).unwrap();
    assert_eq!(victim_content, "CONFIDENTIAL EXTERNAL DATA", "External file was overwritten through symlink!");

    // Also test manifest.bak pointing to external file
    let bak_symlink = project_dir.join("manifest.bak");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&sensitive_file, &bak_symlink).unwrap();

    let res2 = manifest.save_with_backup(&manifest_path);
    assert!(res2.is_ok());
    let victim_content2 = fs::read_to_string(&sensitive_file).unwrap();
    assert_eq!(victim_content2, "CONFIDENTIAL EXTERNAL DATA", "External file was overwritten through .bak symlink!");
}

#[test]
fn test_p1_finding_2_concurrent_start_calls_serialized() {
    let dir = tempdir().unwrap();
    let state = Arc::new(AppState::new_test(dir.path().to_path_buf()));

    let opts = StartRecordingOptions {
        source_id: "screen-main".into(),
        camera_id: None,
        mic_id: None,
        capture_system_audio: false,
        fps: 30,
        resolution: "1080p".into(),
    };

    let mut handles = Vec::new();
    for _ in 0..8 {
        let state_clone = Arc::clone(&state);
        let opts_clone = opts.clone();
        handles.push(std::thread::spawn(move || {
            start_recording_impl(&state_clone, opts_clone)
        }));
    }

    let mut results = Vec::new();
    for h in handles {
        results.push(h.join().unwrap().unwrap());
    }

    assert_eq!(results.len(), 8);
    let session_id_0 = &results[0].session_id;

    for r in &results {
        assert_eq!(&r.session_id, session_id_0, "Concurrent start calls created conflicting sessions!");
        assert_eq!(r.state, SessionState::Recording);
    }

    let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(entries.len(), 1, "Expected exactly 1 project directory created");
}

#[test]
fn test_p1_finding_3_stop_recording_failure_propagation() {
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

    let start_res = start_recording_impl(&state, opts).unwrap();
    assert_eq!(start_res.state, SessionState::Recording);

    // Intentionally cause storage failure:
    // Remove the media/screen directory so segment writer flush/sync/rename fails
    let proj_bundle_dir = dir.path().join(format!("Project_Session_{}.aero", start_res.session_id));
    let screen_media_dir = proj_bundle_dir.join("media").join("screen");
    fs::remove_dir_all(&screen_media_dir).unwrap();

    let stop_res = stop_recording_impl(&state);
    assert!(stop_res.is_err(), "stop_recording_impl must propagate storage failure as Err");

    assert_eq!(state.state_machine.current(), SessionState::Error);
    assert!(state.last_stop_result.read().is_none(), "Must not cache Completed result on failure");
}

#[test]
fn test_p1_finding_4_media_validator_strict_box_bounds_and_samples() {
    let dir = tempdir().unwrap();

    // 1. Probe: 8 bytes claiming 999,999-byte box
    let probe_file = dir.path().join("truncated_box.mp4");
    let mut bad_box = Vec::new();
    bad_box.extend_from_slice(&999_999u32.to_be_bytes());
    bad_box.extend_from_slice(b"ftyp");
    fs::write(&probe_file, &bad_box).unwrap();

    let err = MediaValidator::validate(&probe_file, TrackType::Screen).unwrap_err();
    assert!(
        matches!(err, aeroshoot_lib::project::MediaValidationError::InvalidMp4BoxSize(..)),
        "Must reject 8-byte file claiming 999,999 byte box: {:?}", err
    );

    // 2. Probe: Header-only file (ftyp only, 32 bytes)
    let header_only = dir.path().join("header_only.mp4");
    let mut ftyp = Vec::new();
    ftyp.extend_from_slice(&32u32.to_be_bytes());
    ftyp.extend_from_slice(b"ftyp");
    ftyp.extend_from_slice(b"isom");
    ftyp.extend_from_slice(&0x0200u32.to_be_bytes());
    ftyp.extend_from_slice(b"isomiso2avc1mp41");
    fs::write(&header_only, &ftyp).unwrap();

    let err2 = MediaValidator::validate(&header_only, TrackType::Screen).unwrap_err();
    assert!(
        matches!(err2, aeroshoot_lib::project::MediaValidationError::MissingRequiredBoxes(_)),
        "Must reject header-only MP4: {:?}", err2
    );

    // 3. Probe: Valid fMP4 segment with full boxes and samples
    let valid_file = dir.path().join("valid_segment.mp4");
    let valid_data = generate_valid_fmp4_segment(1_000_000, 2_000_000, true);
    fs::write(&valid_file, &valid_data).unwrap();

    let info = MediaValidator::validate(&valid_file, TrackType::Screen).unwrap();
    assert_eq!(info.container_format, "mp4");
    assert!(info.sample_count > 0);
    assert_eq!(info.duration_us, 2_000_000);
    assert_eq!(info.start_us, 1_000_000);
    assert!(info.is_keyframe_start);
}

#[test]
fn test_p1_finding_5_recovery_repairs_truncated_tail_before_reading() {
    let dir = tempdir().unwrap();
    let bundle = ProjectBundle::create_new(dir.path(), "sess-trunc-tail", "Truncated Tail Test").unwrap();
    let bundle_path = bundle.root_path().to_path_buf();

    // Write a valid segment file
    let seg_path = bundle_path.join("media").join("screen").join("000001.mp4");
    fs::write(&seg_path, generate_valid_fmp4_segment(0, 1_000_000, true)).unwrap();

    // In journal, write valid record followed by truncated tail line without trailing newline
    let journal_path = bundle_path.join("journal.jsonl");
    let valid_rec = r#"{"type":"segment_committed","seq":1,"track_id":"screen","relative_path":"media/screen/000001.mp4","start_us":0,"end_us":1000000,"size_bytes":100,"is_keyframe_start":true}"#;
    let corrupt_tail = r#"{"type":"segment_committed","seq":2,"track_id":"scre"#;
    fs::write(&journal_path, format!("{}\n{}", valid_rec, corrupt_tail)).unwrap();

    // Drop bundle so project lock is released for recovery
    drop(bundle);

    let report = RecoveryEngine::scan_and_recover(&bundle_path).unwrap();
    assert!(report.track_reports.contains_key("screen"));
    assert_eq!(report.track_reports["screen"].valid_segments, 1);

    let journal = ProjectJournal::open_or_create(&bundle_path).unwrap();
    let records = journal.read_all().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].seq(), 1);
}

#[test]
fn test_p1_finding_6_project_lock_mutual_exclusion_and_recovery() {
    let dir = tempdir().unwrap();
    let proj_dir = dir.path().join("test_lock_proj");
    fs::create_dir_all(&proj_dir).unwrap();

    // 1. First lock acquisition succeeds
    let lock1 = ProjectLock::acquire(&proj_dir).unwrap();

    // 2. Second attempt within same process MUST fail with AlreadyLocked
    let lock2 = ProjectLock::acquire(&proj_dir);
    assert!(matches!(lock2, Err(LockError::AlreadyLocked { .. })));

    // 3. RecoveryEngine also attempts to acquire lock and must fail if held
    let rec = RecoveryEngine::scan_and_recover(&proj_dir);
    assert!(matches!(rec, Err(RecoveryError::Lock(LockError::AlreadyLocked { .. }))));

    // 4. Drop first lock; subsequent acquisition succeeds
    drop(lock1);
    let lock3 = ProjectLock::acquire(&proj_dir);
    assert!(lock3.is_ok(), "ProjectLock re-acquisition after drop must succeed");
}

#[test]
fn test_p1_finding_7_segment_writer_prevents_sequence_collision_and_overwrites() {
    let dir = tempdir().unwrap();
    let journal = ProjectJournal::open_or_create(dir.path()).unwrap();

    // Writer 1 writes and commits segment 1
    let mut writer1 = TrackSegmentWriter::new(dir.path(), "screen".into(), TrackType::Screen, "mp4".into());
    writer1.begin_segment(0).unwrap();
    writer1.write_data(b"ORIGINAL_SEGMENT_1_DATA").unwrap();
    let commit1 = writer1.commit_segment(1_000_000, true, &journal).unwrap();
    assert_eq!(commit1.seq, 1);
    let seg1_path = dir.path().join("media").join("screen").join("000001.mp4");
    assert_eq!(fs::read(&seg1_path).unwrap(), b"ORIGINAL_SEGMENT_1_DATA");

    // Writer 2 created on same track: must NOT start at seq 1 and overwrite 000001.mp4!
    let mut writer2 = TrackSegmentWriter::new(dir.path(), "screen".into(), TrackType::Screen, "mp4".into());
    writer2.begin_segment(1_000_000).unwrap();
    writer2.write_data(b"NEW_SEGMENT_2_DATA").unwrap();
    let commit2 = writer2.commit_segment(2_000_000, false, &journal).unwrap();

    assert_eq!(commit2.seq, 2);
    let seg2_path = dir.path().join("media").join("screen").join("000002.mp4");
    assert!(seg2_path.exists());
    assert_eq!(fs::read(&seg2_path).unwrap(), b"NEW_SEGMENT_2_DATA");

    assert_eq!(fs::read(&seg1_path).unwrap(), b"ORIGINAL_SEGMENT_1_DATA");
}

#[test]
fn test_p1_finding_8_recovery_skips_symlinked_directories_escaping_root() {
    let dir = tempdir().unwrap();
    let bundle = ProjectBundle::create_new(dir.path(), "sess-symlink-dir", "Symlink Dir Test").unwrap();
    let bundle_path = bundle.root_path().to_path_buf();
    drop(bundle);

    // Outside directory containing fake media
    let outside_dir = dir.path().join("outside_target");
    fs::create_dir_all(&outside_dir).unwrap();
    let ext_seg = outside_dir.join("000001.mp4");
    fs::write(&ext_seg, generate_valid_fmp4_segment(0, 1_000_000, true)).unwrap();

    // Create a directory symlink in media/ escaping root
    let symlink_track = bundle_path.join("media").join("escaped_track");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_dir, &symlink_track).unwrap();

    let report = RecoveryEngine::scan_and_recover(&bundle_path).unwrap();
    assert!(!report.track_reports.contains_key("escaped_track"), "Escaped symlinked track directory must be ignored!");
}

#[test]
fn test_p2_finding_9_monotonic_segment_timestamps_no_immediate_fabricated_commit() {
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

    let start_res = start_recording_impl(&state, opts).unwrap();
    assert_eq!(start_res.state, SessionState::Recording);

    // Immediately stop recording after a tiny duration (e.g. 5ms)
    std::thread::sleep(std::time::Duration::from_millis(5));
    let stop_res = stop_recording_impl(&state).unwrap();
    assert_eq!(stop_res.state, SessionState::Completed);

    // Read journal records
    let bundle_path = dir.path().join(format!("Project_Session_{}.aero", stop_res.session_id));
    let journal = ProjectJournal::open_or_create(&bundle_path).unwrap();
    let records = journal.read_all().unwrap();

    for r in records {
        if let JournalRecord::SegmentCommitted { start_us, end_us, .. } = r {
            assert!(start_us <= end_us, "Timestamp reversal detected: start_us ({}) > end_us ({})", start_us, end_us);
            assert_ne!(end_us, 500_000, "Fabricated [0, 500000) segment detected!");
        }
    }
}

#[test]
fn test_p2_finding_10_unindexed_recovery_exact_packet_timing_and_journal_index_rebuild() {
    let dir = tempdir().unwrap();
    let bundle = ProjectBundle::create_new(dir.path(), "sess-unindexed-timing", "Unindexed Test").unwrap();
    let bundle_path = bundle.root_path().to_path_buf();
    drop(bundle);

    // Write a valid segment on disk that is NOT in journal (unindexed committed segment)
    let screen_dir = bundle_path.join("media").join("screen");
    let seg1 = screen_dir.join("000001.mp4");
    // Generate valid fMP4 segment with exactly 3,500,000 us (3.5s) duration
    let valid_data = generate_valid_fmp4_segment(0, 3_500_000, true);
    fs::write(&seg1, &valid_data).unwrap();

    // Verify journal is empty
    let journal = ProjectJournal::open_or_create(&bundle_path).unwrap();
    assert_eq!(journal.read_all().unwrap().len(), 0);
    drop(journal);

    // Run recovery
    let report = RecoveryEngine::scan_and_recover(&bundle_path).unwrap();
    let screen_report = &report.track_reports["screen"];
    assert_eq!(screen_report.unindexed_recovered_segments, 1);
    // Duration must be 3,500,000 us, NOT the old hardcoded 2,000,000 us
    assert_eq!(screen_report.max_timestamp_us, 3_500_000);
    assert_eq!(report.recoverable_duration_us, 3_500_000);

    // Check that journal index was rebuilt
    let journal_after = ProjectJournal::open_or_create(&bundle_path).unwrap();
    let records_after = journal_after.read_all().unwrap();
    assert_eq!(records_after.len(), 1, "Journal index must be rebuilt with unindexed segment");
    if let JournalRecord::UnindexedSegmentRecovered { start_us, end_us, .. } = &records_after[0] {
        assert_eq!(*start_us, 0);
        assert_eq!(*end_us, 3_500_000);
    } else {
        panic!("Expected UnindexedSegmentRecovered record in journal");
    }
}

#[test]
fn test_native_callback_draining_handshake() {
    let handle = NativeCaptureSessionHandle::new();
    assert!(handle.is_active());
    assert!(!handle.is_shutdown_complete());

    // Enter callback
    let guard = handle.enter_callback().unwrap();

    // Spawn thread to initiate shutdown
    let handle_clone = handle.clone();
    let shutdown_thread = std::thread::spawn(move || {
        handle_clone.shutdown();
    });

    // Shutdown should wait for active callback to drain
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(!handle.is_active());
    assert!(!handle.is_shutdown_complete(), "Shutdown must NOT complete while callback is active");

    // Drop guard
    drop(guard);

    shutdown_thread.join().unwrap();
    assert!(handle.is_shutdown_complete(), "Shutdown must complete once callback is drained");

    assert!(handle.enter_callback().is_none());
}

#[test]
fn test_permission_status_and_override() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());

    // Overriding permissions to deny screen recording
    *state.permission_override.write() = Some(PermissionStatus {
        screen_recording: PermissionState::Denied,
        camera: PermissionState::Authorized,
        microphone: PermissionState::Authorized,
    });

    let status_denied = get_permission_status_impl(&state);
    assert_eq!(status_denied.screen_recording, PermissionState::Denied);

    // Attempting to start recording with screen permission denied must fail
    let opts = StartRecordingOptions {
        source_id: "screen-main".into(),
        camera_id: None,
        mic_id: None,
        capture_system_audio: false,
        fps: 30,
        resolution: "1080p".into(),
    };
    let start_res = start_recording_impl(&state, opts);
    assert!(start_res.is_err(), "Start recording must fail when screen recording permission is denied");
}
