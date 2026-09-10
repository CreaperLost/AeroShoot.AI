#![cfg(target_os = "macos")]
use aeroshoot_lib::{
    commands::*,
    export::{
        frame_time_us, media_duration_us, ExportFailure, ExportSettings, ExportState,
        SceneEvaluator, AUDIO_DURATION_SLACK_US,
    },
    fixtures::generate_pcm16_wav,
    media::{
        decode_h264_frame, write_solid_h264, PARITY_MEAN_TOLERANCE, PARITY_REGION_MEAN_TOLERANCE,
    },
    project::{JournalRecord, ProjectBundle, TrackDescriptor, TrackType},
    session::SessionState,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn fingerprint(path: &Path) -> (u64, Vec<u8>) {
    let bytes = fs::read(path).unwrap();
    let prefix = bytes.iter().take(64).copied().collect();
    (bytes.len() as u64, prefix)
}

fn two_color_project(base: &Path, id: &str) -> (PathBuf, PathBuf, PathBuf) {
    let mut bundle = ProjectBundle::create_new(base, id, id).unwrap();
    let screen_a = bundle.root_path().join("media/screen/000001.mp4");
    let screen_b = bundle.root_path().join("media/screen/000002.mp4");
    write_solid_h264(&screen_a, 64, 64, 0.92, 0.12, 0.10).unwrap();
    write_solid_h264(&screen_b, 64, 64, 0.10, 0.85, 0.20).unwrap();
    let wav = generate_pcm16_wav(48_000, 1, &vec![16384i16; 19_200]);
    let mic = bundle.root_path().join("media/mic/000001.wav");
    fs::write(&mic, &wav).unwrap();
    bundle.manifest_mut().tracks.push(TrackDescriptor {
        id: "screen".into(),
        track_type: TrackType::Screen,
        codec: "h264".into(),
        relative_path: "media/screen/000001.mp4".into(),
        width: Some(64),
        height: Some(64),
        fps: Some(30),
        sample_rate: None,
        channels: None,
        gaps_total: 0,
        media_timescale: Some(30),
    });
    bundle.manifest_mut().tracks.push(TrackDescriptor {
        id: "mic".into(),
        track_type: TrackType::MicAudio,
        codec: "pcm".into(),
        relative_path: "media/mic/000001.wav".into(),
        width: None,
        height: None,
        fps: None,
        sample_rate: Some(48_000),
        channels: Some(1),
        gaps_total: 0,
        media_timescale: Some(48_000),
    });
    bundle
        .journal()
        .append(JournalRecord::SegmentCommitted {
            seq: 0,
            track_id: "screen".into(),
            relative_path: "media/screen/000001.mp4".into(),
            start_us: 0,
            end_us: 200_000,
            size_bytes: fs::metadata(&screen_a).unwrap().len(),
            is_keyframe_start: true,
            media_timescale: 30,
            media_start_value: 0,
            host_anchor_us: 0,
        })
        .unwrap();
    bundle
        .journal()
        .append(JournalRecord::SegmentCommitted {
            seq: 1,
            track_id: "screen".into(),
            relative_path: "media/screen/000002.mp4".into(),
            start_us: 200_000,
            end_us: 400_000,
            size_bytes: fs::metadata(&screen_b).unwrap().len(),
            is_keyframe_start: true,
            media_timescale: 30,
            media_start_value: 0,
            host_anchor_us: 0,
        })
        .unwrap();
    bundle
        .journal()
        .append(JournalRecord::SegmentCommitted {
            seq: 2,
            track_id: "mic".into(),
            relative_path: "media/mic/000001.wav".into(),
            start_us: 0,
            end_us: 400_000,
            size_bytes: wav.len() as u64,
            is_keyframe_start: true,
            media_timescale: 48_000,
            media_start_value: 0,
            host_anchor_us: 0,
        })
        .unwrap();
    bundle.manifest_mut().duration_us = 400_000;
    bundle.manifest_mut().active_duration_us = 400_000;
    bundle
        .manifest()
        .save_with_backup(&bundle.root_path().join("manifest.json"))
        .unwrap();
    let root = bundle.root_path().to_path_buf();
    drop(bundle);
    (root, screen_a, mic)
}

fn wait_done(state: &AppState, job_id: &str) -> aeroshoot_lib::export::ExportStatus {
    let start = Instant::now();
    loop {
        let status = export_status_impl(state, Some(job_id.to_string())).unwrap();
        if !matches!(status.state, ExportState::Queued | ExportState::Running) {
            return status;
        }
        if start.elapsed() > Duration::from_secs(40) {
            panic!("export timed out: {status:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn settings(dest: &Path) -> ExportSettings {
    ExportSettings {
        video_codec: "h264".into(),
        audio_codec: "aac".into(),
        width: 64,
        height: 64,
        fps: 10,
        destination: Some(dest.to_string_lossy().into()),
    }
}

#[test]
fn rejects_prores_recording_collision_and_source_paths() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let (root, screen, _mic) = two_color_project(dir.path(), "policy");
    let opened = open_project_impl(&state, root.to_string_lossy().into()).unwrap();

    let mut bad = settings(&dir.path().join("out.mp4"));
    bad.video_codec = "prores".into();
    let status = export_start_impl(&state, opened.project_handle.clone(), bad).unwrap();
    assert_eq!(status.state, ExportState::Failed);
    assert!(matches!(
        status.failure,
        Some(ExportFailure::InvalidSettings { .. })
    ));

    let inside = export_start_impl(
        &state,
        opened.project_handle.clone(),
        settings(&root.join("inside.mp4")),
    )
    .unwrap();
    assert!(matches!(
        inside.failure,
        Some(ExportFailure::SourcePath { .. })
    ));

    let overwrite =
        export_start_impl(&state, opened.project_handle.clone(), settings(&screen)).unwrap();
    assert!(matches!(
        overwrite.failure,
        Some(ExportFailure::SourcePath { .. })
    ));

    let dest = dir.path().join("exists.mp4");
    fs::write(&dest, b"previous-output").unwrap();
    let collision =
        export_start_impl(&state, opened.project_handle.clone(), settings(&dest)).unwrap();
    assert!(matches!(
        collision.failure,
        Some(ExportFailure::Collision { .. })
    ));
    assert_eq!(fs::read(&dest).unwrap(), b"previous-output");

    let blocker = dir.path().join("not-a-dir");
    fs::write(&blocker, b"x").unwrap();
    let disk = export_start_impl(
        &state,
        opened.project_handle.clone(),
        settings(&blocker.join("out.mp4")),
    )
    .unwrap();
    assert!(matches!(disk.failure, Some(ExportFailure::Io { .. })));

    state
        .state_machine
        .transition_to(SessionState::Preparing)
        .unwrap();
    state
        .state_machine
        .transition_to(SessionState::Recording)
        .unwrap();
    let recording = export_start_impl(
        &state,
        opened.project_handle.clone(),
        settings(&dir.path().join("while-recording.mp4")),
    )
    .unwrap();
    assert!(matches!(
        recording.failure,
        Some(ExportFailure::RecordingActive { .. })
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn export_matches_preview_at_cut_and_preserves_sources() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let (root, screen, mic) = two_color_project(dir.path(), "parity");
    let opened = open_project_impl(&state, root.to_string_lossy().into()).unwrap();
    let cut = project_ripple_cuts_impl(
        &state,
        opened.project_handle.clone(),
        0,
        vec![EditCut {
            start_us: 100_000,
            end_us: 300_000,
        }],
    )
    .unwrap();
    assert_eq!(cut.edited_duration_us, 200_000);
    let original = cut.retained_intervals.clone();
    let revision = cut.revision;
    let screen_fp = fingerprint(&screen);
    let mic_fp = fingerprint(&mic);
    let dest = dir.path().join("parity.mp4");
    let previous = dir.path().join("previous.mp4");
    fs::write(&previous, b"keep-me").unwrap();

    let started =
        export_start_impl(&state, opened.project_handle.clone(), settings(&dest)).unwrap();
    assert!(!matches!(started.state, ExportState::Failed));
    let json = serde_json::to_value(&started).unwrap();
    assert!(json.get("pixels").is_none());
    assert!(json.get("samples").is_none());

    let later = project_ripple_cuts_impl(
        &state,
        opened.project_handle.clone(),
        revision,
        vec![EditCut {
            start_us: 0,
            end_us: 50_000,
        }],
    )
    .unwrap();
    assert_ne!(later.revision, revision);

    let done = wait_done(&state, &started.job_id);
    assert_eq!(done.state, ExportState::Completed, "{done:?}");
    assert_eq!(done.captured_revision, revision);
    let output = PathBuf::from(done.output_path.as_ref().expect("output path"));
    assert_eq!(output.canonicalize().unwrap(), dest.canonicalize().unwrap());
    assert!(dest.is_file());
    assert!(!dir.path().join(".parity-aeroshoot-partial").exists());
    assert_eq!(fs::read(&previous).unwrap(), b"keep-me");
    assert_eq!(fingerprint(&screen), screen_fp);
    assert_eq!(fingerprint(&mic), mic_fp);

    let tracks = {
        let opened_reader = state.opened_project.lock();
        aeroshoot_lib::playback::tracks_from_reader(opened_reader.as_ref().unwrap())
    };
    let document = aeroshoot_lib::project::EditDocument {
        schema_version: 1,
        revision,
        retained_intervals: original,
        layout: Default::default(),
        ..Default::default()
    };
    let mut preview = SceneEvaluator::new(root.clone(), document, tracks, 64, 64).unwrap();
    for index in [0u32, 1] {
        let pts = frame_time_us(index, 10);
        let reference = preview.preview_at(pts).unwrap();
        let decoded = decode_h264_frame(&dest, pts).unwrap();
        // Assert source semantics independently of the shared evaluator.
        let center = ((32 * 64 + 32) * 4) as usize;
        if index == 0 {
            assert!(decoded.data[center + 2] > decoded.data[center + 1] + 80);
        } else {
            assert!(decoded.data[center + 1] > decoded.data[center + 2] + 80);
        }
        let (max_delta, mean, region, matched) =
            aeroshoot_lib::export::compare_preview_and_export(&reference, &decoded).unwrap();
        println!("pts={pts} mean={mean:.2} max={max_delta} region={region:.2} matched={matched}");
        assert!(
            matched && mean <= PARITY_MEAN_TOLERANCE && region <= PARITY_REGION_MEAN_TOLERANCE,
            "pts={pts} mean={mean} max={max_delta} region={region}"
        );
    }
    let duration = media_duration_us(&dest).unwrap();
    let delta = duration.abs_diff(200_000);
    assert!(
        delta <= AUDIO_DURATION_SLACK_US,
        "export duration {duration} vs edited 200000"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn cancel_leaves_destination_absent() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let (root, _screen, _mic) = two_color_project(dir.path(), "cancel");
    let opened = open_project_impl(&state, root.to_string_lossy().into()).unwrap();
    let dest = dir.path().join("cancelled.mp4");
    let started =
        export_start_impl(&state, opened.project_handle.clone(), settings(&dest)).unwrap();
    let _ = export_cancel_impl(&state, started.job_id.clone());
    let done = wait_done(&state, &started.job_id);
    if dest.exists() {
        assert_eq!(done.state, ExportState::Completed);
    } else {
        assert!(matches!(
            done.state,
            ExportState::Cancelled | ExportState::Failed | ExportState::Completed
        ));
        if done.state != ExportState::Completed {
            assert!(!dest.exists());
        }
    }
}

#[test]
fn cancel_before_run_does_not_publish() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let (root, _screen, _mic) = two_color_project(dir.path(), "precancel");
    let opened = open_project_impl(&state, root.to_string_lossy().into()).unwrap();
    let dest = dir.path().join("never.mp4");
    let captured = {
        let reader = state.opened_project.lock();
        let reader = reader.as_ref().unwrap();
        aeroshoot_lib::export::prepare_job(
            SessionState::Idle,
            reader.root(),
            "precancel",
            reader.history().current.clone(),
            aeroshoot_lib::playback::tracks_from_reader(reader),
            settings(&dest),
            &mut state.export.lock(),
        )
        .unwrap()
    };
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let err = aeroshoot_lib::export::run_export(&captured, &cancel, |_, _| {}, &state.encoder_gate)
        .unwrap_err();
    assert!(matches!(err, ExportFailure::Cancelled { .. }));
    assert!(!dest.exists());
    let _ = opened;
}

#[test]
fn fractional_final_frame_is_exported_at_1080p() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let (root, _, _) = two_color_project(dir.path(), "fractional");
    let opened = open_project_impl(&state, root.to_string_lossy().into()).unwrap();
    project_ripple_cuts_impl(
        &state,
        opened.project_handle.clone(),
        0,
        vec![EditCut {
            start_us: 0,
            end_us: 150_000,
        }],
    )
    .unwrap();
    let dest = dir.path().join("fractional.mp4");
    let mut config = settings(&dest);
    config.width = 1920;
    config.height = 1080;
    let started = export_start_impl(&state, opened.project_handle, config).unwrap();
    let done = wait_done(&state, &started.job_id);
    assert_eq!(done.state, ExportState::Completed, "{done:?}");
    assert_eq!(done.progress_denominator, 3);
    assert!(media_duration_us(&dest).unwrap().abs_diff(250_000) < 20_000);
    let last = decode_h264_frame(&dest, 200_000).unwrap();
    assert_eq!((last.width, last.height), (1920, 1080));
    let center = ((540 * 1920 + 960) * 4) as usize;
    assert!(last.data[center + 1] > last.data[center + 2] + 80);
}

#[test]
fn unreadable_available_video_fails_without_publishing() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let (root, screen, _) = two_color_project(dir.path(), "broken");
    let opened = open_project_impl(&state, root.to_string_lossy().into()).unwrap();
    // Corrupt after indexing: this is an available source that must not become a background.
    fs::write(&screen, b"corrupt video").unwrap();
    let dest = dir.path().join("broken.mp4");
    let started = export_start_impl(&state, opened.project_handle, settings(&dest)).unwrap();
    let done = wait_done(&state, &started.job_id);
    assert_eq!(done.state, ExportState::Failed, "{done:?}");
    assert!(matches!(done.failure, Some(ExportFailure::Native { .. })));
    assert!(!dest.exists());
}

#[test]
fn cancellation_after_render_cleans_partial_file() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let (root, _, _) = two_color_project(dir.path(), "midcancel");
    open_project_impl(&state, root.to_string_lossy().into()).unwrap();
    let dest = dir.path().join("cancel.mp4");
    let captured = {
        let guard = state.opened_project.lock();
        let reader = guard.as_ref().unwrap();
        aeroshoot_lib::export::prepare_job(
            SessionState::Idle,
            reader.root(),
            "midcancel",
            reader.history().current.clone(),
            aeroshoot_lib::playback::tracks_from_reader(reader),
            settings(&dest),
            &mut state.export.lock(),
        )
        .unwrap()
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let result = aeroshoot_lib::export::run_export(
        &captured,
        &cancel,
        |done, _| {
            if done == 1 {
                cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        },
        &state.encoder_gate,
    );
    assert!(matches!(result, Err(ExportFailure::Cancelled { .. })));
    assert!(!dest.exists());
    assert!(!captured.temp.exists());
}

#[test]
fn default_export_destination_and_bundle_refusal() {
    use aeroshoot_lib::export::{default_destination, default_export_filename, is_inside_bundle};

    // Test filename sanitization and extension handling
    assert_eq!(default_export_filename("Launch Demo"), "Launch Demo.mp4");
    assert_eq!(
        default_export_filename("Launch Demo.mp4"),
        "Launch Demo.mp4"
    );
    assert_eq!(
        default_export_filename("Launch Demo.MP4"),
        "Launch Demo.mp4"
    );
    assert_eq!(default_export_filename(""), "Untitled.mp4");
    assert_eq!(default_export_filename("   "), "Untitled.mp4");
    assert_eq!(
        default_export_filename("Test/Slash:Colon*Star?"),
        "TestSlashColonStar.mp4"
    );

    // Test default destination is next to the .aero folder
    let dir = tempdir().unwrap();
    let bundle_path = dir.path().join("My Project.aero");
    fs::create_dir(&bundle_path).unwrap();
    let dest = default_destination(&bundle_path, "My Project", 1);
    assert_eq!(dest, dir.path().join("My Project.mp4"));
    assert_eq!(dest.parent().unwrap(), dir.path());

    // Test is_inside_bundle checks
    assert!(is_inside_bundle(
        &bundle_path.join("output.mp4"),
        Some(&bundle_path)
    ));
    assert!(is_inside_bundle(
        &bundle_path.join("media").join("screen.mp4"),
        Some(&bundle_path)
    ));
    assert!(is_inside_bundle(&bundle_path, Some(&bundle_path)));
    assert!(is_inside_bundle(
        &dir.path().join("other.aero").join("out.mp4"),
        Some(&bundle_path)
    ));
    assert!(!is_inside_bundle(
        &dir.path().join("My Project.mp4"),
        Some(&bundle_path)
    ));
    assert!(!is_inside_bundle(
        &dir.path().join("export.mp4"),
        Some(&bundle_path)
    ));

    // Test export_start_impl rejects destination inside bundle
    let state = AppState::new_test(dir.path().into());
    let (root, _screen, _mic) = two_color_project(dir.path(), "bundle_refusal");
    let opened = open_project_impl(&state, root.to_string_lossy().into()).unwrap();

    let inside = export_start_impl(
        &state,
        opened.project_handle.clone(),
        settings(&root.join("inside.mp4")),
    )
    .unwrap();
    assert_eq!(inside.state, ExportState::Failed);
    assert!(matches!(
        inside.failure,
        Some(ExportFailure::SourcePath { .. })
    ));
    if let Some(ExportFailure::SourcePath { message }) = inside.failure {
        assert!(message.contains("inside the project bundle"));
    }

    // Default destination (None requested) resolves next to the .aero folder
    let mut default_settings = settings(&dir.path().join("dummy.mp4"));
    default_settings.destination = None;
    let prepared_dest =
        aeroshoot_lib::export::resolve_destination(&root, "bundle_refusal", 1, None, &[]).unwrap();
    assert_eq!(
        prepared_dest,
        fs::canonicalize(root.parent().unwrap())
            .unwrap()
            .join("bundle_refusal.mp4")
    );
}
