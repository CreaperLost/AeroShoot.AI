//! Generate a silent 16-second two-segment project for native editor smoke tests.
use aeroshoot_lib::{
    fixtures::generate_pcm16_wav,
    media::{encode_h264_frames, VideoFrame},
    project::{JournalRecord, ProjectBundle, TrackDescriptor, TrackType},
};
use std::{
    fs,
    path::{Path, PathBuf},
};
fn two_color_project(base: &Path, id: &str) -> (PathBuf, PathBuf, PathBuf) {
    let mut bundle = ProjectBundle::create_new(base, id, id).unwrap();
    let screen_a = bundle.root_path().join("media/screen/000001.mp4");
    let screen_b = bundle.root_path().join("media/screen/000002.mp4");
    encode_h264_frames(
        &screen_a,
        &vec![VideoFrame::solid(1280, 720, 20, 30, 230, 0).unwrap(); 8],
        1,
    )
    .unwrap();
    encode_h264_frames(
        &screen_b,
        &vec![VideoFrame::solid(1280, 720, 40, 220, 20, 0).unwrap(); 8],
        1,
    )
    .unwrap();
    let wav = generate_pcm16_wav(48_000, 1, &vec![0i16; 768_000]);
    let mic = bundle.root_path().join("media/mic/000001.wav");
    fs::write(&mic, &wav).unwrap();
    bundle.manifest_mut().tracks.push(TrackDescriptor {
        id: "screen".into(),
        track_type: TrackType::Screen,
        codec: "h264".into(),
        relative_path: "media/screen/000001.mp4".into(),
        width: Some(1280),
        height: Some(720),
        fps: Some(1),
        sample_rate: None,
        channels: None,
        gaps_total: 0,
        media_timescale: Some(1),
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
            end_us: 8_000_000,
            size_bytes: fs::metadata(&screen_a).unwrap().len(),
            is_keyframe_start: true,
            media_timescale: 1,
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
            start_us: 8_000_000,
            end_us: 16_000_000,
            size_bytes: fs::metadata(&screen_b).unwrap().len(),
            is_keyframe_start: true,
            media_timescale: 1,
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
            end_us: 16_000_000,
            size_bytes: wav.len() as u64,
            is_keyframe_start: true,
            media_timescale: 48_000,
            media_start_value: 0,
            host_anchor_us: 0,
        })
        .unwrap();
    bundle.manifest_mut().duration_us = 16_000_000;
    bundle.manifest_mut().active_duration_us = 16_000_000;
    bundle
        .manifest()
        .save_with_backup(&bundle.root_path().join("manifest.json"))
        .unwrap();
    let root = bundle.root_path().to_path_buf();
    drop(bundle);
    (root, screen_a, mic)
}

fn main() {
    let base = PathBuf::from(std::env::args().nth(1).expect("destination directory"));
    fs::create_dir_all(&base).unwrap();
    println!("{}", two_color_project(&base, "studio-smoke").0.display());
}
