//! Webcam recording: Media Foundation Source Reader frames, converted to
//! 1280×720 NV12 by the reader, encoded as H.264 MP4 segments.
//!
//! Camera sample times are performance-counter based on most drivers; when a
//! driver uses its own origin, the first frame's arrival fixes the offset and
//! the driver's timestamps keep their spacing from then on.
use super::clock::{now_hns, RecordingClock, HNS_PER_SECOND};
use super::encoder::VideoFormat;
use super::frames::{self, CameraFrame};
use super::track::{next_command, TrackCommand, TrackContext};
use super::video_track::VideoSegments;
use crate::capture::windows::ComApartment;
use ::windows::core::HSTRING;
use ::windows::Win32::Media::MediaFoundation::*;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The live preview only monitors a recording; 15 fps is enough.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(66);

pub const CAMERA_WIDTH: u32 = 1280;
pub const CAMERA_HEIGHT: u32 = 720;

/// Driver timestamps further than this from the performance counter are
/// treated as having their own origin.
const CLOCK_MATCH_HNS: u64 = HNS_PER_SECOND;

pub fn spawn(
    context: TrackContext,
    symbolic_link: String,
    format: VideoFormat,
    clock: RecordingClock,
    commands: Receiver<TrackCommand>,
) -> Result<JoinHandle<()>, String> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("aeroshoot-webcam".into())
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
            match CameraRecorder::open(context, &symbolic_link, format, clock) {
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
        .map_err(|e| format!("Could not start the camera thread: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(thread),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            Err("The camera thread exited during startup".into())
        }
    }
}

/// Open a camera through the Source Reader, converting to NV12 at the given
/// size. Shared with the live preview.
pub fn open_reader(
    symbolic_link: &str,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(IMFMediaSource, IMFSourceReader), String> {
    let describe = |e: ::windows::core::Error| format!("Could not open the camera: {e}");
    unsafe {
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 2).map_err(describe)?;
        let attributes = attributes.ok_or_else(|| describe(::windows::core::Error::empty()))?;
        attributes
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(describe)?;
        attributes
            .SetString(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                &HSTRING::from(symbolic_link),
            )
            .map_err(describe)?;
        let source = MFCreateDeviceSource(&attributes).map_err(describe)?;

        let mut reader_attributes = None;
        MFCreateAttributes(&mut reader_attributes, 1).map_err(describe)?;
        let reader_attributes =
            reader_attributes.ok_or_else(|| describe(::windows::core::Error::empty()))?;
        // Let the reader convert and scale to NV12 at the track size.
        reader_attributes
            .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
            .map_err(describe)?;
        let reader =
            MFCreateSourceReaderFromMediaSource(&source, &reader_attributes).map_err(describe)?;
        select_native_type(&reader, width, fps);
        let output = MFCreateMediaType().map_err(describe)?;
        output
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(describe)?;
        output
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(describe)?;
        output
            .SetUINT64(
                &MF_MT_FRAME_SIZE,
                (u64::from(width) << 32) | u64::from(height),
            )
            .map_err(describe)?;
        // Without a rate, the reader may renegotiate the camera to its fastest mode.
        output
            .SetUINT64(&MF_MT_FRAME_RATE, (u64::from(fps.max(1)) << 32) | 1)
            .map_err(describe)?;
        reader
            .SetCurrentMediaType(VIDEO_STREAM, None, &output)
            .map_err(describe)?;

        Ok((source, reader))
    }
}

/// Block for the next frame: `Ok(None)` for a stream tick without a picture.
pub fn read_sample(reader: &IMFSourceReader) -> Result<Option<(IMFSample, i64)>, String> {
    let mut flags = 0u32;
    let mut timestamp = 0i64;
    let mut sample = None;
    unsafe {
        reader
            .ReadSample(
                VIDEO_STREAM,
                0,
                None,
                Some(&mut flags),
                Some(&mut timestamp),
                Some(&mut sample),
            )
            .map_err(|e| describe_read_error(&e))?;
    }
    if flags & (MF_SOURCE_READERF_ERROR.0 | MF_SOURCE_READERF_ENDOFSTREAM.0) as u32 != 0 {
        return Err("The camera was disconnected or is in use by another app".into());
    }
    Ok(sample.map(|sample| (sample, timestamp)))
}

/// Copy a packed NV12 sample for the live preview.
pub fn preview_frame(sample: &IMFSample, width: u32, height: u32) -> Option<CameraFrame> {
    let expected = (width * height * 3 / 2) as usize;
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer().ok()?;
        let mut data = std::ptr::null_mut();
        let mut length = 0u32;
        buffer.Lock(&mut data, None, Some(&mut length)).ok()?;
        let frame = (length as usize >= expected && !data.is_null()).then(|| CameraFrame {
            width,
            height,
            nv12: std::slice::from_raw_parts(data, expected).to_vec(),
        });
        let _ = buffer.Unlock();
        frame
    }
}

struct CameraRecorder {
    context: TrackContext,
    clock: RecordingClock,
    source: IMFMediaSource,
    reader: IMFSourceReader,
    frame_hns: u64,
    /// Added to driver timestamps to reach the performance counter.
    timestamp_offset: Option<i64>,
    video: VideoSegments,
    width: u32,
    height: u32,
    last_preview: Option<Instant>,
}

const VIDEO_STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

impl CameraRecorder {
    fn open(
        context: TrackContext,
        symbolic_link: &str,
        format: VideoFormat,
        clock: RecordingClock,
    ) -> Result<Self, String> {
        let (source, reader) = open_reader(symbolic_link, format.width, format.height, format.fps)?;
        let video = VideoSegments::new(&context, format, None)?;
        Ok(Self {
            frame_hns: HNS_PER_SECOND / u64::from(format.fps.max(1)),
            context,
            clock,
            source,
            reader,
            timestamp_offset: None,
            video,
            width: format.width,
            height: format.height,
            last_preview: None,
        })
    }

    fn run(mut self, commands: &Receiver<TrackCommand>) {
        loop {
            match next_command(commands) {
                Some(TrackCommand::Stop) => {
                    if let Err(error) = self.video.close(None) {
                        self.context.fail(error);
                    }
                    self.shutdown();
                    return;
                }
                Some(TrackCommand::Finalize(reply)) => {
                    let _ = self.video.close(Some(reply));
                }
                None => {}
            }
            if let Err(error) = self.read_frame() {
                return self.fail_and_wait(error, commands);
            }
        }
    }

    /// Block for the next camera frame (one frame interval at most).
    fn read_frame(&mut self) -> Result<(), String> {
        let Some((sample, timestamp)) = read_sample(&self.reader)? else {
            return Ok(());
        };
        let arrival = now_hns();
        self.context.stats.record_sample();
        if self
            .last_preview
            .is_none_or(|last| last.elapsed() >= PREVIEW_INTERVAL)
        {
            if let Some(frame) = preview_frame(&sample, self.width, self.height) {
                frames::offer_camera(frame);
            }
            self.last_preview = Some(Instant::now());
        }
        let offset = *self.timestamp_offset.get_or_insert_with(|| {
            if (arrival as i64 - timestamp).unsigned_abs() <= CLOCK_MATCH_HNS {
                0
            } else {
                arrival as i64 - timestamp
            }
        });
        let frame_time = (timestamp + offset).max(0) as u64;
        if self.context.is_paused() {
            return Ok(());
        }
        let Some(frame_us) = self.clock.session_us(frame_time) else {
            return Ok(());
        };
        let duration = unsafe { sample.GetSampleDuration() }
            .ok()
            .filter(|&d| d > 0)
            .map_or(self.frame_hns, |d| d as u64);

        self.video
            .write_sample(&sample, frame_us, frame_time, duration)
    }

    fn shutdown(&mut self) {
        self.video.finish();
        // Release the camera for other apps right away.
        let _ = unsafe { self.source.Shutdown() };
    }

    fn fail_and_wait(mut self, error: String, commands: &Receiver<TrackCommand>) {
        self.context.fail(error.clone());
        let _ = self.video.close(None);
        self.shutdown();
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

/// Name the common case: another app (OBS, Teams, a browser) holds the camera.
fn describe_read_error(error: &::windows::core::Error) -> String {
    const HW_MFT_FAILED_START_STREAMING: u32 = 0xC00D_3704;
    const E_ACCESSDENIED: u32 = 0x8007_0005;
    const VIDEO_RECORDING_DEVICE_LOCKED: u32 = 0xC00D_3E86;
    match error.code().0 as u32 {
        HW_MFT_FAILED_START_STREAMING | E_ACCESSDENIED | VIDEO_RECORDING_DEVICE_LOCKED => {
            "The camera is in use by another app. Close that app or choose another camera.".into()
        }
        _ => format!("The camera stopped delivering frames: {error}"),
    }
}

/// Prefer a native 16:9 mode at the requested width or larger, closest to the
/// requested frame rate, so the reader scales down rather than up or
/// stretches. A smaller mode is used only when the camera has nothing larger.
/// Best effort: the camera's default mode is kept when nothing fits.
fn select_native_type(reader: &IMFSourceReader, target_width: u32, fps: u32) {
    let target_width = i64::from(target_width);
    let mut best: Option<(i64, IMFMediaType)> = None;
    for index in 0.. {
        let Ok(media_type) = (unsafe { reader.GetNativeMediaType(VIDEO_STREAM, index) }) else {
            break;
        };
        let size = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }.unwrap_or(0);
        let rate = unsafe { media_type.GetUINT64(&MF_MT_FRAME_RATE) }.unwrap_or(0);
        let (width, height) = ((size >> 32) as i64, (size & 0xffff_ffff) as i64);
        let (numerator, denominator) = ((rate >> 32) as i64, (rate & 0xffff_ffff).max(1) as i64);
        if width <= 0 || width * 9 != height * 16 {
            continue;
        }
        let mode_fps = (numerator + denominator / 2) / denominator;
        // Closest rate, then the smallest size at least the requested width,
        // then the largest smaller one.
        let size_penalty = if width >= target_width {
            width - target_width
        } else {
            50_000 + (target_width - width)
        };
        let score = (mode_fps - i64::from(fps)).abs() * 100_000 + size_penalty;
        if best
            .as_ref()
            .is_none_or(|(best_score, _)| score < *best_score)
        {
            best = Some((score, media_type));
        }
    }
    if let Some((_, media_type)) = best {
        let _ = unsafe { reader.SetCurrentMediaType(VIDEO_STREAM, None, &media_type) };
    }
}
