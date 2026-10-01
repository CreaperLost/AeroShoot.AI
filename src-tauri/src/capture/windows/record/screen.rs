//! Screen and window recording: Windows Graphics Capture frames are scaled
//! and converted on the GPU and encoded as H.264 MP4 segments.
//!
//! Capture only delivers a frame when the picture changes, so output is paced
//! at a constant frame rate that repeats the latest picture. Every tick is a
//! recorded sample, which also keeps static screens from looking stalled.
use super::clock::{now_hns, RecordingClock, HNS_PER_SECOND};
use super::encoder::{SharedDeviceManager, VideoFormat};
use super::frames;
use super::gpu::{self, Converter, Gpu};
use super::screen_capture::{ScreenCapture, ScreenTarget};
use super::track::{next_command, TrackCommand, TrackContext};
use super::video_track::VideoSegments;
use crate::capture::windows::ComApartment;
use ::windows::Win32::Graphics::Direct3D11::{D3D11_BOX, D3D11_TEXTURE2D_DESC};
use ::windows::Win32::Media::MediaFoundation::{
    MFShutdown, MFStartup, MFSTARTUP_NOSOCKET, MF_VERSION,
};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The live preview only monitors a recording; 15 fps is enough.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(66);

pub struct ScreenOptions {
    pub format: VideoFormat,
    pub show_cursor: bool,
}

pub fn spawn(
    context: TrackContext,
    target: ScreenTarget,
    options: ScreenOptions,
    clock: RecordingClock,
    commands: Receiver<TrackCommand>,
) -> Result<JoinHandle<()>, String> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("aeroshoot-screen".into())
        .spawn(move || {
            let com = match ComApartment::enter() {
                Ok(com) => com,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            if let Err(error) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) } {
                let _ = ready_tx.send(Err(format!("Media Foundation is unavailable: {error}")));
                return;
            }
            match ScreenRecorder::open(context, target, options, clock) {
                Ok(recorder) => {
                    let _ = ready_tx.send(Ok(()));
                    recorder.run(&commands);
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            }
            let _ = unsafe { MFShutdown() };
            drop(com);
        })
        .map_err(|e| format!("Could not start the screen thread: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(thread),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            Err("The screen thread exited during startup".into())
        }
    }
}

struct ScreenRecorder {
    context: TrackContext,
    clock: RecordingClock,
    format: VideoFormat,
    gpu: Arc<Gpu>,
    capture: ScreenCapture,
    converter: Option<Converter>,
    video: VideoSegments,
    tick_hns: u64,
    last_preview: Option<Instant>,
}

impl ScreenRecorder {
    fn open(
        context: TrackContext,
        target: ScreenTarget,
        options: ScreenOptions,
        clock: RecordingClock,
    ) -> Result<Self, String> {
        let gpu = gpu::shared()?;
        let capture = ScreenCapture::open(&gpu, target, options.show_cursor)?;
        let video = VideoSegments::new(
            &context,
            options.format,
            Some(SharedDeviceManager(gpu.manager.clone())),
        )?;
        Ok(Self {
            tick_hns: HNS_PER_SECOND / u64::from(options.format.fps.max(1)),
            context,
            clock,
            format: options.format,
            gpu,
            capture,
            converter: None,
            video,
            last_preview: None,
        })
    }

    fn run(mut self, commands: &Receiver<TrackCommand>) {
        let start = self.clock.start_hns();
        let mut next_tick: u64 = 0;
        loop {
            match next_command(commands) {
                Some(TrackCommand::Stop) => {
                    if let Err(error) = self.video.close(None) {
                        self.context.fail(error);
                    }
                    self.video.finish();
                    return;
                }
                Some(TrackCommand::Finalize(reply)) => {
                    let _ = self.video.close(Some(reply));
                }
                None => {}
            }
            if self.capture.is_closed() {
                return self.fail_and_wait(
                    "The captured screen or window is no longer available".into(),
                    commands,
                );
            }
            if let Err(error) = self.drain_frames() {
                return self.fail_and_wait(error, commands);
            }

            let now = now_hns();
            if now < start {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            let due = (now - start) / self.tick_hns;
            if due < next_tick {
                let wait = start + next_tick * self.tick_hns - now;
                std::thread::sleep(Duration::from_nanos(wait * 100));
                continue;
            }
            // Behind schedule: skip missed ticks rather than bursting. Only
            // ticks inside an open segment are lost frames.
            if due > next_tick {
                if self.video.is_open() {
                    self.context
                        .stats
                        .dropped
                        .fetch_add(due - next_tick, Ordering::Relaxed);
                }
                next_tick = due;
            }
            if let Err(error) = self.encode_tick(next_tick) {
                return self.fail_and_wait(error, commands);
            }
            next_tick += 1;
        }
    }

    /// Take every delivered frame, keeping only the newest picture.
    fn drain_frames(&mut self) -> Result<(), String> {
        let gpu = self.gpu.clone();
        let output = (self.format.width, self.format.height);
        let fps = self.format.fps;
        let converter = &mut self.converter;
        let frames = self.capture.drain(|texture, content| {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe { texture.GetDesc(&mut desc) };
            let size = (desc.Width, desc.Height);
            if converter.as_ref().map(|c| c.source_size()) != Some(size) {
                *converter = Some(Converter::new(&gpu, size, output, fps)?);
            }
            let converter = converter.as_mut().expect("converter created above");
            let region = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: size.0,
                bottom: size.1,
                back: 1,
            };
            unsafe {
                gpu.context.CopySubresourceRegion(
                    &converter.source,
                    0,
                    0,
                    0,
                    0,
                    texture,
                    0,
                    Some(&region),
                );
            }
            converter.set_content_size(content.0, content.1);
            Ok(())
        })?;
        if frames > 0 && !self.video.is_open() {
            // Warm-up and pause: count delivery so readiness sees the source.
            self.context.stats.record_sample();
        }
        if frames > 0 {
            self.offer_preview();
        }
        Ok(())
    }

    fn offer_preview(&mut self) {
        if self
            .last_preview
            .is_some_and(|last| last.elapsed() < PREVIEW_INTERVAL)
        {
            return;
        }
        if let Some(converter) = &self.converter {
            // Best effort: the preview never affects the recording.
            let _ = frames::offer_screen(&self.gpu, &converter.source, converter.content_size());
            self.last_preview = Some(Instant::now());
        }
    }

    fn encode_tick(&mut self, tick: u64) -> Result<(), String> {
        if self.context.is_paused() {
            return Ok(());
        }
        let Some(converter) = self.converter.as_mut() else {
            // No picture yet.
            return Ok(());
        };
        let texture = converter
            .convert()
            .map_err(|e| format!("Screen frame conversion failed: {e}"))?;
        let time_hns = tick * self.tick_hns;
        self.video
            .write_texture(&texture, time_hns / 10, time_hns, self.tick_hns)?;
        self.context.stats.record_sample();
        Ok(())
    }

    fn fail_and_wait(mut self, error: String, commands: &Receiver<TrackCommand>) {
        self.context.fail(error.clone());
        let _ = self.video.close(None);
        self.video.finish();
        while let Ok(command) = commands.recv() {
            match command {
                TrackCommand::Stop => return,
                TrackCommand::Finalize(reply) => {
                    let _ = reply.send(Err(error.clone()));
                }
            }
        }
    }
}
