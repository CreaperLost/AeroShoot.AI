//! Hardware recordings through the real lifecycle commands and the native
//! Windows recorder. They need real devices, so they are ignored by default:
//!
//!   cargo test --no-default-features --test windows_recording_tests -- --ignored --test-threads=1 --nocapture
//!
//! One recording at a time: native segment routing is process-wide, as in the app.
#![cfg(windows)]

use aeroshoot_lib::commands::*;
use aeroshoot_lib::project::journal::{JournalRecord, ProjectJournal};
use std::path::Path;
use std::time::Duration;
use tempfile::tempdir;

fn native_state(dir: &Path) -> AppState {
    let state = AppState::new(dir.to_path_buf());
    assert!(state.native_capture_enabled, "Windows records natively");
    state
}

fn options(mic_id: Option<String>, capture_system_audio: bool) -> StartRecordingOptions {
    StartRecordingOptions {
        source_id: "display:1".into(),
        capture_screen: false,
        camera_id: None,
        mic_id,
        capture_system_audio,
        fps: 30,
        resolution: "1080p".into(),
        layout: None,
        project_name: Some("Windows Hardware".into()),
        project_dir: None,
        mic_gain_db: None,
        video_bitrate_bps: None,
        capture_mouse: false,
        start_delay_ms: 0,
        camera: Default::default(),
    }
}

/// (track id, start_us, end_us, relative path) for every committed segment.
fn committed(project: &Path) -> Vec<(String, u64, u64, String)> {
    let journal = ProjectJournal::open_or_create(project).unwrap();
    journal
        .read_all()
        .unwrap()
        .into_iter()
        .filter_map(|record| match record {
            JournalRecord::SegmentCommitted {
                track_id,
                start_us,
                end_us,
                relative_path,
                ..
            } => Some((track_id, start_us, end_us, relative_path)),
            _ => None,
        })
        .collect()
}

#[test]
#[ignore = "needs a microphone and an audio output device"]
fn records_mic_and_system_audio_across_pause() {
    let mic = list_devices_impl()
        .mics
        .into_iter()
        .find(|mic| mic.is_default)
        .expect("a default microphone");
    let dir = tempdir().unwrap();
    let state = native_state(dir.path());

    let started = start_recording_impl(&state, options(Some(mic.id), true)).unwrap();
    std::thread::sleep(Duration::from_secs(3));
    pause_recording_impl(&state).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    resume_recording_impl(&state).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let status = get_session_status_impl(&state);
    println!("status before stop: {status:?}");
    let stopped = stop_recording_impl(&state).unwrap();

    let project = Path::new(started.project_path.as_deref().unwrap());
    let segments = committed(project);
    for segment in &segments {
        println!("{segment:?}");
    }
    for track in ["mic", "system"] {
        let track_segments: Vec<_> = segments.iter().filter(|s| s.0 == track).collect();
        // One segment before the pause, one after.
        assert_eq!(track_segments.len(), 2, "{track}: {track_segments:?}");
        for (_, start_us, end_us, path) in &track_segments {
            assert!(project.join(path).exists(), "{path}");
            assert!(end_us > start_us);
        }
        let first_len = track_segments[0].2 - track_segments[0].1;
        assert!(
            (2_500_000..3_600_000).contains(&first_len),
            "{track} first: {first_len}"
        );
    }
    let report = std::fs::read_to_string(project.join("qualification.json")).unwrap();
    println!("{report}");
    println!("stopped: {stopped:?}");
    assert!(report.contains("\"passed\": true") || report.contains("\"passed\":true"));
}

/// ffprobe's view of an MP4: (codec, profile, width, height, frames, duration s).
fn probe(path: &Path) -> Option<(String, String, u64, u64, u64, f64)> {
    let output = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,profile,width,height,nb_read_frames:format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let stream = &json["streams"][0];
    Some((
        stream["codec_name"].as_str()?.to_string(),
        stream["profile"].as_str().unwrap_or("").to_string(),
        stream["width"].as_u64()?,
        stream["height"].as_u64()?,
        stream["nb_read_frames"].as_str()?.parse().ok()?,
        json["format"]["duration"].as_str()?.parse().ok()?,
    ))
}

#[test]
#[ignore = "needs a display; ffprobe is used when installed"]
fn records_display_as_h264_across_pause() {
    let dir = tempdir().unwrap();
    let state = native_state(dir.path());
    let mut opts = options(None, false);
    opts.capture_screen = true;
    opts.source_id = list_capture_sources_impl()
        .into_iter()
        .find(|source| source.id.starts_with("display:"))
        .expect("a display")
        .id;

    let started = start_recording_impl(&state, opts).unwrap();
    std::thread::sleep(Duration::from_secs(3));
    pause_recording_impl(&state).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    resume_recording_impl(&state).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    println!("status before stop: {:?}", get_session_status_impl(&state));
    stop_recording_impl(&state).unwrap();

    let project = Path::new(started.project_path.as_deref().unwrap());
    let segments: Vec<_> = committed(project)
        .into_iter()
        .filter(|s| s.0 == "screen")
        .collect();
    assert_eq!(segments.len(), 2, "{segments:?}");
    for (_, start_us, end_us, path) in &segments {
        let file = project.join(path);
        let probed = probe(&file);
        println!("{path}: {start_us}..{end_us} {probed:?}");
        if let Some((codec, profile, width, height, frames, seconds)) = probed {
            assert_eq!(codec, "h264");
            assert_eq!(profile, "High");
            assert_eq!((width, height), (1920, 1080));
            // 30 fps constant rate.
            assert!(
                (frames as f64 - seconds * 30.0).abs() <= 2.0,
                "{frames} frames in {seconds}s"
            );
        }
    }
    let first = &segments[0];
    assert!(
        (2_500_000..3_600_000).contains(&(first.2 - first.1)),
        "{first:?}"
    );
    let report = std::fs::read_to_string(project.join("qualification.json")).unwrap();
    assert!(report.contains("\"passed\": true"), "{report}");
}

#[test]
#[ignore = "needs a camera; ffprobe is used when installed"]
fn records_camera_as_h264() {
    let camera = list_devices_impl()
        .cameras
        .into_iter()
        .next()
        .expect("a camera");
    let dir = tempdir().unwrap();
    let state = native_state(dir.path());
    let mut opts = options(None, false);
    opts.camera_id = Some(camera.id.clone());
    // A camera format of its own, unlike the 1280×720 default.
    opts.camera = CameraOptions {
        camera_width: Some(854),
        camera_height: Some(480),
        camera_fps: Some(30),
        camera_bitrate_bps: Some(4_000_000),
    };

    // As in the app: the live preview holds the camera when Record is pressed,
    // and the recording takes it over.
    aeroshoot_lib::capture::preview::start("display:1", true, false, Some(&camera.id), None, 0.0)
        .unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let started = start_recording_impl(&state, opts).unwrap();
    std::thread::sleep(Duration::from_secs(4));
    println!("status before stop: {:?}", get_session_status_impl(&state));
    stop_recording_impl(&state).unwrap();

    let project = Path::new(started.project_path.as_deref().unwrap());
    let segments: Vec<_> = committed(project)
        .into_iter()
        .filter(|s| s.0 == "webcam")
        .collect();
    assert_eq!(segments.len(), 1, "{segments:?}");
    let (_, start_us, end_us, path) = &segments[0];
    if let Ok(keep) = std::env::var("AEROSHOOT_KEEP_SEGMENT") {
        std::fs::copy(project.join(path), keep).unwrap();
    }
    let probed = probe(&project.join(path));
    println!("{path}: {start_us}..{end_us} {probed:?}");
    if let Some((codec, _, width, height, frames, seconds)) = probed {
        assert_eq!(codec, "h264");
        assert_eq!((width, height), (854, 480));
        assert!(
            (seconds * 25.0..=seconds * 35.0).contains(&(frames as f64)),
            "{frames} frames in {seconds}s, expected about 30 fps"
        );
    }
    assert!(end_us - start_us >= 3_000_000);
}

#[test]
#[ignore = "needs a display, microphone, and audio output; runs for over two minutes"]
fn records_all_sources_with_countdown_and_rotation() {
    let mic = list_devices_impl()
        .mics
        .into_iter()
        .find(|mic| mic.is_default)
        .expect("a default microphone");
    let dir = tempdir().unwrap();
    let state = native_state(dir.path());
    let mut opts = options(Some(mic.id), true);
    opts.capture_screen = true;
    opts.fps = 60;
    opts.start_delay_ms = 3_000;

    let started = start_recording_impl(&state, opts).unwrap();
    std::thread::sleep(Duration::from_secs(3 + 125));
    println!("status before stop: {:?}", get_session_status_impl(&state));
    stop_recording_impl(&state).unwrap();

    let project = Path::new(started.project_path.as_deref().unwrap());
    let segments = committed(project);
    for track in ["screen", "mic", "system"] {
        let track_segments: Vec<_> = segments.iter().filter(|s| s.0 == track).collect();
        println!("{track}: {track_segments:?}");
        // 125 s at a 55 s cadence: three segments, each under 60 s, back to back.
        assert_eq!(track_segments.len(), 3, "{track}");
        for pair in track_segments.windows(2) {
            let gap = pair[1].1 as i64 - pair[0].2 as i64;
            assert!(gap.abs() < 100_000, "{track} gap {gap} between {pair:?}");
        }
        for (_, start_us, end_us, _) in &track_segments {
            assert!(end_us - start_us <= 60_000_000);
        }
    }
    let screen = segments.iter().find(|s| s.0 == "screen").unwrap();
    if let Some((_, _, _, _, frames, seconds)) = probe(&project.join(&screen.3)) {
        println!("screen segment 1: {frames} frames in {seconds}s");
        assert!((frames as f64 - seconds * 60.0).abs() <= 3.0);
    }
    let report = std::fs::read_to_string(project.join("qualification.json")).unwrap();
    println!("{report}");
    assert!(report.contains("\"passed\": true"));
}

/// Nudge the pointer right and back through the input stack (no clicks), so
/// the low-level hook sees real moves.
fn nudge_pointer() -> std::process::Child {
    let script = r#"
Add-Type -Namespace Win32 -Name Input -MemberDefinition '[DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, System.UIntPtr extra);'
Start-Sleep -Milliseconds 800
for ($i = 0; $i -lt 20; $i++) { [Win32.Input]::mouse_event(1, 5, 0, 0, [System.UIntPtr]::Zero); Start-Sleep -Milliseconds 25 }
for ($i = 0; $i -lt 20; $i++) { [Win32.Input]::mouse_event(1, -5, 0, 0, [System.UIntPtr]::Zero); Start-Sleep -Milliseconds 25 }
"#;
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", script])
        .spawn()
        .expect("powershell")
}

#[test]
#[ignore = "needs a display and moves the pointer briefly"]
fn records_pointer_telemetry_with_cursor_replaced() {
    let dir = tempdir().unwrap();
    let state = native_state(dir.path());
    let mut opts = options(None, false);
    opts.capture_screen = true;
    opts.capture_mouse = true;
    opts.source_id = list_capture_sources_impl()
        .into_iter()
        .find(|source| source.id.starts_with("display:"))
        .expect("a display")
        .id;

    let started = start_recording_impl(&state, opts).unwrap();
    let mut mover = nudge_pointer();
    std::thread::sleep(Duration::from_secs(3));
    let _ = mover.wait();
    stop_recording_impl(&state).unwrap();

    let project = Path::new(started.project_path.as_deref().unwrap());
    let manifest = std::fs::read_to_string(project.join("manifest.json")).unwrap();
    assert!(
        manifest.contains("\"cursorMode\": \"replace\""),
        "{manifest}"
    );

    let geometry = std::fs::read_to_string(project.join("telemetry/geometry.jsonl")).unwrap();
    println!("{geometry}");
    assert!(geometry.contains("windows_virtual_screen"));
    let events = std::fs::read_to_string(project.join("telemetry/events.jsonl")).unwrap();
    let moves = events
        .lines()
        .filter(|l| l.contains("\"kind\":\"move\""))
        .count();
    println!("{} event records, {moves} moves", events.lines().count());
    for line in events.lines().filter(|l| !l.contains("\"kind\":\"move\"")) {
        println!("{line}");
    }
    assert!(moves >= 5, "expected pointer moves");
    assert!(events.contains("cursor_changed"));

    let stream = aeroshoot_lib::telemetry::reader::read_telemetry(project).unwrap();
    assert!(stream.diagnostics.is_empty(), "{:?}", stream.diagnostics);
    assert!(
        stream.geometries.values().all(|g| g.supported),
        "{:?}",
        stream.geometries
    );

    let report = std::fs::read_to_string(project.join("qualification.json")).unwrap();
    let start = report.find("\"mouseTelemetry\"").unwrap_or(0);
    println!("{}", &report[start..report.len().min(start + 900)]);
    assert!(report.contains("\"passed\": true"), "{report}");
}
