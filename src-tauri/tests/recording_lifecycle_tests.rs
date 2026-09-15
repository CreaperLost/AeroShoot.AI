#[cfg(target_os = "macos")]
use aeroshoot_lib::commands::NativeCaptureOutcome;
use aeroshoot_lib::commands::{
    get_session_status_impl, pause_recording_impl, resume_recording_impl, start_recording_impl,
    stop_recording_impl, AppState, StartRecordingOptions,
};
use aeroshoot_lib::fixtures::generate_valid_fmp4_segment;
use aeroshoot_lib::project::{
    DurabilityFault, JournalRecord, ProjectBundle, ProjectJournal, RecoveryEngine,
    TrackSegmentWriter, TrackType,
};
use aeroshoot_lib::session::SessionState;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

fn start_opts(camera: bool) -> StartRecordingOptions {
    StartRecordingOptions {
        source_id: "screen-main".into(),
        capture_screen: true,
        camera_id: camera.then(|| "cam-1".into()),
        mic_id: None,
        capture_system_audio: false,
        fps: 30,
        resolution: "1080p".into(),
        layout: None,
        project_name: Some("H1 Lifecycle".into()),
        project_dir: None,
        mic_gain_db: None,
        video_bitrate_bps: None,
        capture_mouse: true,
    }
}

fn project_root(state: &AppState) -> PathBuf {
    state
        .active_session
        .read()
        .as_ref()
        .unwrap()
        .project_bundle
        .root_path()
        .to_path_buf()
}

#[test]
fn pause_does_not_report_paused_when_finalize_fails() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    start_recording_impl(&state, start_opts(false)).unwrap();

    {
        let mut guard = state.active_session.write();
        let session = guard.as_mut().unwrap();
        session
            .segment_writer
            .as_mut()
            .unwrap()
            .inject_fault(DurabilityFault::FailJournalAppend);
    }

    let err = pause_recording_impl(&state).expect_err("finalize must surface");
    assert!(
        err.contains("Pause finalization failed"),
        "unexpected error: {err}"
    );
    assert_eq!(state.state_machine.current(), SessionState::Recording);
    assert!(
        state.diagnostics.last_runtime_error().is_some(),
        "typed diagnostic must be recorded"
    );
    let status = get_session_status_impl(&state);
    assert_ne!(status.state, SessionState::Paused);
    assert_eq!(status.last_runtime_error.unwrap().error_code, -610);

    let retry_err = {
        let mut guard = state.active_session.write();
        guard
            .as_mut()
            .unwrap()
            .segment_writer
            .as_mut()
            .unwrap()
            .inject_fault(DurabilityFault::FailJournalAppend);
        drop(guard);
        pause_recording_impl(&state).expect_err("retry must not invent success")
    };
    assert!(retry_err.contains("Pause finalization failed"));
    assert_eq!(state.state_machine.current(), SessionState::Recording);

    {
        let mut guard = state.active_session.write();
        guard
            .as_mut()
            .unwrap()
            .segment_writer
            .as_mut()
            .unwrap()
            .inject_fault(DurabilityFault::None);
    }
    let paused = pause_recording_impl(&state).unwrap();
    assert_eq!(paused.state, SessionState::Paused);
    assert_eq!(state.state_machine.current(), SessionState::Paused);
}

#[test]
fn pause_started_journal_failure_refuses_paused_and_retries() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    start_recording_impl(&state, start_opts(false)).unwrap();

    {
        let mut guard = state.active_session.write();
        let session = guard.as_mut().unwrap();
        let now = session.epoch.current_elapsed_us().max(33_333);
        session
            .segment_writer
            .as_mut()
            .unwrap()
            .finalize(now, session.project_bundle.journal())
            .unwrap();
        session.project_bundle.journal().inject_fail_next_appends(1);
    }

    let err = pause_recording_impl(&state).expect_err("PauseStarted journal failure");
    assert!(err.contains("PauseStarted"));
    assert_eq!(state.state_machine.current(), SessionState::Recording);
    assert!(state.active_session.read().is_some());

    let paused = pause_recording_impl(&state).unwrap();
    assert_eq!(paused.state, SessionState::Paused);
}

#[test]
fn pause_started_journal_failure_keeps_unacked_native_pause() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    start_recording_impl(&state, start_opts(false)).unwrap();

    {
        let mut guard = state.active_session.write();
        let session = guard.as_mut().unwrap();
        let now = session.epoch.current_elapsed_us().max(33_333);
        session
            .segment_writer
            .as_mut()
            .unwrap()
            .finalize(now, session.project_bundle.journal())
            .unwrap();
        session.native_pause_unacked = true;
        session.project_bundle.journal().inject_fail_next_appends(1);
    }

    let err = pause_recording_impl(&state).expect_err("PauseStarted journal failure");
    assert!(err.contains("PauseStarted"));
    assert_eq!(state.state_machine.current(), SessionState::Recording);
    assert!(
        state
            .active_session
            .read()
            .as_ref()
            .unwrap()
            .native_pause_unacked,
        "native pause must stay unacked until PauseStarted is journaled"
    );

    let resume = resume_recording_impl(&state).expect_err("must not no-op as live Recording");
    assert!(
        resume.contains("retry Pause or Stop"),
        "unexpected resume: {resume}"
    );
    assert_eq!(state.state_machine.current(), SessionState::Recording);

    let paused = pause_recording_impl(&state).unwrap();
    assert_eq!(paused.state, SessionState::Paused);
    assert!(
        !state
            .active_session
            .read()
            .as_ref()
            .unwrap()
            .native_pause_unacked
    );
}

#[test]
fn resume_after_pause_opens_new_segment_interval() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    start_recording_impl(&state, start_opts(true)).unwrap();
    let root = project_root(&state);
    let first_path = root.join("media/screen/000001.mp4");

    let paused = pause_recording_impl(&state).unwrap();
    assert_eq!(paused.state, SessionState::Paused);
    assert!(first_path.exists());
    let first_bytes = fs::read(&first_path).unwrap();
    let after_pause = ProjectJournal::open_or_create(&root)
        .unwrap()
        .read_all()
        .unwrap();
    let pause_started = after_pause
        .iter()
        .find_map(|r| match r {
            JournalRecord::PauseStarted { t_us, .. } => Some(*t_us),
            _ => None,
        })
        .expect("PauseStarted journaled");
    let first_end = after_pause
        .iter()
        .find_map(|r| match r {
            JournalRecord::SegmentCommitted {
                track_id, end_us, ..
            } if track_id == "screen" => Some(*end_us),
            _ => None,
        })
        .unwrap();
    assert_eq!(first_end, pause_started.max(33_333));

    std::thread::sleep(std::time::Duration::from_millis(20));
    let resumed = resume_recording_impl(&state).unwrap();
    assert_eq!(resumed.state, SessionState::Recording);
    assert_eq!(fs::read(&first_path).unwrap(), first_bytes);
    assert!(
        !root.join("media/screen/000001.tmp").exists(),
        "must not append to the closed file"
    );

    let stop = stop_recording_impl(&state).unwrap();
    assert_eq!(stop.state, SessionState::Completed);
    assert!(root.join("media/screen/000002.mp4").exists());
    assert_eq!(fs::read(&first_path).unwrap(), first_bytes);

    let records = ProjectJournal::open_or_create(&root)
        .unwrap()
        .read_all()
        .unwrap();
    let screen: Vec<_> = records
        .iter()
        .filter_map(|r| match r {
            JournalRecord::SegmentCommitted {
                track_id,
                start_us,
                end_us,
                relative_path,
                ..
            } if track_id == "screen" => Some((*start_us, *end_us, relative_path.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(screen.len(), 2);
    assert_eq!(screen[0].2, "media/screen/000001.mp4");
    assert_eq!(screen[1].2, "media/screen/000002.mp4");
    assert!(screen[1].0 >= screen[0].1, "resume starts a new interval");
    assert!(screen[0].0 < screen[0].1);
    assert!(screen[1].0 < screen[1].1);

    let webcam: Vec<_> = records
        .iter()
        .filter_map(|r| match r {
            JournalRecord::SegmentCommitted {
                track_id, start_us, ..
            } if track_id == "webcam" => Some(*start_us),
            _ => None,
        })
        .collect();
    assert_eq!(webcam.len(), 2);
    assert_eq!(webcam[0], 0);
    assert_eq!(webcam[1], screen[1].0);
}

#[test]
fn injected_sync_failure_preserves_sources_and_prior_commits() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    start_recording_impl(&state, start_opts(false)).unwrap();
    let root = project_root(&state);

    {
        let mut guard = state.active_session.write();
        let session = guard.as_mut().unwrap();
        let now = session.epoch.current_elapsed_us().max(33_333);
        session
            .segment_writer
            .as_mut()
            .unwrap()
            .finalize(now, session.project_bundle.journal())
            .unwrap();
        session
            .segment_writer
            .as_mut()
            .unwrap()
            .begin_segment(now)
            .unwrap();
        session
            .segment_writer
            .as_mut()
            .unwrap()
            .write_data(&generate_valid_fmp4_segment(now, 33_333, true))
            .unwrap();
        session
            .segment_writer
            .as_mut()
            .unwrap()
            .inject_fault(DurabilityFault::FailSync);
    }

    let first = root.join("media/screen/000001.mp4");
    let prior = fs::read(&first).unwrap();
    let err = pause_recording_impl(&state).expect_err("sync failure");
    assert!(err.contains("Pause finalization failed"));
    assert_eq!(state.state_machine.current(), SessionState::Recording);
    assert!(state.active_session.read().is_some());
    assert_eq!(fs::read(&first).unwrap(), prior);
    assert!(!root.join("media/screen/000002.mp4").exists());
}

#[test]
fn forced_termination_after_commit_preserves_prior_journal() {
    let dir = tempdir().unwrap();
    let bundle =
        ProjectBundle::create_new(dir.path(), "kill-after-commit", "Kill After Commit").unwrap();
    let root = bundle.root_path().to_path_buf();
    let journal = ProjectJournal::open_or_create(&root).unwrap();
    let mut writer =
        TrackSegmentWriter::new(&root, "screen".into(), TrackType::Screen, "h264".into());
    writer.begin_segment(0).unwrap();
    writer
        .write_data(&generate_valid_fmp4_segment(0, 1_000_000, true))
        .unwrap();
    let committed = writer.commit_segment(1_000_000, true, &journal).unwrap();
    let prior = fs::read(root.join(&committed.relative_path)).unwrap();

    writer.begin_segment(1_000_000).unwrap();
    writer
        .write_data(&generate_valid_fmp4_segment(1_000_000, 500_000, true))
        .unwrap();
    drop(writer);
    drop(bundle);

    let records = ProjectJournal::open_or_create(&root)
        .unwrap()
        .read_all()
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        fs::read(root.join(&committed.relative_path)).unwrap(),
        prior
    );

    let report = RecoveryEngine::scan_and_recover(&root).unwrap();
    assert!(report.track_reports["screen"].valid_segments >= 1);
    assert_eq!(
        fs::read(root.join("media/screen/000001.mp4")).unwrap(),
        prior
    );
}

#[test]
fn pause_boundaries_and_late_track_preserve_source_time() {
    let dir = tempdir().unwrap();
    let bundle = ProjectBundle::create_new(dir.path(), "late-track", "Late Track").unwrap();
    let root = bundle.root_path().to_path_buf();
    fs::create_dir_all(root.join("media/screen")).unwrap();
    fs::create_dir_all(root.join("media/webcam")).unwrap();
    fs::write(
        root.join("media/screen/000001.mp4"),
        generate_valid_fmp4_segment(0, 2_000_000, true),
    )
    .unwrap();
    fs::write(
        root.join("media/webcam/000001.mp4"),
        generate_valid_fmp4_segment(1_500_000, 1_000_000, true),
    )
    .unwrap();

    let journal = bundle.journal();
    journal
        .append(JournalRecord::SegmentCommitted {
            seq: 0,
            track_id: "screen".into(),
            relative_path: "media/screen/000001.mp4".into(),
            start_us: 0,
            end_us: 2_000_000,
            size_bytes: 1,
            is_keyframe_start: true,
            media_timescale: 90_000,
            media_start_value: 0,
            host_anchor_us: 0,
        })
        .unwrap();
    journal
        .append(JournalRecord::SegmentCommitted {
            seq: 0,
            track_id: "webcam".into(),
            relative_path: "media/webcam/000001.mp4".into(),
            start_us: 1_500_000,
            end_us: 2_500_000,
            size_bytes: 1,
            is_keyframe_start: true,
            media_timescale: 90_000,
            media_start_value: 135_000,
            host_anchor_us: 1_500_000,
        })
        .unwrap();
    journal
        .append(JournalRecord::PauseStarted {
            seq: 0,
            t_us: 2_000_000,
        })
        .unwrap();
    journal
        .append(JournalRecord::PauseEnded {
            seq: 0,
            start_us: 2_000_000,
            end_us: 2_250_000,
        })
        .unwrap();
    drop(bundle);

    let report = RecoveryEngine::scan_and_recover(&root).unwrap();
    assert_eq!(report.pause_intervals.len(), 1);
    assert_eq!(report.pause_intervals[0].start_us, 2_000_000);
    assert_eq!(report.pause_intervals[0].end_us, 2_250_000);
    let webcam = &report.track_reports["webcam"];
    assert_eq!(webcam.valid_segments, 1);
    assert!(
        webcam.max_timestamp_us >= 2_500_000,
        "late webcam keeps its source end, not screen-zero"
    );
    assert!(
        report.recoverable_duration_us >= 2_500_000,
        "late track extends source duration"
    );
}

#[test]
fn failed_stop_retains_session_and_start_refuses_competing_owner() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    start_recording_impl(&state, start_opts(false)).unwrap();
    let root = project_root(&state);
    fs::remove_dir_all(root.join("media").join("screen")).unwrap();

    let stop = stop_recording_impl(&state);
    assert!(stop.is_err());
    assert_eq!(state.state_machine.current(), SessionState::Error);
    assert!(state.active_session.read().is_some());
    assert!(state.diagnostics.last_runtime_error().is_some());

    let start = start_recording_impl(&state, start_opts(false));
    assert!(
        start.is_err(),
        "must not create a competing session while the failed one still owns the project"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn native_stop_failure_is_retained_on_retry() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    start_recording_impl(&state, start_opts(false)).unwrap();

    {
        let mut guard = state.active_session.write();
        let session = guard.as_mut().unwrap();
        session.native_session = None;
        session.native_outcome = NativeCaptureOutcome::StopFailed {
            code: -600,
            message: "Rust segment commit failed".into(),
        };
        session.segment_writer = None;
        session.extra_writers.clear();
    }

    let first = stop_recording_impl(&state).expect_err("native stop failure must surface");
    assert!(first.contains("code -600"), "unexpected: {first}");
    assert_eq!(state.state_machine.current(), SessionState::Error);
    assert!(state.active_session.read().is_some());
    assert!(state.last_stop_result.read().is_none());

    let second = stop_recording_impl(&state).expect_err("retry must not invent Completed");
    assert!(second.contains("code -600"), "unexpected: {second}");
    assert_eq!(state.state_machine.current(), SessionState::Error);
    assert!(state.active_session.read().is_some());
    assert!(state.last_stop_result.read().is_none());
}

#[cfg(target_os = "macos")]
#[test]
fn native_missing_screen_media_is_retained_on_retry() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    start_recording_impl(&state, start_opts(false)).unwrap();

    {
        let mut guard = state.active_session.write();
        let session = guard.as_mut().unwrap();
        session.native_session = None;
        session.native_outcome = NativeCaptureOutcome::Live;
        session.segment_writer = None;
        session.extra_writers.clear();
    }

    let first = stop_recording_impl(&state).expect_err("missing screen media");
    assert!(
        first.contains("No valid committed media was saved for screen"),
        "unexpected: {first}"
    );
    assert_eq!(state.state_machine.current(), SessionState::Error);
    assert!(state.active_session.read().is_some());

    let second =
        stop_recording_impl(&state).expect_err("retry must keep the missing-media outcome");
    assert!(
        second.contains("No valid committed media was saved for screen"),
        "unexpected: {second}"
    );
    assert_eq!(state.state_machine.current(), SessionState::Error);
    assert!(state.last_stop_result.read().is_none());
}

#[cfg(target_os = "macos")]
#[test]
fn native_prepare_failure_stop_does_not_complete() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    start_recording_impl(&state, start_opts(false)).unwrap();

    {
        let mut guard = state.active_session.write();
        let session = guard.as_mut().unwrap();
        session.native_session = None;
        session.native_outcome = NativeCaptureOutcome::PrepareFailed {
            message: "ScreenCaptureKit denied".into(),
        };
        session.segment_writer = None;
        session.extra_writers.clear();
        let _ = state.state_machine.transition_to(SessionState::Error);
    }

    let first = stop_recording_impl(&state).expect_err("prepare failure is not a successful Stop");
    assert!(first.contains("never started"), "unexpected: {first}");
    assert_eq!(state.state_machine.current(), SessionState::Error);
    assert!(state.last_stop_result.read().is_none());

    let second = stop_recording_impl(&state).expect_err("retry must not Complete");
    assert!(second.contains("never started"), "unexpected: {second}");
    assert!(state.active_session.read().is_some());
}
