//! Native Windows recording session. Each enabled track runs on its own
//! thread and writes independently valid segments of at most 60 seconds,
//! published through `capture::segments` exactly like the macOS recorder.
pub(super) mod audio;
pub(super) mod camera;
mod clock;
mod cursor;
mod encoder;
pub(super) mod frames;
pub(super) mod gpu;
mod mouse;
mod screen;
pub(super) mod screen_capture;
mod track;
mod video_track;
mod wav;

use crate::capture::backend::NativeCaptureSession;
use crate::capture::windows::exclusion::{exclude_own_windows, ExclusionGuard};
use crate::capture::{NativeCaptureStats, NativeRecordingConfig};
use crate::project::manifest::TrackType;
use audio::AudioSource;
use clock::RecordingClock;
use encoder::VideoFormat;
use mouse::{MouseConfig, MouseSource, MouseTracker};
use screen::ScreenOptions;
use screen_capture::ScreenTarget;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use track::{TrackCommand, TrackContext, TrackStats, CAPTURE_ERROR};

const FINALIZE_TIMEOUT: Duration = Duration::from_secs(15);

struct TrackHandle {
    id: &'static str,
    stats: Arc<TrackStats>,
    commands: Sender<TrackCommand>,
    thread: Option<JoinHandle<()>>,
}

impl TrackHandle {
    fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub struct WinCaptureSession {
    clock: RecordingClock,
    paused: Arc<AtomicBool>,
    tracks: Vec<TrackHandle>,
    mouse: Option<MouseTracker>,
    /// Keeps AeroShoot out of a display recording, as on macOS.
    _exclusion: Option<ExclusionGuard>,
}

impl WinCaptureSession {
    fn track(&self, id: &str) -> Option<&TrackHandle> {
        self.tracks.iter().find(|track| track.id == id)
    }

    fn add_track(
        &mut self,
        config: &NativeRecordingConfig<'_>,
        id: &'static str,
        track_type: TrackType,
        spawn: impl FnOnce(TrackContext, mpsc::Receiver<TrackCommand>) -> Result<JoinHandle<()>, String>,
    ) -> Result<(), String> {
        let stats = TrackStats::new();
        let (commands, receiver) = mpsc::channel();
        let context = TrackContext {
            track_id: id,
            track_type,
            project_path: config.project_path.to_path_buf(),
            stats: stats.clone(),
            paused: self.paused.clone(),
        };
        let thread = spawn(context, receiver)?;
        self.tracks.push(TrackHandle {
            id,
            stats,
            commands,
            thread: Some(thread),
        });
        Ok(())
    }

    /// Signal every track before waiting on any, so all tracks end at the
    /// same moment even though finishing a video segment takes a while.
    fn stop_tracks(&mut self) {
        for track in &self.tracks {
            let _ = track.commands.send(TrackCommand::Stop);
        }
        for track in &mut self.tracks {
            track.join();
        }
    }

    fn first_error(&self) -> Option<String> {
        self.tracks.iter().find_map(|track| track.stats.error())
    }
}

impl NativeCaptureSession for WinCaptureSession {
    fn start(config: NativeRecordingConfig<'_>) -> Result<Self, String> {
        // The recording takes over the devices and feeds the live preview.
        crate::capture::windows::preview::stop();
        frames::set_recording(true);
        frames::set_screen_shown(config.capture_screen);
        if config.camera_id.is_none() {
            frames::clear_camera();
        }
        let clock = RecordingClock::starting_after(config.start_delay_ms);
        let mut session = Self {
            clock,
            paused: Arc::new(AtomicBool::new(false)),
            tracks: Vec::new(),
            mouse: None,
            _exclusion: None,
        };
        let started = (|| {
            // Pointer telemetry starts first: if the hook cannot be installed,
            // the cursor must stay in the video.
            let mut show_cursor = !config.hide_cursor;
            if config.capture_screen && config.capture_mouse {
                if let Some(source) = MouseSource::from_source_id(config.source_id) {
                    let mouse_config = MouseConfig {
                        source_id: config.source_id.to_string(),
                        source,
                        output_width: config.width,
                        output_height: config.height,
                        cursor_mode: if config.hide_cursor {
                            crate::telemetry::native::CURSOR_MODE_REPLACE
                        } else {
                            crate::telemetry::native::CURSOR_MODE_BAKED
                        },
                    };
                    let (tracker, hooked) = MouseTracker::start(
                        config.project_path,
                        mouse_config,
                        clock,
                        session.paused.clone(),
                    )?;
                    show_cursor |= !hooked;
                    session.mouse = Some(tracker);
                }
            }
            if config.capture_screen {
                let target = ScreenTarget::from_source_id(config.source_id)?;
                if matches!(target, ScreenTarget::Display(_)) {
                    session._exclusion = Some(exclude_own_windows());
                }
                // NV12 needs even dimensions.
                let (width, height) = (config.width & !1, config.height & !1);
                let options = ScreenOptions {
                    format: VideoFormat {
                        width,
                        height,
                        fps: config.fps.max(1),
                        bitrate_bps: config.video_bitrate_bps.unwrap_or_else(|| {
                            VideoFormat::automatic_bitrate(width, height, config.fps)
                        }),
                    },
                    show_cursor,
                };
                session.add_track(&config, "screen", TrackType::Screen, |context, commands| {
                    screen::spawn(context, target, options, clock, commands)
                })?;
            }
            if let Some(camera_id) = config.camera_id {
                // The requested camera format, or 1280×720 at the screen rate.
                let width = config.camera_width.unwrap_or(camera::CAMERA_WIDTH) & !1;
                let height = config.camera_height.unwrap_or(camera::CAMERA_HEIGHT) & !1;
                let fps = config.camera_fps.unwrap_or(config.fps).max(1);
                let format = VideoFormat {
                    width,
                    height,
                    fps,
                    bitrate_bps: config
                        .camera_bitrate_bps
                        .unwrap_or_else(|| VideoFormat::automatic_bitrate(width, height, fps)),
                };
                let symbolic_link = camera_id.to_string();
                session.add_track(&config, "webcam", TrackType::Webcam, |context, commands| {
                    camera::spawn(context, symbolic_link, format, clock, commands)
                })?;
            }
            if let Some(mic_id) = config.mic_id {
                let source = AudioSource::Microphone {
                    endpoint_id: mic_id.to_string(),
                    gain_db: config.mic_gain_db.unwrap_or(0.0),
                };
                session.add_track(&config, "mic", TrackType::MicAudio, |context, commands| {
                    audio::spawn(context, source, clock, commands)
                })?;
            }
            if config.capture_system_audio {
                session.add_track(
                    &config,
                    "system",
                    TrackType::SystemAudio,
                    |context, commands| {
                        audio::spawn(context, AudioSource::SystemLoopback, clock, commands)
                    },
                )?;
            }
            Ok(())
        })();
        if let Err(error) = started {
            session.stop_tracks();
            frames::set_recording(false);
            return Err(error);
        }
        // Like the macOS bridge, return once the countdown is over: sources
        // warm up meanwhile, and `recording_started_ago_us` is then known so
        // the session clock can move to recording time zero.
        let remaining_hns = clock.start_hns().saturating_sub(clock::now_hns());
        std::thread::sleep(Duration::from_nanos(remaining_hns * 100));
        Ok(session)
    }

    fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    fn pause_and_finalize(&self) -> Result<(), (i32, String)> {
        self.paused.store(true, Ordering::SeqCst);
        let mut replies = Vec::new();
        for track in &self.tracks {
            let (reply, receiver) = mpsc::channel();
            if track.commands.send(TrackCommand::Finalize(reply)).is_err() {
                return Err((
                    CAPTURE_ERROR,
                    format!("{} recorder is not running", track.id),
                ));
            }
            replies.push((track.id, receiver));
        }
        for (id, receiver) in replies {
            match receiver.recv_timeout(FINALIZE_TIMEOUT) {
                Ok(Ok(())) => {}
                Ok(Err(message)) => return Err((CAPTURE_ERROR, message)),
                Err(_) => return Err((CAPTURE_ERROR, format!("{id} did not finish its segment"))),
            }
        }
        Ok(())
    }

    fn stats(&self) -> NativeCaptureStats {
        let samples = |id| {
            self.track(id)
                .map_or(0, |t| t.stats.samples.load(Ordering::Relaxed))
        };
        let peak = |id| self.track(id).and_then(|t| t.stats.peak_db());
        let age = |id| self.track(id).and_then(|t| t.stats.last_sample_age_ms());
        let sum = |field: fn(&TrackStats) -> u64| {
            self.tracks.iter().map(|t| field(&t.stats)).sum::<u64>()
        };
        NativeCaptureStats {
            recording_started_ago_us: self.clock.started_ago_us(),
            dropped_frames: sum(|s| s.dropped.load(Ordering::Relaxed)),
            audio_buffer_underflows: 0,
            timestamp_records_dropped: 0,
            gaps_total: sum(|s| s.gaps.load(Ordering::Relaxed)),
            last_error: self.first_error(),
            screen_samples: samples("screen"),
            camera_samples: samples("webcam"),
            system_audio_samples: samples("system"),
            mic_samples: samples("mic"),
            system_audio_peak_db: peak("system"),
            mic_peak_db: peak("mic"),
            screen_last_sample_age_ms: age("screen"),
            camera_last_sample_age_ms: age("webcam"),
            system_audio_last_sample_age_ms: age("system"),
            mic_last_sample_age_ms: age("mic"),
        }
    }

    fn stop_with_result(mut self) -> Result<(), (i32, String)> {
        self.stop_tracks();
        let telemetry = self.mouse.take().map_or(Ok(()), MouseTracker::stop);
        match self.first_error() {
            Some(message) => Err((CAPTURE_ERROR, message)),
            // Same code as the macOS bridge for a telemetry persistence failure.
            None => telemetry.map_err(|message| (-700, message)),
        }
    }
}

impl Drop for WinCaptureSession {
    fn drop(&mut self) {
        self.stop_tracks();
        frames::set_recording(false);
    }
}
