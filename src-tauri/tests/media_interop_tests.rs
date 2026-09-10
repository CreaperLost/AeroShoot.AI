#![cfg(unix)]
use aeroshoot_lib::{commands::*, media};
use tempfile::tempdir;

#[cfg(target_os = "macos")]
#[test]
fn decoder_handles_capture_dimensions_and_seeks_between_frame_times() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("1080.mp4");
    media::write_solid_h264(&path, 1920, 1080, 0.9, 0.1, 0.1).unwrap();
    let frame = media::decode_h264_frame(&path, 100_000).unwrap();
    assert_eq!((frame.width, frame.height), (1920, 1080));
    let path = dir.path().join("changing.mp4");
    let frames: Vec<_> = (0..8)
        .map(|i| {
            media::VideoFrame::solid(
                64,
                64,
                0,
                if i < 4 { 0 } else { 220 },
                if i < 4 { 220 } else { 0 },
                i * 1_000_000 / 30,
            )
            .unwrap()
        })
        .collect();
    media::encode_h264_frames(&path, &frames, 30).unwrap();
    let later = media::decode_h264_frame(&path, 150_000).unwrap();
    assert!(later.data[1] > later.data[2] + 100);
    assert!(later.pts_us <= 150_000);
    let early = media::decode_h264_frame(&path, 10_000).unwrap();
    assert!(early.data[2] > early.data[1] + 100);
    assert_eq!(early.pts_us, 0);
}

#[test]
fn media_status_omits_pixels_and_keeps_preview_unavailable() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let status = media_interop_status_impl(&state);
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["previewAvailable"], false);
    assert_eq!(json["ffmpegPinned"], false);
    assert_eq!(json["copiesComposite"], 2);
    assert_eq!(json["concurrentEncoderLimit"], 1);
    assert!(json.get("pixels").is_none());
    assert!(json.get("samples").is_none());
}

#[cfg(target_os = "macos")]
#[test]
fn preview_and_export_frames_match_within_tolerance() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let report = media_run_parity_impl(&state).expect("F2 parity pipeline");
    let json = serde_json::to_value(&report).unwrap();
    assert!(json.get("pixels").is_none());
    assert_eq!(json["ffmpegPinned"], false);
    assert_eq!(report.preview_width, 64);
    assert_eq!(report.export_width, 64);
    assert_eq!(report.concurrent_encoder_limit, 1);
    assert!(
        report.pcm_peak > 0.4,
        "PCM decode should see the 0.5-ish fixture, peak={}",
        report.pcm_peak
    );
    assert!(
        report.matched,
        "preview/export delta mean={} max={} compositor_mean={} compositor_max={} region={} diagnostics={:?}",
        report.mean_abs_delta,
        report.max_abs_delta,
        report.compositor_mean_delta,
        report.compositor_max_delta,
        report.region_mean_delta,
        report.diagnostics
    );
    assert!(report.mean_abs_delta <= media::PARITY_MEAN_TOLERANCE);
    assert!(report.compositor_mean_delta <= media::COMPOSITOR_MEAN_TOLERANCE);
}

#[cfg(target_os = "macos")]
#[test]
fn native_audio_clock_advances_without_a_microphone() {
    let mut output = aeroshoot_lib::playback::audio::AudioOutput::new().unwrap();
    for _ in 0..3 {
        output.queue(&vec![0; 9_600]).unwrap();
    }
    output.play();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while output.position_frames().unwrap() == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let first = output.position_frames().unwrap();
    assert!(first > 0);
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert!(output.position_frames().unwrap() >= first);
}

#[test]
fn short_native_mp4_passes_recording_storage_validation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("short.mp4");
    aeroshoot_lib::media::write_solid_h264(&path, 64, 64, 0.8, 0.2, 0.1).unwrap();
    let info = aeroshoot_lib::project::media_validator::MediaValidator::validate(
        &path,
        aeroshoot_lib::project::TrackType::Screen,
    )
    .unwrap();
    assert_eq!(info.sample_count, 6);
    assert!(info.duration_us.abs_diff(200_000) < 20_000);
    // Truncating encoded media must not be accepted as a completed recording.
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(std::fs::metadata(&path).unwrap().len() / 2)
        .unwrap();
    assert!(
        aeroshoot_lib::project::media_validator::MediaValidator::validate(
            &path,
            aeroshoot_lib::project::TrackType::Screen
        )
        .is_err()
    );
}
