//! Project-scoped silence analysis over E2 PCM segments.
use super::manifest::TrackType;
use super::pcm::{PcmReader, READ_FRAME_CHUNK};
use super::reader::safe_path;
use super::waveform::WaveformTrackContext;
use crate::dsp::silence::{
    channel_policy_name, finalize_regions, ChannelPolicy, SilenceConfig, SilenceCutInterval,
    SilenceDetectionResult, StreamingSilenceDetector,
};
use crate::project::revision::MAX_CUTS_PER_REVISION;
use crate::timeline::{SourceInterval, TimelineMapper};
use std::fs;

const MAX_DIAGNOSTICS: usize = 32;

pub fn detect_track_silence(
    ctx: &WaveformTrackContext,
    config: &SilenceConfig,
) -> Result<SilenceDetectionResult, String> {
    config.validate()?;
    if !matches!(ctx.track_type, TrackType::MicAudio | TrackType::SystemAudio) {
        return Err("Track is not audio".into());
    }
    let policy = config.resolved_policy()?;
    let mut diagnostics = Vec::new();
    let mut raw_regions = Vec::new();
    let mut detector: Option<StreamingSilenceDetector> = None;
    let mut sample_rate = 0u32;
    let mut channels = 0u16;

    if ctx.segments.is_empty() {
        push_diagnostic(
            &mut diagnostics,
            "Audio track is empty; no PCM segments to analyze",
        );
        return Ok(SilenceDetectionResult {
            track_id: ctx.track_id.clone(),
            sample_rate,
            channels,
            channel_policy: channel_policy_name(policy),
            suggestions: Vec::new(),
            diagnostics,
        });
    }

    for segment in &ctx.segments {
        let path = match safe_path(&ctx.root, &segment.relative_path) {
            Ok(path) => path,
            Err(error) => {
                reset_detector(&mut detector, &mut raw_regions);
                push_diagnostic(
                    &mut diagnostics,
                    &format!(
                        "Unsafe audio path treated as a gap, not silence: {} ({error})",
                        segment.relative_path
                    ),
                );
                continue;
            }
        };
        if !segment.available || !path.is_file() {
            reset_detector(&mut detector, &mut raw_regions);
            push_diagnostic(
                &mut diagnostics,
                &format!(
                    "Missing file treated as a gap, not silence: {}",
                    segment.relative_path
                ),
            );
            continue;
        }
        let file_len = match fs::metadata(&path) {
            Ok(meta) => meta.len(),
            Err(_) => {
                reset_detector(&mut detector, &mut raw_regions);
                push_diagnostic(
                    &mut diagnostics,
                    &format!(
                        "Missing file treated as a gap, not silence: {}",
                        segment.relative_path
                    ),
                );
                continue;
            }
        };
        if file_len != segment.size_bytes {
            reset_detector(&mut detector, &mut raw_regions);
            push_diagnostic(
                &mut diagnostics,
                &format!(
                    "Size-mismatched audio treated as a gap, not silence: {}",
                    segment.relative_path
                ),
            );
            continue;
        }

        let mut reader = match PcmReader::open(&path) {
            Ok(reader) => reader,
            Err(error) => {
                reset_detector(&mut detector, &mut raw_regions);
                push_diagnostic(
                    &mut diagnostics,
                    &format!(
                        "Unsupported encoding treated as a gap, not silence: {} ({error})",
                        segment.relative_path
                    ),
                );
                continue;
            }
        };
        let info = reader.info().clone();
        if let Some(active) = detector.as_mut() {
            if active.sample_rate() != info.sample_rate || active.channels() != info.channels {
                raw_regions.extend(active.take_raw_regions());
                detector = None;
            }
        }
        if detector.is_none() {
            match StreamingSilenceDetector::new(info.sample_rate, info.channels, config) {
                Ok(created) => {
                    sample_rate = info.sample_rate;
                    channels = info.channels;
                    if let ChannelPolicy::Channel(index) = policy {
                        if index as usize >= info.channels as usize {
                            return Err("Selected silence channel is out of range".into());
                        }
                    }
                    detector = Some(created);
                }
                Err(error) => {
                    push_diagnostic(&mut diagnostics, &error);
                    continue;
                }
            }
        }

        let ch = info.channels as usize;
        let mut interleaved = vec![0.0f32; READ_FRAME_CHUNK * ch];
        let mut frame_index = 0u64;
        let mut feed_failed = None;
        loop {
            let frames = match reader.read_frames(&mut interleaved, READ_FRAME_CHUNK) {
                Ok(0) => break,
                Ok(frames) => frames,
                Err(error) => {
                    push_diagnostic(
                        &mut diagnostics,
                        &format!(
                            "Unreadable audio treated as a gap, not silence: {} ({error})",
                            segment.relative_path
                        ),
                    );
                    reset_detector(&mut detector, &mut raw_regions);
                    break;
                }
            };
            let local_us = info.frame_us(frame_index);
            let source_us = segment.start_us.saturating_add(local_us);
            if source_us >= segment.end_us {
                break;
            }
            let mut use_frames = frames;
            while use_frames > 0 {
                let end_us = segment
                    .start_us
                    .saturating_add(info.frame_us(frame_index + use_frames as u64));
                if end_us <= segment.end_us {
                    break;
                }
                use_frames -= 1;
            }
            if use_frames == 0 {
                break;
            }
            let samples = &interleaved[..use_frames * ch];
            if let Some(active) = detector.as_mut() {
                if let Err(error) = active.feed(samples, source_us) {
                    feed_failed = Some(error);
                    break;
                }
            }
            frame_index += use_frames as u64;
        }
        if let Some(error) = feed_failed {
            return Err(error);
        }
    }

    if let Some(mut active) = detector.take() {
        raw_regions.extend(active.take_raw_regions());
    }

    let source_ranges = finalize_regions(raw_regions, config);
    let mapper = TimelineMapper::try_new(
        ctx.retained
            .iter()
            .enumerate()
            .map(|(i, interval)| {
                SourceInterval::new(format!("ret-{i}"), interval.start_us, interval.end_us)
            })
            .collect(),
    )?;

    let mut suggestions = Vec::new();
    let mut bounded = false;
    for (source_start, source_end) in source_ranges {
        for (edited_start, edited_end) in mapper.source_range_to_edited(source_start, source_end) {
            if edited_end <= edited_start {
                continue;
            }
            if suggestions.len() >= MAX_CUTS_PER_REVISION {
                bounded = true;
                break;
            }
            suggestions.push(SilenceCutInterval {
                id: format!("silence-{}", suggestions.len() + 1),
                start_us: edited_start,
                end_us: edited_end,
                duration_ms: edited_end.saturating_sub(edited_start) / 1_000,
                selected: true,
                source_start_us: source_start,
                source_end_us: source_end,
            });
        }
        if bounded {
            break;
        }
    }
    if bounded {
        push_diagnostic(
            &mut diagnostics,
            "Silence suggestion count was bounded to one revision of ripple cuts",
        );
    }

    Ok(SilenceDetectionResult {
        track_id: ctx.track_id.clone(),
        sample_rate,
        channels,
        channel_policy: channel_policy_name(policy),
        suggestions,
        diagnostics,
    })
}

fn reset_detector(
    detector: &mut Option<StreamingSilenceDetector>,
    raw_regions: &mut Vec<(u64, u64)>,
) {
    if let Some(active) = detector.as_mut() {
        active.notify_discontinuity();
        raw_regions.extend(active.take_raw_regions());
    }
}

fn push_diagnostic(diagnostics: &mut Vec<String>, message: &str) {
    if diagnostics.len() < MAX_DIAGNOSTICS {
        diagnostics.push(message.to_string());
    }
}
