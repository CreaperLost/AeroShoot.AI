use aeroshoot_lib::{
    commands::*,
    dsp::SilenceConfig,
    fixtures::generate_pcm16_wav,
    project::{JournalRecord, OpenedProject, ProjectBundle, TrackDescriptor, TrackType},
};
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

const SAMPLE_RATE: u32 = 48_000;

fn speech(frames: usize) -> Vec<i16> {
    vec![16_384; frames]
}

fn silent(frames: usize) -> Vec<i16> {
    vec![0; frames]
}

fn concat(parts: &[Vec<i16>]) -> Vec<i16> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(part);
    }
    out
}

struct WavSeg {
    file: &'static str,
    start_us: u64,
    samples: Vec<i16>,
    channels: u16,
    write: bool,
}

fn open_mic_project(segs: &[WavSeg]) -> (tempfile::TempDir, AppState, OpenedProject, PathBuf) {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().into());
    let mut bundle = ProjectBundle::create_new(dir.path(), "silence", "silence").unwrap();
    bundle.manifest_mut().tracks.push(TrackDescriptor {
        id: "mic".into(),
        track_type: TrackType::MicAudio,
        codec: "pcm".into(),
        relative_path: format!("media/mic/{}", segs[0].file),
        width: None,
        height: None,
        fps: None,
        sample_rate: Some(SAMPLE_RATE),
        channels: Some(segs[0].channels),
        gaps_total: 0,
        media_timescale: Some(SAMPLE_RATE),
    });
    let mut duration_us = 0u64;
    for (seq, seg) in segs.iter().enumerate() {
        let wav = generate_pcm16_wav(SAMPLE_RATE, seg.channels, &seg.samples);
        let relative = format!("media/mic/{}", seg.file);
        let path = bundle.root_path().join(&relative);
        if seg.write {
            fs::write(&path, &wav).unwrap();
        }
        let end_us = seg.start_us
            + (seg.samples.len() as u64 / seg.channels as u64) * 1_000_000 / SAMPLE_RATE as u64;
        duration_us = duration_us.max(end_us);
        bundle
            .journal()
            .append(JournalRecord::SegmentCommitted {
                seq: seq as u64,
                track_id: "mic".into(),
                relative_path: relative,
                start_us: seg.start_us,
                end_us,
                size_bytes: wav.len() as u64,
                is_keyframe_start: true,
                media_timescale: SAMPLE_RATE,
                media_start_value: 0,
                host_anchor_us: seg.start_us as i64,
            })
            .unwrap();
    }
    bundle.manifest_mut().duration_us = duration_us;
    bundle.manifest_mut().active_duration_us = duration_us;
    bundle
        .manifest()
        .save_with_backup(&bundle.root_path().join("manifest.json"))
        .unwrap();
    let path = bundle.root_path().to_path_buf();
    drop(bundle);
    let opened = open_project_impl(&state, path.to_string_lossy().into()).unwrap();
    (dir, state, opened, path)
}

fn crossing_silence_files() -> [WavSeg; 2] {
    let half = SAMPLE_RATE as usize / 2;
    let one = SAMPLE_RATE as usize;
    [
        WavSeg {
            file: "000001.wav",
            start_us: 0,
            samples: concat(&[speech(half), silent(one)]),
            channels: 1,
            write: true,
        },
        WavSeg {
            file: "000002.wav",
            start_us: 1_500_000,
            samples: concat(&[silent(one), speech(half)]),
            channels: 1,
            write: true,
        },
    ]
}

#[test]
fn silent_run_across_two_contiguous_wavs_is_one_suggestion() {
    let (_dir, state, opened, _) = open_mic_project(&crossing_silence_files());
    let result = detect_silence_impl(
        &state,
        opened.project_handle.clone(),
        "mic".into(),
        SilenceConfig::default(),
    )
    .unwrap();
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["channelPolicy"], "max_energy");
    assert_eq!(json["trackId"], "mic");
    assert!(json["suggestions"][0]["startUs"].is_number());
    assert!(json["suggestions"][0]["sourceStartUs"].is_number());
    assert_eq!(result.suggestions.len(), 1, "{result:?}");
    let cut = &result.suggestions[0];
    assert!(cut.start_us >= 500_000, "{}", cut.start_us);
    assert!(cut.end_us <= 2_500_000, "{}", cut.end_us);
    assert!(cut.duration_ms >= 1_800);
    assert_eq!(cut.start_us, cut.source_start_us);
    assert_eq!(cut.end_us, cut.source_end_us);
}

#[test]
fn missing_file_is_not_classified_as_silence() {
    let one = SAMPLE_RATE as usize;
    let segs = [
        WavSeg {
            file: "000001.wav",
            start_us: 0,
            samples: silent(one),
            channels: 1,
            write: true,
        },
        WavSeg {
            file: "000002.wav",
            start_us: 1_000_000,
            samples: silent(one),
            channels: 1,
            write: true,
        },
        WavSeg {
            file: "000003.wav",
            start_us: 2_000_000,
            samples: silent(one),
            channels: 1,
            write: true,
        },
    ];
    let (_dir, state, opened, path) = open_mic_project(&segs);
    fs::remove_file(path.join("media/mic/000002.wav")).unwrap();
    let result = detect_silence_impl(
        &state,
        opened.project_handle.clone(),
        "mic".into(),
        SilenceConfig {
            padding_ms: 0,
            ..SilenceConfig::default()
        },
    )
    .unwrap();
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.to_lowercase().contains("missing")
                && d.to_lowercase().contains("not silence")),
        "{:?}",
        result.diagnostics
    );
    assert_eq!(result.suggestions.len(), 2, "{result:?}");
    assert!(result
        .suggestions
        .iter()
        .all(|s| s.end_us <= 1_000_000 || s.start_us >= 2_000_000));
    assert!(result
        .suggestions
        .iter()
        .all(|s| !(s.start_us < 1_000_000 && s.end_us > 2_000_000)));
}

#[test]
fn opposite_polarity_stereo_is_not_a_cut() {
    let frames = SAMPLE_RATE as usize * 2;
    let mut samples = Vec::with_capacity(frames * 2);
    for _ in 0..frames {
        samples.push(16_384);
        samples.push(-16_384);
    }
    let (_dir, state, opened, _) = open_mic_project(&[WavSeg {
        file: "000001.wav",
        start_us: 0,
        samples,
        channels: 2,
        write: true,
    }]);
    let result = detect_silence_impl(
        &state,
        opened.project_handle.clone(),
        "mic".into(),
        SilenceConfig::default(),
    )
    .unwrap();
    assert!(result.suggestions.is_empty(), "{result:?}");
}

#[test]
fn apply_selected_cuts_is_one_revision_and_undo_reopen_preserve() {
    let (_dir, state, opened, path) = open_mic_project(&crossing_silence_files());
    let handle = opened.project_handle.clone();
    let original = opened.retained_intervals.clone();
    let detected = detect_silence_impl(
        &state,
        handle.clone(),
        "mic".into(),
        SilenceConfig::default(),
    )
    .unwrap();
    assert_eq!(detected.suggestions.len(), 1);
    let cuts: Vec<EditCut> = detected
        .suggestions
        .iter()
        .map(|s| EditCut {
            start_us: s.start_us,
            end_us: s.end_us,
        })
        .collect();
    let applied =
        project_ripple_cuts_impl(&state, handle.clone(), opened.revision, cuts.clone()).unwrap();
    assert_eq!(applied.revision, opened.revision + 1);
    assert_ne!(applied.retained_intervals, original);
    assert_eq!(applied.retained_intervals.len(), 2);

    let undone = project_undo_impl(&state, handle.clone(), applied.revision).unwrap();
    assert_eq!(undone.retained_intervals, original);

    let reapplied =
        project_ripple_cuts_impl(&state, handle.clone(), undone.revision, cuts).unwrap();
    let persisted = reapplied.retained_intervals.clone();
    close_project_impl(&state, handle.clone()).unwrap();
    let reopened = open_project_impl(&state, path.to_string_lossy().into()).unwrap();
    assert_eq!(reopened.retained_intervals, persisted);
}

#[test]
fn stale_handle_and_invalid_config_are_rejected() {
    let (_dir, state, opened, _) = open_mic_project(&crossing_silence_files());
    let stale = detect_silence_impl(
        &state,
        "stale".into(),
        "mic".into(),
        SilenceConfig::default(),
    )
    .unwrap_err();
    assert!(stale.contains("Stale"), "{stale}");

    let zero = detect_silence_impl(
        &state,
        opened.project_handle.clone(),
        "mic".into(),
        SilenceConfig {
            window_ms: Some(0),
            ..SilenceConfig::default()
        },
    )
    .unwrap_err();
    assert!(zero.to_lowercase().contains("window"), "{zero}");

    let nan = detect_silence_impl(
        &state,
        opened.project_handle.clone(),
        "mic".into(),
        SilenceConfig {
            threshold_db: f32::NAN,
            ..SilenceConfig::default()
        },
    )
    .unwrap_err();
    assert!(nan.to_lowercase().contains("finite"), "{nan}");
}

#[test]
fn unsupported_encoding_is_a_gap_not_silence() {
    let one = SAMPLE_RATE as usize;
    let (_dir, state, opened, path) = open_mic_project(&[
        WavSeg {
            file: "000001.wav",
            start_us: 0,
            samples: speech(one),
            channels: 1,
            write: true,
        },
        WavSeg {
            file: "000002.wav",
            start_us: 1_000_000,
            samples: silent(one),
            channels: 1,
            write: true,
        },
        WavSeg {
            file: "000003.wav",
            start_us: 2_000_000,
            samples: speech(one),
            channels: 1,
            write: true,
        },
    ]);
    let mut bad = generate_pcm16_wav(SAMPLE_RATE, 1, &silent(one));
    bad[20] = 7;
    bad[21] = 0;
    fs::write(path.join("media/mic/000002.wav"), bad).unwrap();
    let result = detect_silence_impl(
        &state,
        opened.project_handle.clone(),
        "mic".into(),
        SilenceConfig::default(),
    )
    .unwrap();
    assert!(result.suggestions.is_empty(), "{result:?}");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.to_lowercase().contains("encoding")
                || d.to_lowercase().contains("unsupported")),
        "{:?}",
        result.diagnostics
    );
}
