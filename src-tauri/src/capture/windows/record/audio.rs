//! WASAPI microphone and system-audio (loopback) recording into WAV segments.
//!
//! Windows converts every device to 48 kHz 16-bit PCM for us (mono mic,
//! stereo system audio). Packets carry performance-counter positions, which
//! place them on the session timeline. Loopback capture delivers nothing
//! while the system is silent, so its track is padded with silence to stay
//! continuous; the mic pads only real gaps between packets.
use super::clock::RecordingClock;
use super::track::{
    job, next_command, Job, Publisher, Reply, TrackCommand, TrackContext, SEGMENT_TARGET_US,
};
use super::wav::{WavSegment, SAMPLE_RATE};
use crate::capture::windows::ComApartment;
use ::windows::core::HSTRING;
use ::windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX, WAVE_FORMAT_PCM,
};
use ::windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// A later packet may still fill this much of the recent past, so idle
/// loopback silence is only committed behind it.
const LOOPBACK_LATENCY_US: u64 = 100_000;
/// Packet timing jitter tolerated before a gap is filled or overlap trimmed.
const GAP_TOLERANCE_US: u64 = 20_000;
const REOPEN_INTERVAL: Duration = Duration::from_millis(500);

pub enum AudioSource {
    Microphone { endpoint_id: String, gain_db: f32 },
    SystemLoopback,
}

impl AudioSource {
    fn channels(&self) -> u16 {
        match self {
            Self::Microphone { .. } => 1,
            Self::SystemLoopback => 2,
        }
    }

    fn is_loopback(&self) -> bool {
        matches!(self, Self::SystemLoopback)
    }
}

/// Open the device, then record on a dedicated thread. Returns once the
/// device is capturing, or with the reason it could not be opened.
pub fn spawn(
    context: TrackContext,
    source: AudioSource,
    clock: RecordingClock,
    commands: Receiver<TrackCommand>,
) -> Result<JoinHandle<()>, String> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name(format!("aeroshoot-{}", context.track_id))
        .spawn(move || {
            let com = match ComApartment::enter() {
                Ok(com) => com,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            let stream = match Stream::open(&source) {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            let recorder = match AudioRecorder::new(context, source, clock) {
                Ok(recorder) => recorder,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            recorder.run(stream, &commands);
            drop(com);
        })
        .map_err(|e| format!("Could not start the audio thread: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(thread),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            Err("The audio thread exited during startup".into())
        }
    }
}

struct Stream {
    client: IAudioClient,
    capture: IAudioCaptureClient,
}

impl Stream {
    fn open(source: &AudioSource) -> Result<Self, String> {
        let describe = |e: ::windows::core::Error| match source {
            AudioSource::Microphone { .. } => format!("Could not open the microphone: {e}"),
            AudioSource::SystemLoopback => format!("Could not capture system audio: {e}"),
        };
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(describe)?;
            let device = match source {
                AudioSource::Microphone { endpoint_id, .. } => {
                    enumerator.GetDevice(&HSTRING::from(endpoint_id.as_str()))
                }
                AudioSource::SystemLoopback => {
                    enumerator.GetDefaultAudioEndpoint(eRender, eConsole)
                }
            }
            .map_err(describe)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(describe)?;
            let channels = source.channels();
            let format = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_PCM as u16,
                nChannels: channels,
                nSamplesPerSec: SAMPLE_RATE,
                nAvgBytesPerSec: SAMPLE_RATE * u32::from(channels) * 2,
                nBlockAlign: channels * 2,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            let mut flags =
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            if source.is_loopback() {
                flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
            }
            // 200 ms of device buffering; polled every 10 ms.
            client
                .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &format, None)
                .map_err(describe)?;
            let capture: IAudioCaptureClient = client.GetService().map_err(describe)?;
            client.Start().map_err(describe)?;
            Ok(Self { client, capture })
        }
    }

    /// Drain every packet the device has ready: (QPC time in hns, samples).
    fn read_packets(&self, channels: u16) -> ::windows::core::Result<Vec<(u64, Vec<i16>, bool)>> {
        let mut packets = Vec::new();
        unsafe {
            while self.capture.GetNextPacketSize()? > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                let mut qpc = 0u64;
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc))?;
                let count = frames as usize * usize::from(channels);
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let samples = if silent || data.is_null() {
                    vec![0i16; count]
                } else {
                    std::slice::from_raw_parts(data as *const i16, count).to_vec()
                };
                self.capture.ReleaseBuffer(frames)?;
                let discontinuity = flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0;
                packets.push((qpc, samples, discontinuity));
            }
        }
        Ok(packets)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = unsafe { self.client.Stop() };
    }
}

struct OpenSegment {
    wav: WavSegment,
    anchor_us: u64,
}

impl OpenSegment {
    fn end_us(&self) -> u64 {
        self.anchor_us + self.wav.duration_us()
    }
}

struct AudioRecorder {
    context: TrackContext,
    source: AudioSource,
    clock: RecordingClock,
    channels: u16,
    gain: f32,
    segment: Option<OpenSegment>,
    next_index: u32,
    publisher: Publisher,
}

impl AudioRecorder {
    fn new(
        context: TrackContext,
        source: AudioSource,
        clock: RecordingClock,
    ) -> Result<Self, String> {
        let gain = match &source {
            AudioSource::Microphone { gain_db, .. } => {
                10f32.powf(gain_db.clamp(-24.0, 24.0) / 20.0)
            }
            AudioSource::SystemLoopback => 1.0,
        };
        Ok(Self {
            channels: source.channels(),
            publisher: Publisher::start(&context)?,
            context,
            source,
            clock,
            gain,
            segment: None,
            next_index: 0,
        })
    }

    fn run(mut self, stream: Stream, commands: &Receiver<TrackCommand>) {
        let mut stream = Some(stream);
        let mut last_reopen = Instant::now();
        loop {
            match next_command(commands) {
                Some(TrackCommand::Stop) => {
                    if let Err(error) = self.close_segment(None) {
                        self.context.fail(error);
                    }
                    self.publisher.finish();
                    return;
                }
                Some(TrackCommand::Finalize(reply)) => {
                    let _ = self.close_segment(Some(reply));
                }
                None => {}
            }

            if stream.is_none() && last_reopen.elapsed() >= REOPEN_INTERVAL {
                last_reopen = Instant::now();
                stream = Stream::open(&self.source).ok();
            }
            if let Some(active) = &stream {
                match active.read_packets(self.channels) {
                    Ok(packets) => {
                        for (qpc, samples, discontinuity) in packets {
                            if let Err(error) = self.handle_packet(qpc, samples, discontinuity) {
                                return self.fail_and_wait(error, commands);
                            }
                        }
                    }
                    // The default output device changed or went away: keep the
                    // track continuous with silence and follow the new device.
                    Err(_) if self.source.is_loopback() => {
                        stream = None;
                        self.context.stats.gaps.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(error) => {
                        return self.fail_and_wait(
                            format!("Microphone capture stopped: {error}"),
                            commands,
                        );
                    }
                }
            }
            if self.source.is_loopback() {
                if let Err(error) = self.pad_idle_loopback() {
                    return self.fail_and_wait(error, commands);
                }
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// After a fatal error, keep answering the session so Pause and Stop
    /// report the failure instead of hanging.
    fn fail_and_wait(mut self, error: String, commands: &Receiver<TrackCommand>) {
        self.context.fail(error.clone());
        // Keep whatever was captured; the discarded tail is covered by the error.
        let _ = self.close_segment(None);
        self.publisher.finish();
        while let Ok(command) = commands.recv() {
            match command {
                TrackCommand::Stop => return,
                TrackCommand::Finalize(reply) => {
                    let _ = reply.send(Err(error.clone()));
                }
            }
        }
    }

    fn handle_packet(
        &mut self,
        qpc_hns: u64,
        mut samples: Vec<i16>,
        discontinuity: bool,
    ) -> Result<(), String> {
        let channels = usize::from(self.channels);
        self.context.stats.record_sample();
        if self.gain != 1.0 {
            for sample in &mut samples {
                *sample = (f32::from(*sample) * self.gain).clamp(-32768.0, 32767.0) as i16;
            }
        }
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        self.context.stats.set_peak_db(peak_dbfs(peak));

        if self.context.is_paused() {
            return Ok(());
        }
        // Drop the part of the packet that precedes recording time zero.
        let frames = (samples.len() / channels) as u64;
        let start_hns = self.clock.start_hns();
        let mut packet_hns = qpc_hns;
        if packet_hns < start_hns {
            let early = frames_for_us((start_hns - packet_hns) / 10).min(frames);
            samples.drain(..early as usize * channels);
            packet_hns = start_hns;
        }
        if samples.is_empty() {
            return Ok(());
        }
        let Some(packet_us) = self.clock.session_us(packet_hns) else {
            return Ok(());
        };

        let track_id = self.context.track_id;
        let expected_us = self.open_segment(packet_us)?.end_us();
        let segment = self.segment.as_mut().expect("segment opened above");
        if packet_us > expected_us + GAP_TOLERANCE_US {
            segment
                .wav
                .write_silence(frames_for_us(packet_us - expected_us))
                .map_err(|e| write_error(track_id, e))?;
            if !self.source.is_loopback() || discontinuity {
                self.context.stats.gaps.fetch_add(1, Ordering::Relaxed);
            }
        } else if packet_us + GAP_TOLERANCE_US < expected_us {
            // Already covered (by loopback silence padding): keep only the tail.
            let overlap =
                frames_for_us(expected_us - packet_us).min((samples.len() / channels) as u64);
            samples.drain(..overlap as usize * channels);
        }
        let segment = self.segment.as_mut().expect("segment opened above");
        segment
            .wav
            .write(&samples)
            .map_err(|e| write_error(track_id, e))?;
        if segment.wav.duration_us() >= SEGMENT_TARGET_US {
            self.close_segment(None)?;
        }
        Ok(())
    }

    /// Loopback only: commit silence up to `now - latency` when no packets
    /// arrived, so a silent system still yields a continuous track.
    fn pad_idle_loopback(&mut self) -> Result<(), String> {
        if self.context.is_paused() {
            return Ok(());
        }
        let Some(now_us) = self.clock.started_ago_us() else {
            return Ok(());
        };
        self.open_segment(now_us)?;
        self.pad_open_segment(now_us.saturating_sub(LOOPBACK_LATENCY_US))?;
        if self
            .segment
            .as_ref()
            .is_some_and(|s| s.wav.duration_us() >= SEGMENT_TARGET_US)
        {
            self.close_segment(None)?;
        }
        Ok(())
    }

    /// Extend the open segment with silence up to session time `until_us`.
    fn pad_open_segment(&mut self, until_us: u64) -> Result<(), String> {
        let track_id = self.context.track_id;
        let Some(segment) = self.segment.as_mut() else {
            return Ok(());
        };
        let end_us = segment.end_us();
        if until_us > end_us {
            segment
                .wav
                .write_silence(frames_for_us(until_us - end_us))
                .map_err(|e| write_error(track_id, e))?;
        }
        Ok(())
    }

    fn open_segment(&mut self, anchor_us: u64) -> Result<&mut OpenSegment, String> {
        if self.segment.is_none() {
            let path = self.context.segment_path(self.next_index);
            let wav = WavSegment::create(&path, self.channels)
                .map_err(|e| write_error(self.context.track_id, e))?;
            self.segment = Some(OpenSegment { wav, anchor_us });
        }
        Ok(self.segment.as_mut().expect("segment is open"))
    }

    /// Close the open segment and hand it to the publisher; `reply` hears
    /// once it is committed. Loopback is first padded to now, which on Pause
    /// is the moment recording paused.
    fn close_segment(&mut self, reply: Option<Reply>) -> Result<(), String> {
        let closed = self.take_closed();
        self.publisher.close(closed, reply)
    }

    fn take_closed(&mut self) -> Result<Option<Job>, String> {
        if self.source.is_loopback() {
            if let Some(now_us) = self.clock.started_ago_us() {
                self.pad_open_segment(now_us)?;
            }
        }
        let Some(segment) = self.segment.take() else {
            return Ok(None);
        };
        if segment.wav.frames() == 0 {
            let _ = std::fs::remove_file(segment.wav.path());
            return Ok(None);
        }
        let index = self.next_index;
        self.next_index += 1;
        let context = self.context.clone();
        Ok(Some(job(move || {
            let path = segment
                .wav
                .finish()
                .map_err(|e| write_error(context.track_id, e))?;
            context.publish(index, segment.anchor_us, &path)
        })))
    }
}

fn write_error(track_id: &str, error: std::io::Error) -> String {
    format!("Could not write the {track_id} audio segment: {error}")
}

fn frames_for_us(us: u64) -> u64 {
    us * u64::from(SAMPLE_RATE) / 1_000_000
}

fn peak_dbfs(peak: u16) -> f64 {
    if peak == 0 {
        -160.0
    } else {
        (20.0 * (f64::from(peak) / 32_768.0).log10()).max(-160.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_levels_are_dbfs() {
        assert_eq!(peak_dbfs(0), -160.0);
        assert!((peak_dbfs(32_767) - 0.0).abs() < 0.01);
        assert!((peak_dbfs(16_384) + 6.02).abs() < 0.01);
    }

    #[test]
    fn frame_counts_follow_48k() {
        assert_eq!(frames_for_us(1_000_000), 48_000);
        assert_eq!(frames_for_us(10_000), 480);
    }
}

/// Level meter for the live preview: opens the device on its own thread and
/// keeps `peak_db` (an `f64` stored as bits, NaN until the first packet)
/// current until `stop` is set. Returns once the device is capturing.
pub fn spawn_meter(
    source: AudioSource,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    peak_db: std::sync::Arc<std::sync::atomic::AtomicU64>,
) -> Result<JoinHandle<()>, String> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("aeroshoot-meter".into())
        .spawn(move || {
            let _com = match ComApartment::enter() {
                Ok(com) => com,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            let stream = match Stream::open(&source) {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            let gain = match &source {
                AudioSource::Microphone { gain_db, .. } => {
                    10f32.powf(gain_db.clamp(-24.0, 24.0) / 20.0)
                }
                AudioSource::SystemLoopback => 1.0,
            };
            let mut quiet_since = Instant::now();
            while !stop.load(Ordering::SeqCst) {
                let packets = stream.read_packets(source.channels()).unwrap_or_default();
                let peak = packets
                    .iter()
                    .flat_map(|(_, samples, _)| samples.iter())
                    .map(|&s| ((f32::from(s) * gain).abs().min(32767.0)) as u16)
                    .max();
                match peak {
                    Some(peak) => {
                        peak_db.store(peak_dbfs(peak).to_bits(), Ordering::Relaxed);
                        quiet_since = Instant::now();
                    }
                    // Loopback delivers nothing while the system is silent.
                    None if quiet_since.elapsed() > Duration::from_millis(300) => {
                        peak_db.store((-160.0f64).to_bits(), Ordering::Relaxed);
                    }
                    None => {}
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        })
        .map_err(|e| format!("Could not start the level meter: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(thread),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => Err("The level meter exited during startup".into()),
    }
}
