#![cfg(unix)]
use aeroshoot_lib::{
    commands::*,
    fixtures::generate_pcm16_wav,
    playback::{ClockKind, PlaybackState, PreviewHitMode, MAX_OPEN_FILES},
    project::{JournalRecord, ProjectBundle, TrackDescriptor, TrackType},
};
use std::fs;
use std::time::Instant;
use tempfile::tempdir;

#[test]
fn edit_while_playing_pauses_and_new_project_generations_are_unique() {
    let dir = tempdir().unwrap();
    let doc = aeroshoot_lib::project::EditDocument::from_retained(vec![
        aeroshoot_lib::project::RetainedInterval {
            start_us: 0,
            end_us: 1_000_000,
        },
    ])
    .unwrap();
    let mut owner =
        aeroshoot_lib::playback::PlaybackOwner::open("a".into(), dir.path().into(), &doc, vec![])
            .unwrap();
    let first = owner.play().unwrap();
    owner.apply_document(&doc).unwrap();
    assert_eq!(
        owner.status().unwrap().state,
        aeroshoot_lib::playback::PlaybackState::Paused
    );
    let mut other =
        aeroshoot_lib::playback::PlaybackOwner::open("b".into(), dir.path().into(), &doc, vec![])
            .unwrap();
    assert!(other.status().unwrap().generation > first.generation);
    assert!(!other.accept_decode_result(first.generation, 0));
}

fn audio_project(base: &std::path::Path, id: &str, duration_us: u64) -> std::path::PathBuf {
    let mut bundle = ProjectBundle::create_new(base, id, id).unwrap();
    let frames = (duration_us * 48 / 1_000) as usize;
    let wav = generate_pcm16_wav(48_000, 1, &vec![16384i16; frames.max(48)]);
    fs::write(bundle.root_path().join("media/mic/000001.wav"), &wav).unwrap();
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
    bundle.manifest_mut().tracks.push(TrackDescriptor {
        id: "screen".into(),
        track_type: TrackType::Screen,
        codec: "h264".into(),
        relative_path: "media/screen/000001.mp4".into(),
        width: Some(1920),
        height: Some(1080),
        fps: Some(30),
        sample_rate: None,
        channels: None,
        gaps_total: 0,
        media_timescale: Some(30),
    });
    bundle
        .journal()
        .append(JournalRecord::SegmentCommitted {
            seq: 0,
            track_id: "mic".into(),
            relative_path: "media/mic/000001.wav".into(),
            start_us: 0,
            end_us: duration_us,
            size_bytes: wav.len() as u64,
            is_keyframe_start: true,
            media_timescale: 48_000,
            media_start_value: 0,
            host_anchor_us: 0,
        })
        .unwrap();
    bundle.manifest_mut().duration_us = duration_us;
    bundle.manifest_mut().active_duration_us = duration_us;
    bundle
        .manifest()
        .save_with_backup(&bundle.root_path().join("manifest.json"))
        .unwrap();
    bundle.root_path().to_path_buf()
}

#[test]
fn playback_and_edits_share_one_interval_list() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let path = audio_project(dir.path(), "cut", 10_000_000);
    let opened = open_project_impl(&state, path.to_string_lossy().into()).unwrap();
    assert_eq!(opened.revision, 0);
    let json = serde_json::to_value(&opened).unwrap();
    assert_eq!(json["undoAvailable"], false);

    let after = project_ripple_cuts_impl(
        &state,
        opened.project_handle.clone(),
        0,
        vec![EditCut {
            start_us: 2_000_000,
            end_us: 5_000_000,
        }],
    )
    .unwrap();
    assert_eq!(after.revision, 1);
    assert_eq!(after.edited_duration_us, 7_000_000);
    assert_eq!(after.retained_intervals.len(), 2);
    assert!(after.undo_available);

    let status = playback_seek_impl(&state, opened.project_handle.clone(), 2_000_000).unwrap();
    assert_eq!(status.state, PlaybackState::Paused);
    for plan in &status.plans {
        if plan.track_id == "mic" {
            assert_eq!(plan.source_us, Some(5_000_000));
            assert_eq!(plan.decode_to_source_us, Some(5_000_000));
            assert!(!plan.ended);
        }
    }
    let end = playback_seek_impl(&state, opened.project_handle.clone(), 7_000_000).unwrap();
    assert_eq!(end.state, PlaybackState::Ended);
    assert!(end
        .plans
        .iter()
        .all(|plan| plan.decode_to_source_us.is_none()));

    assert!(project_ripple_cuts_impl(
        &state,
        opened.project_handle.clone(),
        0,
        vec![EditCut {
            start_us: 0,
            end_us: 1_000_000
        }]
    )
    .unwrap_err()
    .contains("Stale"));

    let undone = project_undo_impl(&state, opened.project_handle.clone(), 1).unwrap();
    assert_eq!(undone.revision, 2);
    assert_eq!(undone.edited_duration_us, 10_000_000);
    close_project_impl(&state, opened.project_handle).unwrap();

    let reopened = open_project_impl(&state, path.to_string_lossy().into()).unwrap();
    assert_eq!(reopened.revision, 2);
    let redone = project_redo_impl(&state, reopened.project_handle.clone(), 2);
    assert!(
        redone.is_err(),
        "undo history is session-local; disk has revision 2"
    );
}

#[test]
fn reopen_preserves_persisted_cuts_and_clock_does_not_stall_without_mic() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let path = audio_project(dir.path(), "keep", 10_000_000);
    let opened = open_project_impl(&state, path.to_string_lossy().into()).unwrap();
    project_ripple_cuts_impl(
        &state,
        opened.project_handle.clone(),
        0,
        vec![EditCut {
            start_us: 0,
            end_us: 2_000_000,
        }],
    )
    .unwrap();
    close_project_impl(&state, opened.project_handle).unwrap();
    let reopened = open_project_impl(&state, path.to_string_lossy().into()).unwrap();
    assert_eq!(reopened.revision, 1);
    assert_eq!(reopened.edited_duration_us, 8_000_000);
    assert_eq!(reopened.retained_intervals[0].start_us, 2_000_000);

    fs::remove_file(path.join("media/mic/000001.wav")).unwrap();
    close_project_impl(&state, reopened.project_handle.clone()).unwrap();
    let without_mic = open_project_impl(&state, path.to_string_lossy().into()).unwrap();
    let playing = playback_play_impl(&state, without_mic.project_handle.clone()).unwrap();
    assert_eq!(playing.clock_kind, ClockKind::Monotonic);
    std::thread::sleep(std::time::Duration::from_millis(25));
    let later = playback_status_impl(&state, without_mic.project_handle.clone()).unwrap();
    assert!(later.position_us > 0 || later.state == PlaybackState::Ended);
}

#[test]
fn thousands_of_segments_keep_a_bounded_working_set() {
    let dir = tempdir().unwrap();
    let mut bundle = ProjectBundle::create_new(dir.path(), "many", "many").unwrap();
    let wav = generate_pcm16_wav(48_000, 1, &[0; 48]);
    bundle.manifest_mut().tracks.push(TrackDescriptor {
        id: "screen".into(),
        track_type: TrackType::Screen,
        codec: "pcm".into(),
        relative_path: "media/screen/000001.wav".into(),
        width: None,
        height: None,
        fps: None,
        sample_rate: None,
        channels: None,
        gaps_total: 0,
        media_timescale: None,
    });
    let mut duration = 0u64;
    for i in 0..2_048u64 {
        let relative = format!("media/screen/{i:06}.wav");
        if i < 24 {
            fs::write(bundle.root_path().join(&relative), &wav).unwrap();
        }
        bundle
            .journal()
            .append(JournalRecord::SegmentCommitted {
                seq: i,
                track_id: "screen".into(),
                relative_path: relative,
                start_us: i * 10_000,
                end_us: (i + 1) * 10_000,
                size_bytes: wav.len() as u64,
                is_keyframe_start: i % 8 == 0,
                media_timescale: 48_000,
                media_start_value: 0,
                host_anchor_us: (i * 10_000) as i64,
            })
            .unwrap();
        duration = (i + 1) * 10_000;
    }
    bundle.manifest_mut().duration_us = duration;
    bundle.manifest_mut().active_duration_us = duration;
    bundle
        .manifest()
        .save_with_backup(&bundle.root_path().join("manifest.json"))
        .unwrap();
    let path = bundle.root_path().to_path_buf();
    drop(bundle);

    let state = AppState::new_test(dir.path().into());
    let opened = open_project_impl(&state, path.to_string_lossy().into()).unwrap();
    let started = Instant::now();
    let mut max_open = 0usize;
    for i in 0..2_048u64 {
        let status = playback_seek_impl(&state, opened.project_handle.clone(), i * 10_000).unwrap();
        max_open = max_open.max(status.open_files);
        assert!(status.plans.iter().all(|plan| {
            plan.decode_to_source_us
                .map(|source| source < (i + 1) * 10_000)
                .unwrap_or(true)
        }));
    }
    let elapsed_ms = started.elapsed().as_millis();
    assert!(max_open <= MAX_OPEN_FILES, "open files {max_open}");
    assert!(
        elapsed_ms < 30_000,
        "2048 seeks took {elapsed_ms}ms in-process (not a device measurement)"
    );
}

#[test]
fn playback_status_serializes_camel_case() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let path = audio_project(dir.path(), "ser", 100_000);
    let opened = open_project_impl(&state, path.to_string_lossy().into()).unwrap();
    let status = playback_status_impl(&state, opened.project_handle).unwrap();
    let json = serde_json::to_value(&status).unwrap();
    // Headless tests do not own an audio device; report the clock actually used.
    assert_eq!(json["clockKind"], "monotonic");
    assert_eq!(json["previewAvailable"], false);
    assert!(json.get("positionUs").is_some());
    assert!(json["plans"][0].get("decodeToSourceUs").is_some());
}

#[test]
fn preview_command_requires_a_native_window_and_omits_pixels() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let err =
        preview_attach_impl(&state, "main".into(), PreviewHitMode::Consume, None).unwrap_err();
    assert!(err.contains("desktop window") || err.contains("not implemented"));
    let status = preview_status_impl(&state);
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["arrangement"], "child_overlay");
    assert!(json.get("pixels").is_none());
    assert!(json.get("samples").is_none());
    assert_eq!(json["copiesPerPresent"], 1);
}
