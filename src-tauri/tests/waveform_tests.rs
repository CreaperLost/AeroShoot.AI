#![cfg(unix)]
use aeroshoot_lib::{
    commands::*,
    fixtures::generate_pcm16_wav,
    project::{JournalRecord, ProjectBundle, TrackDescriptor, TrackType},
};
use std::fs;
use tempfile::tempdir;

#[test]
fn waveform_command_serializes_and_respects_gaps() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let mut bundle = ProjectBundle::create_new(dir.path(), "wave", "wave").unwrap();
    let samples = vec![16384i16; 4_800];
    let wav = generate_pcm16_wav(48_000, 1, &samples);
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
    bundle
        .journal()
        .append(JournalRecord::SegmentCommitted {
            seq: 0,
            track_id: "mic".into(),
            relative_path: "media/mic/000001.wav".into(),
            start_us: 0,
            end_us: 100_000,
            size_bytes: wav.len() as u64,
            is_keyframe_start: true,
            media_timescale: 48_000,
            media_start_value: 0,
            host_anchor_us: 0,
        })
        .unwrap();
    bundle.manifest_mut().duration_us = 100_000;
    bundle.manifest_mut().active_duration_us = 100_000;
    bundle
        .manifest()
        .save_with_backup(&bundle.root_path().join("manifest.json"))
        .unwrap();
    let path = bundle.root_path().to_path_buf();
    drop(bundle);

    let opened = open_project_impl(&state, path.to_string_lossy().into()).unwrap();
    let page = project_waveform_impl(
        &state,
        opened.project_handle.clone(),
        "mic".into(),
        0,
        100_000,
        8,
    )
    .unwrap();
    let json = serde_json::to_value(&page).unwrap();
    assert_eq!(json["channelPolicy"], "max_energy");
    assert_eq!(json["buckets"][0]["gap"], false);
    assert!((json["buckets"][0]["peak"].as_f64().unwrap() - 0.5).abs() < 0.05);
    assert!(!json.as_object().unwrap().contains_key("samples"));

    fs::remove_file(path.join("media/mic/000001.wav")).unwrap();
    let missing = project_waveform_impl(
        &state,
        opened.project_handle.clone(),
        "mic".into(),
        0,
        100_000,
        8,
    )
    .unwrap();
    assert!(missing.buckets.iter().all(|b| b.gap));
    assert!(!missing.diagnostics.is_empty());
    assert!(project_waveform_impl(
        &state,
        opened.project_handle.clone(),
        "mic".into(),
        0,
        100_000,
        0
    )
    .is_err());
    assert!(project_waveform_impl(
        &state,
        opened.project_handle.clone(),
        "screen".into(),
        0,
        100_000,
        8
    )
    .is_err());
}
