//! Playback owner: generation-aware seek plans, bounded open files, and a clock.
//! F1 adds a discardable native preview overlay; F2 still owns decode/compositor.
pub mod audio;
#[cfg(feature = "tauri-app")]
pub mod engine;
mod native;
pub mod preview;

use crate::project::manifest::TrackType;
use crate::project::reader::{RetainedInterval, SegmentSummary, TrackSummary};
use crate::project::revision::EditDocument;
use crate::timeline::{SourceInterval, TimelineMapper};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::File;
use std::path::PathBuf;
use std::time::Instant;

pub use preview::{PreviewHitMode, PreviewOwner, PreviewStatus, PreviewViewport};
pub const MAX_OPEN_FILES: usize = 8;
pub const MAX_PLAN_TRACKS: usize = 16;

fn next_generation() -> u64 {
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    Closed,
    Ready,
    Playing,
    Paused,
    Seeking,
    Ended,
    Error,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClockKind {
    Audio,
    Monotonic,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackDecodePlan {
    pub track_id: String,
    pub generation: u64,
    pub edited_us: u64,
    pub source_us: Option<u64>,
    pub gap: bool,
    pub ended: bool,
    pub relative_path: Option<String>,
    pub keyframe_source_us: Option<u64>,
    pub decode_to_source_us: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackStatus {
    pub project_handle: String,
    pub state: PlaybackState,
    pub generation: u64,
    pub position_us: u64,
    pub duration_us: u64,
    pub clock_kind: ClockKind,
    pub preview_available: bool,
    pub open_files: usize,
    pub plans: Vec<TrackDecodePlan>,
    pub error: Option<String>,
    pub diagnostics: Vec<String>,
}

struct OpenedMedia {
    path: PathBuf,
    _file: File,
}

pub struct PlaybackOwner {
    pub(crate) native_enabled: bool,
    pub(crate) audio: Option<audio::AudioOutput>,
    pub(crate) audio_start_frame: u64,
    #[cfg_attr(not(feature = "tauri-app"), allow(dead_code))]
    pub(crate) audio_queued_frame: u64,
    preview_available: bool,
    project_handle: String,
    root: PathBuf,
    state: PlaybackState,
    generation: u64,
    position_us: u64,
    duration_us: u64,
    retained: Vec<RetainedInterval>,
    tracks: Vec<(TrackSummary, Vec<SegmentSummary>)>,
    open_files: VecDeque<OpenedMedia>,
    last_accepted_generation: u64,
    play_anchor: Option<(Instant, u64)>,
    error: Option<String>,
    diagnostics: Vec<String>,
}

impl PlaybackOwner {
    pub fn closed() -> Self {
        Self {
            native_enabled: false,
            audio: None,
            audio_start_frame: 0,
            audio_queued_frame: 0,
            preview_available: false,
            project_handle: String::new(),
            root: PathBuf::new(),
            state: PlaybackState::Closed,
            generation: 0,
            position_us: 0,
            duration_us: 0,
            retained: Vec::new(),
            tracks: Vec::new(),
            open_files: VecDeque::new(),
            last_accepted_generation: 0,
            play_anchor: None,
            error: None,
            diagnostics: Vec::new(),
        }
    }

    pub fn open(
        project_handle: String,
        root: PathBuf,
        document: &EditDocument,
        tracks: Vec<(TrackSummary, Vec<SegmentSummary>)>,
    ) -> Result<Self, String> {
        let mapper = document.mapper()?;
        let duration_us = mapper.total_edited_duration_us();
        let mut owner = Self {
            native_enabled: false,
            audio: None,
            audio_start_frame: 0,
            audio_queued_frame: 0,
            preview_available: false,
            project_handle,
            root,
            state: if duration_us == 0 {
                PlaybackState::Ended
            } else {
                PlaybackState::Ready
            },
            generation: next_generation(),
            position_us: 0,
            duration_us,
            retained: document.retained_intervals.clone(),
            tracks,
            open_files: VecDeque::new(),
            last_accepted_generation: 0,
            play_anchor: None,
            error: None,
            diagnostics: Vec::new(),
        };
        owner.touch_plans(0)?;
        Ok(owner)
    }

    pub fn apply_document(&mut self, document: &EditDocument) -> Result<(), String> {
        self.advance();
        self.bump_generation();
        self.retained = document.retained_intervals.clone();
        self.duration_us = document.edited_duration_us()?;
        if self.position_us > self.duration_us {
            self.position_us = self.duration_us;
        }
        self.play_anchor = None;
        self.audio = None;
        self.state = PlaybackState::Paused;
        if self.duration_us == 0 {
            self.state = PlaybackState::Ended;
        } else if self.position_us >= self.duration_us {
            self.state = PlaybackState::Ended;
        } else if self.state == PlaybackState::Ended || self.state == PlaybackState::Closed {
            self.state = PlaybackState::Paused;
        }
        self.touch_plans(self.position_us)?;
        Ok(())
    }

    pub fn close(&mut self) {
        *self = Self::closed();
    }

    pub fn play(&mut self) -> Result<PlaybackStatus, String> {
        self.ensure_open()?;
        self.advance();
        if self.state == PlaybackState::Ended {
            self.seek(0)?;
        }
        self.state = PlaybackState::Playing;
        self.play_anchor = if self.needs_audio() {
            None
        } else {
            Some((Instant::now(), self.position_us))
        };
        self.status()
    }

    pub fn pause(&mut self) -> Result<PlaybackStatus, String> {
        self.ensure_open()?;
        self.advance();
        if self.state != PlaybackState::Ended {
            self.state = PlaybackState::Paused;
        }
        self.play_anchor = None;
        self.audio = None;
        self.status()
    }

    pub fn seek(&mut self, edited_us: u64) -> Result<PlaybackStatus, String> {
        self.ensure_open()?;
        let was_playing = self.state == PlaybackState::Playing;
        self.bump_generation();
        self.audio = None;
        self.state = PlaybackState::Seeking;
        self.position_us = edited_us.min(self.duration_us);
        self.play_anchor =
            if was_playing && !self.needs_audio() && self.position_us < self.duration_us {
                Some((Instant::now(), self.position_us))
            } else {
                None
            };
        self.touch_plans(self.position_us)?;
        if self.position_us >= self.duration_us {
            self.state = PlaybackState::Ended;
        } else if was_playing {
            self.state = PlaybackState::Playing;
        } else {
            self.state = PlaybackState::Paused;
        }
        self.status()
    }

    pub fn status(&mut self) -> Result<PlaybackStatus, String> {
        if self.state != PlaybackState::Closed {
            self.advance();
        }
        Ok(self.snapshot())
    }

    /// Decoder/output threads must drop results whose generation no longer matches.
    pub fn accept_decode_result(&mut self, generation: u64, source_us: u64) -> bool {
        if self.state == PlaybackState::Closed || generation != self.generation {
            return false;
        }
        if self.sample_source_us(self.position_us) != Some(source_us) {
            return false;
        }
        self.last_accepted_generation = generation;
        true
    }

    pub fn open_file_count(&self) -> usize {
        self.open_files.len()
    }

    fn ensure_open(&self) -> Result<(), String> {
        if self.state == PlaybackState::Closed {
            return Err("Playback is closed".into());
        }
        if self.state == PlaybackState::Error {
            return Err(self
                .error
                .clone()
                .unwrap_or_else(|| "Playback error".into()));
        }
        Ok(())
    }

    fn bump_generation(&mut self) {
        self.preview_available = false;
        self.generation = next_generation();
    }

    fn mapper(&self) -> TimelineMapper {
        TimelineMapper::try_new(
            self.retained
                .iter()
                .enumerate()
                .map(|(i, interval)| {
                    SourceInterval::new(format!("ret-{i}"), interval.start_us, interval.end_us)
                })
                .collect(),
        )
        .unwrap_or_else(|_| TimelineMapper::new(Vec::new()))
    }

    fn sample_source_us(&self, edited_us: u64) -> Option<u64> {
        self.mapper().edited_to_source_us(edited_us)
    }

    fn advance(&mut self) {
        if self.state != PlaybackState::Playing {
            return;
        }
        if let Some(audio) = &self.audio {
            match audio.position_frames() {
                Ok(frames) => {
                    let total = self.audio_start_frame + frames;
                    let end = (self.duration_us as u128 * crate::media::audio::SAMPLE_RATE as u128
                        / 1_000_000) as u64;
                    self.position_us = if total >= end {
                        self.duration_us
                    } else {
                        (total as u128 * 1_000_000 / crate::media::audio::SAMPLE_RATE as u128)
                            as u64
                    };
                }
                Err(error) => {
                    self.fail(self.generation, error);
                    return;
                }
            }
        } else {
            let Some((anchor, start_us)) = self.play_anchor else {
                return;
            };
            let elapsed = Instant::now().saturating_duration_since(anchor).as_micros() as u64;
            self.position_us = start_us.saturating_add(elapsed).min(self.duration_us);
        }
        if self.position_us >= self.duration_us {
            self.position_us = self.duration_us;
            self.state = PlaybackState::Ended;
            self.play_anchor = None;
        }
        let _ = self.touch_plans(self.position_us);
    }

    fn clock_kind(&self, _source_us: Option<u64>) -> ClockKind {
        if self.audio.is_some() {
            ClockKind::Audio
        } else {
            ClockKind::Monotonic
        }
    }
    fn needs_audio(&self) -> bool {
        self.native_enabled
            && self.tracks.iter().any(|(track, segments)| {
                matches!(
                    track.descriptor.track_type,
                    TrackType::MicAudio | TrackType::SystemAudio
                ) && segments.iter().any(|s| s.available)
            })
    }
    pub fn fail(&mut self, generation: u64, error: String) {
        if self.generation != generation {
            return;
        }
        self.audio = None;
        self.play_anchor = None;
        self.state = PlaybackState::Error;
        self.error = Some(error);
        self.preview_available = false;
    }
    pub fn mark_presented(&mut self, generation: u64) -> bool {
        if self.generation != generation
            || self.state == PlaybackState::Closed
            || self.state == PlaybackState::Error
        {
            return false;
        }
        self.preview_available = true;
        true
    }

    fn touch_plans(&mut self, edited_us: u64) -> Result<(), String> {
        let source_us = self.sample_source_us(edited_us);
        let ended = edited_us >= self.duration_us;
        let mut plans = Vec::new();
        for (track, segments) in &self.tracks {
            if plans.len() >= MAX_PLAN_TRACKS {
                break;
            }
            plans.push(plan_for_track(
                &track.descriptor.id,
                self.generation,
                edited_us,
                source_us,
                ended,
                segments,
            ));
        }
        for plan in &plans {
            if let Some(relative) = &plan.relative_path {
                self.touch_file(relative)?;
            }
        }
        self.diagnostics = plans
            .iter()
            .filter(|plan| plan.gap && !plan.ended)
            .map(|plan| format!("Gap on track {}", plan.track_id))
            .take(32)
            .collect();
        let _ = plans;
        Ok(())
    }

    fn touch_file(&mut self, relative: &str) -> Result<(), String> {
        let path = crate::project::reader::safe_path(&self.root, relative)?;
        if let Some(index) = self.open_files.iter().position(|entry| entry.path == path) {
            if let Some(entry) = self.open_files.remove(index) {
                self.open_files.push_back(entry);
            }
            return Ok(());
        }
        if !path.is_file() {
            return Ok(());
        }
        let file = crate::project::reader::open_regular(&path)?;
        if self.open_files.len() >= MAX_OPEN_FILES {
            self.open_files.pop_front();
        }
        self.open_files.push_back(OpenedMedia { path, _file: file });
        Ok(())
    }

    fn snapshot(&self) -> PlaybackStatus {
        let source_us = self.sample_source_us(self.position_us);
        let ended = self.position_us >= self.duration_us && self.duration_us > 0
            || self.state == PlaybackState::Ended;
        let plans = self
            .tracks
            .iter()
            .take(MAX_PLAN_TRACKS)
            .map(|(track, segments)| {
                plan_for_track(
                    &track.descriptor.id,
                    self.generation,
                    self.position_us,
                    source_us,
                    ended && self.position_us >= self.duration_us,
                    segments,
                )
            })
            .collect();
        PlaybackStatus {
            project_handle: self.project_handle.clone(),
            state: self.state,
            generation: self.generation,
            position_us: self.position_us,
            duration_us: self.duration_us,
            clock_kind: self.clock_kind(source_us),
            preview_available: self.preview_available,
            open_files: self.open_files.len(),
            plans,
            error: self.error.clone(),
            diagnostics: self.diagnostics.clone(),
        }
    }
}

pub(crate) fn plan_for_track(
    track_id: &str,
    generation: u64,
    edited_us: u64,
    source_us: Option<u64>,
    ended: bool,
    segments: &[SegmentSummary],
) -> TrackDecodePlan {
    if ended || source_us.is_none() {
        return TrackDecodePlan {
            track_id: track_id.into(),
            generation,
            edited_us,
            source_us: None,
            gap: !ended,
            ended,
            relative_path: None,
            keyframe_source_us: None,
            decode_to_source_us: None,
        };
    }
    let source_us = source_us.unwrap();
    let containing = segments
        .iter()
        .find(|segment| segment.start_us <= source_us && source_us < segment.end_us);
    let Some(segment) = containing else {
        return TrackDecodePlan {
            track_id: track_id.into(),
            generation,
            edited_us,
            source_us: Some(source_us),
            gap: true,
            ended: false,
            relative_path: None,
            keyframe_source_us: None,
            decode_to_source_us: None,
        };
    };
    if !segment.available {
        return TrackDecodePlan {
            track_id: track_id.into(),
            generation,
            edited_us,
            source_us: Some(source_us),
            gap: true,
            ended: false,
            relative_path: Some(segment.relative_path.clone()),
            keyframe_source_us: None,
            decode_to_source_us: None,
        };
    }
    let keyframe_source_us = preceding_keyframe(segments, source_us);
    TrackDecodePlan {
        track_id: track_id.into(),
        generation,
        edited_us,
        source_us: Some(source_us),
        gap: false,
        ended: false,
        relative_path: Some(segment.relative_path.clone()),
        keyframe_source_us,
        decode_to_source_us: Some(source_us),
    }
}

fn preceding_keyframe(segments: &[SegmentSummary], source_us: u64) -> Option<u64> {
    let mut keyframe = None;
    for segment in segments {
        if segment.start_us > source_us {
            break;
        }
        if segment.is_keyframe_start.unwrap_or(true) {
            keyframe = Some(segment.start_us);
        }
        if segment.start_us <= source_us && source_us < segment.end_us && keyframe.is_none() {
            keyframe = Some(segment.start_us);
        }
    }
    keyframe
}

pub fn tracks_from_reader(
    reader: &crate::project::ProjectReader,
) -> Vec<(TrackSummary, Vec<SegmentSummary>)> {
    reader
        .summary
        .tracks
        .iter()
        .map(|track| {
            let segments = reader
                .segments_for(&track.descriptor.id)
                .unwrap_or(&[])
                .to_vec();
            (track.clone(), segments)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::generate_pcm16_wav;
    use crate::project::manifest::{TrackDescriptor, TrackType};
    use crate::project::reader::ProjectReader;
    use crate::project::{JournalRecord, ProjectBundle};
    use std::fs;

    #[test]
    fn exclusive_end_is_not_a_decode_request() {
        let plan = plan_for_track(
            "screen",
            1,
            2_000_000,
            None,
            true,
            &[SegmentSummary {
                track_id: "screen".into(),
                relative_path: "media/screen/000001.mp4".into(),
                start_us: 0,
                end_us: 2_000_000,
                size_bytes: 1,
                media_timescale: 30,
                media_start_value: 0,
                host_anchor_us: 0,
                is_keyframe_start: Some(true),
                available: true,
            }],
        );
        assert!(plan.ended);
        assert!(plan.decode_to_source_us.is_none());
        assert!(plan.source_us.is_none());
    }

    #[test]
    fn stale_generation_is_rejected_and_missing_mic_uses_monotonic_clock() {
        let dir = tempfile::tempdir().unwrap();
        let mut bundle = ProjectBundle::create_new(dir.path(), "pb", "pb").unwrap();
        let wav = generate_pcm16_wav(48_000, 1, &vec![0i16; 4_800]);
        fs::write(bundle.root_path().join("media/screen/000001.wav"), &wav).unwrap();
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
                relative_path: "media/screen/000001.wav".into(),
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
        let root = bundle.root_path().to_path_buf();
        drop(bundle);
        let reader = ProjectReader::open(&root).unwrap();
        let document =
            EditDocument::from_retained(reader.summary.retained_intervals.clone()).unwrap();
        let mut owner = PlaybackOwner::open(
            reader.summary.project_handle.clone(),
            root,
            &document,
            tracks_from_reader(&reader),
        )
        .unwrap();
        let first = owner.seek(10_000).unwrap();
        let stale = first.generation;
        let second = owner.seek(20_000).unwrap();
        assert_ne!(stale, second.generation);
        assert!(!owner.accept_decode_result(stale, 10_000));
        assert!(owner.accept_decode_result(second.generation, 20_000));
        assert_eq!(second.clock_kind, ClockKind::Monotonic);
        owner.play().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let playing = owner.status().unwrap();
        assert!(playing.position_us > 0 || playing.state == PlaybackState::Ended);
        assert_eq!(playing.clock_kind, ClockKind::Monotonic);
    }
}
