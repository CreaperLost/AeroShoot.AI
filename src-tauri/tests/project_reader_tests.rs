#![cfg(unix)]
use aeroshoot_lib::{
    commands::*,
    fixtures::generate_valid_wav_segment,
    project::{
        lock::ProjectLock, JournalRecord, OpenedProject, ProjectBundle, TrackDescriptor, TrackType,
    },
};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::tempdir;

fn fixture(base: &Path, id: &str, duration: u64, count: usize) -> PathBuf {
    let mut bundle = ProjectBundle::create_new(base, id, id).unwrap();
    for n in 0..count {
        let track_id = if n == 0 { "mic" } else { "system" };
        let track = TrackDescriptor {
            id: track_id.into(),
            track_type: if n == 0 {
                TrackType::MicAudio
            } else {
                TrackType::SystemAudio
            },
            codec: "pcm".into(),
            relative_path: format!("media/{track_id}/000001.wav"),
            width: None,
            height: None,
            fps: None,
            sample_rate: Some(48_000),
            channels: Some(1),
            gaps_total: 0,
            media_timescale: Some(48_000),
        };
        let data = generate_valid_wav_segment(duration, 48_000, 1);
        let relative_path = format!("media/{track_id}/000001.wav");
        fs::write(bundle.root_path().join(&relative_path), &data).unwrap();
        bundle.manifest_mut().tracks.push(track);
        bundle
            .journal()
            .append(JournalRecord::SegmentCommitted {
                seq: 0,
                track_id: track_id.into(),
                relative_path,
                start_us: 0,
                end_us: duration,
                size_bytes: data.len() as u64,
                is_keyframe_start: true,
                media_timescale: 48_000,
                media_start_value: 0,
                host_anchor_us: 0,
            })
            .unwrap();
    }
    bundle.manifest_mut().duration_us = duration;
    bundle.manifest_mut().active_duration_us = duration;
    bundle
        .manifest()
        .save_with_backup(&bundle.root_path().join("manifest.json"))
        .unwrap();
    bundle.root_path().to_path_buf()
}

fn screen_two_segments(base: &Path, id: &str) -> PathBuf {
    let mut bundle = ProjectBundle::create_new(base, id, id).unwrap();
    let data_a = generate_valid_wav_segment(100_000, 48_000, 1);
    let data_b = generate_valid_wav_segment(150_000, 48_000, 1);
    fs::write(bundle.root_path().join("media/screen/000001.wav"), &data_a).unwrap();
    fs::write(bundle.root_path().join("media/screen/000002.wav"), &data_b).unwrap();
    bundle.manifest_mut().tracks.push(TrackDescriptor {
        id: "screen".into(),
        track_type: TrackType::Screen,
        codec: "pcm".into(),
        relative_path: "media/screen/000001.wav".into(),
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
            relative_path: "media/screen/000001.wav".into(),
            start_us: 0,
            end_us: 100_000,
            size_bytes: data_a.len() as u64,
            is_keyframe_start: true,
            media_timescale: 48_000,
            media_start_value: 0,
            host_anchor_us: 0,
        })
        .unwrap();
    bundle
        .journal()
        .append(JournalRecord::SegmentCommitted {
            seq: 0,
            track_id: "screen".into(),
            relative_path: "media/screen/000002.wav".into(),
            start_us: 100_000,
            end_us: 250_000,
            size_bytes: data_b.len() as u64,
            is_keyframe_start: true,
            media_timescale: 48_000,
            media_start_value: 4_800,
            host_anchor_us: 100_000,
        })
        .unwrap();
    bundle.manifest_mut().duration_us = 250_000;
    bundle.manifest_mut().active_duration_us = 250_000;
    bundle
        .manifest()
        .save_with_backup(&bundle.root_path().join("manifest.json"))
        .unwrap();
    bundle.root_path().to_path_buf()
}

fn open(state: &AppState, path: &Path) -> OpenedProject {
    open_project_impl(state, path.to_string_lossy().into()).unwrap()
}

#[test]
fn command_open_close_reopen_and_paged_serialization() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let a = fixture(dir.path(), "one", 100_000, 1);
    let b = fixture(dir.path(), "two", 200_000, 2);
    let before = fs::read(a.join("journal.jsonl")).unwrap();
    let first = open(&state, &a);
    let json = serde_json::to_value(&first).unwrap();
    assert_eq!(json["tracks"][0]["descriptor"]["trackType"], "mic_audio");
    assert_eq!(json["editedDurationUs"], 100_000);
    assert_eq!(json["previewAvailable"], false);
    assert!(!json.as_object().unwrap().contains_key("segments"));
    assert!(ProjectLock::acquire(&a).is_err());
    let page =
        project_segments_impl(&state, first.project_handle.clone(), "mic".into(), 0, 1).unwrap();
    assert_eq!(page.segments.len(), 1);
    assert!(page.segments[0].available);
    assert_eq!(page.next_offset, None);
    assert_eq!(
        serde_json::to_value(page).unwrap()["segments"][0]["mediaTimescale"],
        48_000
    );
    assert!(
        project_segments_impl(&state, first.project_handle.clone(), "mic".into(), 0, 257).is_err()
    );
    let second = open(&state, &b);
    assert_eq!(second.tracks.len(), 2);
    assert_eq!(second.source_duration_us, 200_000);
    assert!(
        project_segments_impl(&state, first.project_handle.clone(), "mic".into(), 0, 1).is_err()
    );
    assert!(close_project_impl(&state, first.project_handle).is_err());
    close_project_impl(&state, second.project_handle).unwrap();
    let reopened = open(&state, &a);
    assert_eq!(reopened.manifest, first.manifest);
    assert_eq!(fs::read(a.join("journal.jsonl")).unwrap(), before);
    assert!(!a.join(".lock").exists());
}

#[test]
fn failed_replacement_preserves_current_handle_and_playback() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let a = fixture(dir.path(), "current", 100_000, 1);
    let opened = open(&state, &a);
    playback_seek_impl(&state, opened.project_handle.clone(), 50_000).unwrap();
    assert!(
        open_project_impl(&state, dir.path().join("missing").to_string_lossy().into()).is_err()
    );
    let status = playback_status_impl(&state, opened.project_handle.clone()).unwrap();
    assert_eq!(status.position_us, 50_000);
    assert_eq!(
        project_segments_impl(&state, opened.project_handle.clone(), "mic".into(), 0, 1)
            .unwrap()
            .segments
            .len(),
        1
    );
    assert!(ProjectLock::acquire(&a).is_err());
    close_project_impl(&state, opened.project_handle).unwrap();
}

#[test]
fn later_segments_index_even_when_track_relative_path_is_the_first_file() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let path = screen_two_segments(dir.path(), "screen-two");
    let summary = open(&state, &path);
    assert_eq!(summary.tracks.len(), 1);
    assert_eq!(summary.tracks[0].segment_count, 2);
    assert_eq!(summary.tracks[0].available_segment_count, 2);
    assert_eq!(summary.source_duration_us, 250_000);
    let page = project_segments_impl(
        &state,
        summary.project_handle.clone(),
        "screen".into(),
        0,
        2,
    )
    .unwrap();
    assert_eq!(page.segments.len(), 2);
    assert_eq!(page.segments[0].relative_path, "media/screen/000001.wav");
    assert_eq!(page.segments[1].relative_path, "media/screen/000002.wav");
    assert_eq!(page.segments[1].start_us, 100_000);
    assert_eq!(page.segments[1].host_anchor_us, 100_000);
    let json = serde_json::to_value(&summary).unwrap();
    assert_eq!(json["sourceDurationUs"], 250_000);
    assert_eq!(json["retainedIntervals"][0]["endUs"], 250_000);
}

#[test]
fn malformed_paths_versions_and_active_writers_do_not_mutate_project() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let path = fixture(dir.path(), "safe", 100_000, 1);
    let bytes = fs::read(path.join("manifest.json")).unwrap();
    let lock = ProjectLock::acquire(&path).unwrap();
    assert!(open_project_impl(&state, path.to_string_lossy().into()).is_err());
    drop(lock);
    assert!(open_project_impl(&state, format!("{}/../safe", path.display())).is_err());
    let mut manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    manifest["version"] = serde_json::json!(999);
    fs::write(path.join("manifest.json"), manifest.to_string()).unwrap();
    let bad = fs::read(path.join("manifest.json")).unwrap();
    assert!(open_project_impl(&state, path.to_string_lossy().into()).is_err());
    assert_eq!(fs::read(path.join("manifest.json")).unwrap(), bad);
    fs::write(path.join("manifest.json"), &bytes).unwrap();
    fs::remove_file(path.join("media/mic/000001.wav")).unwrap();
    fs::remove_dir(path.join("media/mic")).unwrap();
    std::os::unix::fs::symlink(dir.path(), path.join("media/mic")).unwrap();
    assert!(open_project_impl(&state, path.to_string_lossy().into()).is_err());
    assert_eq!(fs::read(path.join("manifest.json")).unwrap(), bytes);
}

#[test]
fn missing_media_and_partial_tail_are_diagnostics_but_corrupt_interior_fails() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let path = fixture(dir.path(), "missing", 100_000, 1);
    fs::remove_file(path.join("media/mic/000001.wav")).unwrap();
    let journal = path.join("journal.jsonl");
    let original = fs::read(&journal).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap()
        .write_all(b"{\"type\":")
        .unwrap();
    let before = fs::read(&journal).unwrap();
    let summary = open(&state, &path);
    assert_eq!(summary.tracks[0].available_segment_count, 0);
    assert_eq!(summary.diagnostics.len(), 2);
    assert_eq!(fs::read(&journal).unwrap(), before);
    close_project_impl(&state, summary.project_handle).unwrap();
    let mut corrupt = original.clone();
    corrupt.extend_from_slice(b"{bad}\n");
    fs::write(&journal, &corrupt).unwrap();
    assert!(open_project_impl(&state, path.to_string_lossy().into()).is_err());
    assert_eq!(fs::read(&journal).unwrap(), corrupt);
    let mut unknown = original;
    unknown.extend_from_slice(b"{\"type\":\"future_schema\"}");
    fs::write(&journal, unknown).unwrap();
    assert!(open_project_impl(&state, path.to_string_lossy().into()).is_err());
}

#[test]
fn oversized_inputs_and_fifo_are_rejected() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let path = fixture(dir.path(), "large", 100_000, 1);
    fs::write(path.join("journal.jsonl"), vec![b' '; 65_537]).unwrap();
    assert!(open_project_impl(&state, path.to_string_lossy().into()).is_err());
    fs::remove_file(path.join("journal.jsonl")).unwrap();
    let fifo = std::ffi::CString::new(path.join("journal.jsonl").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(open_project_impl(&state, path.to_string_lossy().into()).is_err());
}

#[test]
fn stop_response_resolves_real_bundle_and_retry_preserves_identity() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    start_recording_impl(
        &state,
        StartRecordingOptions {
            source_id: "screen-main".into(),
            camera_id: None,
            mic_id: None,
            capture_system_audio: false,
            fps: 30,
            resolution: "1080p".into(),
        },
    )
    .unwrap();
    let result = stop_recording_impl(&state).unwrap();
    assert!(Path::new(&result.project_path).is_dir());
    assert_eq!(stop_recording_impl(&state).unwrap(), result);
    let summary = open_project_impl(&state, result.project_path).unwrap();
    assert_eq!(summary.manifest.session_id, result.session_id);
    assert_eq!(summary.tracks.len(), 1);
    assert_eq!(summary.tracks[0].descriptor.id, "screen");
}

#[test]
fn empty_bundle_opens_with_no_fabricated_tracks() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let path = {
        let bundle = ProjectBundle::create_new(dir.path(), "empty", "Empty").unwrap();
        bundle.root_path().to_path_buf()
    };
    let summary = open(&state, &path);
    assert!(summary.tracks.is_empty());
    assert_eq!(summary.source_duration_us, 0);
    assert_eq!(summary.edited_duration_us, 0);
    assert!(!summary.preview_available);
}
