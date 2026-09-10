use serde::{Deserialize, Serialize};

pub const DEFAULT_WINDOW_MS: u32 = 20;
pub const DEFAULT_STEP_MS: u32 = 10;
pub const MAX_ENERGY_POLICY: &str = "max_energy";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelPolicy {
    MaxEnergy,
    Channel(u16),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SilenceConfig {
    pub threshold_db: f32,    // e.g. -38.0 dBFS
    pub min_duration_ms: u32, // e.g. 400 ms
    pub padding_ms: u32,      // e.g. 50 ms
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_policy: Option<String>,
}

impl Default for SilenceConfig {
    fn default() -> Self {
        Self {
            threshold_db: -38.0,
            min_duration_ms: 400,
            padding_ms: 50,
            window_ms: None,
            step_ms: None,
            channel_policy: None,
        }
    }
}

impl SilenceConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !self.threshold_db.is_finite() {
            return Err("Silence threshold must be finite".into());
        }
        let window_ms = self.window_ms.unwrap_or(DEFAULT_WINDOW_MS);
        if window_ms == 0 {
            return Err("Silence window must be greater than zero".into());
        }
        match self.step_ms {
            Some(0) => return Err("Silence step must be greater than zero".into()),
            Some(step) if step > window_ms => {
                return Err("Silence step cannot exceed window".into());
            }
            _ => {}
        }
        parse_channel_policy(self.channel_policy.as_deref())?;
        Ok(())
    }

    pub fn resolved_window_ms(&self) -> u32 {
        self.window_ms.unwrap_or(DEFAULT_WINDOW_MS).max(1)
    }

    pub fn resolved_step_ms(&self) -> u32 {
        match self.step_ms {
            Some(step) => step.max(1),
            None => (self.resolved_window_ms() / 2).max(1),
        }
    }

    pub fn resolved_policy(&self) -> Result<ChannelPolicy, String> {
        parse_channel_policy(self.channel_policy.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SilenceCutInterval {
    pub id: String,
    pub start_us: u64,
    pub end_us: u64,
    pub duration_ms: u64,
    pub selected: bool,
    #[serde(default)]
    pub source_start_us: u64,
    #[serde(default)]
    pub source_end_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SilenceDetectionResult {
    pub track_id: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub channel_policy: String,
    pub suggestions: Vec<SilenceCutInterval>,
    pub diagnostics: Vec<String>,
}

pub struct SilenceDetector;

impl SilenceDetector {
    /// Computes RMS (Root Mean Square) energy of an audio sample buffer.
    pub fn compute_rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }

        let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }

    /// Converts an RMS amplitude value (0.0 to 1.0) to decibels relative to full scale (dBFS).
    pub fn rms_to_dbfs(rms: f32) -> f32 {
        if rms <= 1e-6 {
            -120.0
        } else {
            20.0 * rms.log10()
        }
    }

    /// Scans a monophonic PCM f32 buffer. Times are counted in sample frames.
    pub fn detect_silence(
        samples: &[f32],
        sample_rate: u32,
        config: &SilenceConfig,
    ) -> Result<Vec<SilenceCutInterval>, String> {
        Self::detect_interleaved(samples, sample_rate, 1, config)
    }

    /// Scans interleaved sample frames. `channels` is the frame width, not a time scale.
    pub fn detect_interleaved(
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
        config: &SilenceConfig,
    ) -> Result<Vec<SilenceCutInterval>, String> {
        let mut detector = StreamingSilenceDetector::new(sample_rate, channels, config)?;
        detector.feed(samples, 0)?;
        Ok(intervals_from_source_ranges(detector.finish()))
    }
}

pub(crate) struct StreamingSilenceDetector {
    sample_rate: u32,
    channels: usize,
    policy: ChannelPolicy,
    threshold_db: f32,
    config: SilenceConfig,
    window_frames: usize,
    step_frames: usize,
    leftover: Vec<f32>,
    leftover_origin_us: u64,
    next_source_us: Option<u64>,
    in_silence: bool,
    silence_start_us: u64,
    regions: Vec<(u64, u64)>,
}

impl StreamingSilenceDetector {
    pub(crate) fn new(
        sample_rate: u32,
        channels: u16,
        config: &SilenceConfig,
    ) -> Result<Self, String> {
        config.validate()?;
        if sample_rate == 0 {
            return Err("Silence sample rate must be greater than zero".into());
        }
        if channels == 0 {
            return Err("Silence channel count must be greater than zero".into());
        }
        let policy = config.resolved_policy()?;
        if let ChannelPolicy::Channel(index) = policy {
            if index as usize >= channels as usize {
                return Err("Selected silence channel is out of range".into());
            }
        }
        let window_frames = frames_for_ms(sample_rate, config.resolved_window_ms());
        let step_frames = frames_for_ms(sample_rate, config.resolved_step_ms());
        if window_frames == 0 {
            return Err("Silence window is smaller than one sample frame".into());
        }
        if step_frames == 0 {
            return Err("Silence step is smaller than one sample frame".into());
        }
        Ok(Self {
            sample_rate,
            channels: channels as usize,
            policy,
            threshold_db: config.threshold_db,
            config: config.clone(),
            window_frames,
            step_frames,
            leftover: Vec::new(),
            leftover_origin_us: 0,
            next_source_us: None,
            in_silence: false,
            silence_start_us: 0,
            regions: Vec::new(),
        })
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub(crate) fn channels(&self) -> u16 {
        self.channels as u16
    }

    pub(crate) fn notify_discontinuity(&mut self) {
        self.flush_run();
        self.reset_window_state();
    }

    pub(crate) fn feed(&mut self, interleaved: &[f32], source_start_us: u64) -> Result<(), String> {
        if interleaved.is_empty() {
            return Ok(());
        }
        if interleaved.len() % self.channels != 0 {
            return Err("PCM chunk is not an integer number of sample frames".into());
        }
        if !self.is_contiguous(source_start_us) {
            self.notify_discontinuity();
        }

        let mut buffer = std::mem::take(&mut self.leftover);
        let origin = if buffer.is_empty() {
            source_start_us
        } else {
            self.leftover_origin_us
        };
        buffer.extend_from_slice(interleaved);

        let total_frames = buffer.len() / self.channels;
        let mut frame_idx = 0usize;
        while frame_idx + self.window_frames <= total_frames {
            let start = frame_idx * self.channels;
            let end = (frame_idx + self.window_frames) * self.channels;
            let silent = window_is_silent(
                &buffer[start..end],
                self.channels,
                self.policy,
                self.threshold_db,
            );
            let window_us = origin.saturating_add(frame_us(frame_idx as u64, self.sample_rate));
            self.observe(silent, window_us);
            frame_idx += self.step_frames;
        }

        let leftover_frames = total_frames.saturating_sub(frame_idx);
        if leftover_frames > 0 {
            let start = frame_idx * self.channels;
            self.leftover = buffer[start..].to_vec();
            self.leftover_origin_us =
                origin.saturating_add(frame_us(frame_idx as u64, self.sample_rate));
        } else {
            self.leftover.clear();
            self.leftover_origin_us = 0;
        }
        self.next_source_us =
            Some(origin.saturating_add(frame_us(total_frames as u64, self.sample_rate)));
        Ok(())
    }

    pub(crate) fn take_raw_regions(&mut self) -> Vec<(u64, u64)> {
        self.flush_run();
        std::mem::take(&mut self.regions)
    }

    pub(crate) fn finish(mut self) -> Vec<(u64, u64)> {
        let raw = self.take_raw_regions();
        finalize_regions(raw, &self.config)
    }

    fn is_contiguous(&self, source_start_us: u64) -> bool {
        let Some(expected) = self.next_source_us else {
            return true;
        };
        if source_start_us == expected {
            return true;
        }
        let frame = (1_000_000u128 / self.sample_rate.max(1) as u128) as u64;
        source_start_us.abs_diff(expected) <= frame.max(1)
    }

    fn observe(&mut self, silent: bool, window_us: u64) {
        if silent && !self.in_silence {
            self.in_silence = true;
            self.silence_start_us = window_us;
        } else if !silent && self.in_silence {
            self.in_silence = false;
            if window_us > self.silence_start_us {
                self.regions.push((self.silence_start_us, window_us));
            }
        }
    }

    fn flush_run(&mut self) {
        if !self.in_silence {
            return;
        }
        let leftover_frames = if self.channels == 0 {
            0
        } else {
            self.leftover.len() / self.channels
        };
        let end_us = if leftover_frames > 0 {
            self.leftover_origin_us
                .saturating_add(frame_us(leftover_frames as u64, self.sample_rate))
        } else {
            self.next_source_us.unwrap_or(self.silence_start_us)
        };
        if end_us > self.silence_start_us {
            self.regions.push((self.silence_start_us, end_us));
        }
        self.in_silence = false;
    }

    fn reset_window_state(&mut self) {
        self.leftover.clear();
        self.leftover_origin_us = 0;
        self.next_source_us = None;
        self.in_silence = false;
    }
}

pub(crate) fn parse_channel_policy(value: Option<&str>) -> Result<ChannelPolicy, String> {
    match value.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(ChannelPolicy::MaxEnergy),
        Some("max_energy") | Some("maxEnergy") => Ok(ChannelPolicy::MaxEnergy),
        Some(rest) if rest.starts_with("channel:") => {
            let index = rest[8..]
                .parse::<u16>()
                .map_err(|_| "Invalid silence channel policy".to_string())?;
            Ok(ChannelPolicy::Channel(index))
        }
        Some(_) => Err("Unknown silence channel policy".into()),
    }
}

pub(crate) fn channel_policy_name(policy: ChannelPolicy) -> String {
    match policy {
        ChannelPolicy::MaxEnergy => MAX_ENERGY_POLICY.into(),
        ChannelPolicy::Channel(index) => format!("channel:{index}"),
    }
}

pub(crate) fn finalize_regions(
    mut regions: Vec<(u64, u64)>,
    config: &SilenceConfig,
) -> Vec<(u64, u64)> {
    regions.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in regions {
        if end <= start {
            continue;
        }
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    let min_duration_us = config.min_duration_ms as u64 * 1_000;
    let padding_us = config.padding_ms as u64 * 1_000;
    let mut out = Vec::new();
    for (start, end) in merged {
        if end.saturating_sub(start) < min_duration_us {
            continue;
        }
        let padded_start = start.saturating_add(padding_us);
        let padded_end = end.saturating_sub(padding_us);
        if padded_end > padded_start {
            out.push((padded_start, padded_end));
        }
    }
    out
}

pub(crate) fn intervals_from_source_ranges(ranges: Vec<(u64, u64)>) -> Vec<SilenceCutInterval> {
    ranges
        .into_iter()
        .enumerate()
        .map(|(idx, (start, end))| SilenceCutInterval {
            id: format!("silence-{}", idx + 1),
            start_us: start,
            end_us: end,
            duration_ms: end.saturating_sub(start) / 1_000,
            selected: true,
            source_start_us: start,
            source_end_us: end,
        })
        .collect()
}

fn frames_for_ms(sample_rate: u32, ms: u32) -> usize {
    (ms as u128 * sample_rate as u128 / 1_000) as usize
}

fn frame_us(frame_index: u64, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    (frame_index as u128 * 1_000_000 / sample_rate as u128) as u64
}

fn window_is_silent(
    interleaved: &[f32],
    channels: usize,
    policy: ChannelPolicy,
    threshold_db: f32,
) -> bool {
    if channels == 0 || interleaved.len() < channels {
        return false;
    }
    let frames = interleaved.len() / channels;
    if frames == 0 {
        return false;
    }
    let rms = match policy {
        ChannelPolicy::MaxEnergy => {
            let mut best = 0.0f32;
            for ch in 0..channels {
                best = best.max(channel_rms(interleaved, channels, frames, ch));
            }
            best
        }
        ChannelPolicy::Channel(index) => channel_rms(interleaved, channels, frames, index as usize),
    };
    SilenceDetector::rms_to_dbfs(rms) < threshold_db
}

fn channel_rms(interleaved: &[f32], channels: usize, frames: usize, channel: usize) -> f32 {
    if channel >= channels || frames == 0 {
        return 0.0;
    }
    let mut sum_sq = 0.0f32;
    for frame in 0..frames {
        let x = interleaved[frame * channels + channel];
        sum_sq += x * x;
    }
    (sum_sq / frames as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speech_tone(sample_rate: u32, seconds: u32) -> Vec<f32> {
        (0..sample_rate * seconds)
            .map(|i| {
                ((i as f32 * 440.0 * 2.0 * std::f32::consts::PI) / sample_rate as f32).sin() * 0.5
            })
            .collect()
    }

    #[test]
    fn test_rms_and_dbfs() {
        let full_scale = vec![1.0f32; 100];
        let rms = SilenceDetector::compute_rms(&full_scale);
        assert!((rms - 1.0).abs() < 1e-4);
        assert!((SilenceDetector::rms_to_dbfs(rms) - 0.0).abs() < 1e-3);

        let quiet = vec![0.001f32; 100];
        let quiet_rms = SilenceDetector::compute_rms(&quiet);
        let quiet_db = SilenceDetector::rms_to_dbfs(quiet_rms);
        assert!(quiet_db < -55.0);
    }

    #[test]
    fn test_silence_detection_with_synthetic_audio() {
        let sample_rate = 48_000;
        let mut samples = speech_tone(sample_rate, 1);
        samples.extend(std::iter::repeat_n(0.0, sample_rate as usize));
        samples.extend(speech_tone(sample_rate, 1));

        let config = SilenceConfig::default();
        let cuts = SilenceDetector::detect_silence(&samples, sample_rate, &config).unwrap();
        assert_eq!(cuts.len(), 1, "Should detect exactly 1 silence pocket");

        let cut = &cuts[0];
        assert!(cut.start_us >= 1_000_000);
        assert!(cut.end_us <= 2_000_000);
        assert!(cut.duration_ms >= 800, "Padded duration should be ~900ms");
    }

    #[test]
    fn rejects_zero_window_and_non_finite_threshold() {
        let mut zero_window = SilenceConfig::default();
        zero_window.window_ms = Some(0);
        let err = zero_window.validate().unwrap_err();
        assert!(err.to_lowercase().contains("window"));

        let mut zero_step = SilenceConfig::default();
        zero_step.step_ms = Some(0);
        assert!(zero_step
            .validate()
            .unwrap_err()
            .to_lowercase()
            .contains("step"));

        let mut nan = SilenceConfig::default();
        nan.threshold_db = f32::NAN;
        assert!(nan
            .validate()
            .unwrap_err()
            .to_lowercase()
            .contains("finite"));

        let mut inf = SilenceConfig::default();
        inf.threshold_db = f32::INFINITY;
        assert!(inf
            .validate()
            .unwrap_err()
            .to_lowercase()
            .contains("finite"));
    }

    #[test]
    fn opposite_polarity_stereo_is_not_silence() {
        let sample_rate = 48_000;
        let frames = sample_rate as usize * 2;
        let mut samples = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            samples.push(0.5);
            samples.push(-0.5);
        }
        let cuts = SilenceDetector::detect_interleaved(
            &samples,
            sample_rate,
            2,
            &SilenceConfig::default(),
        )
        .unwrap();
        assert!(
            cuts.is_empty(),
            "max-energy must keep opposite-polarity stereo"
        );
    }

    #[test]
    fn selected_channel_timing_uses_sample_frames() {
        let sample_rate = 48_000u32;
        let frames = sample_rate as usize;
        let mut samples = Vec::with_capacity(frames * 3 * 2);
        for i in 0..frames * 3 {
            let ch0 = if i >= frames && i < frames * 2 {
                0.0
            } else {
                0.5
            };
            samples.push(ch0);
            samples.push(0.5);
        }
        let mut config = SilenceConfig::default();
        config.channel_policy = Some("channel:0".into());
        let cuts = SilenceDetector::detect_interleaved(&samples, sample_rate, 2, &config).unwrap();
        assert_eq!(cuts.len(), 1);
        assert!(
            cuts[0].start_us >= 1_000_000 && cuts[0].end_us <= 2_000_000,
            "channel timing must use frames, got {}-{}",
            cuts[0].start_us,
            cuts[0].end_us
        );

        let max_energy = SilenceDetector::detect_interleaved(
            &samples,
            sample_rate,
            2,
            &SilenceConfig::default(),
        )
        .unwrap();
        assert!(
            max_energy.is_empty(),
            "loud second channel should suppress max-energy cuts"
        );
    }

    #[test]
    fn contiguous_chunks_preserve_one_silence_run() {
        let sample_rate = 48_000;
        let config = SilenceConfig::default();
        let mut detector = StreamingSilenceDetector::new(sample_rate, 1, &config).unwrap();
        let first = vec![0.0f32; sample_rate as usize];
        let second = vec![0.0f32; sample_rate as usize];
        detector.feed(&first, 0).unwrap();
        detector.feed(&second, 1_000_000).unwrap();
        let ranges = detector.finish();
        assert_eq!(ranges.len(), 1, "contiguous silent chunks must merge");
        assert!(ranges[0].1.saturating_sub(ranges[0].0) >= 1_800_000);
    }

    #[test]
    fn discontinuity_resets_and_does_not_invent_silence() {
        let sample_rate = 48_000;
        let config = SilenceConfig {
            min_duration_ms: 400,
            padding_ms: 0,
            ..SilenceConfig::default()
        };
        let mut detector = StreamingSilenceDetector::new(sample_rate, 1, &config).unwrap();
        detector.feed(&vec![0.0; sample_rate as usize], 0).unwrap();
        detector.notify_discontinuity();
        detector
            .feed(&vec![0.0; sample_rate as usize], 2_000_000)
            .unwrap();
        let ranges = detector.finish();
        assert_eq!(ranges.len(), 2);
        assert!(ranges[0].1 <= 1_000_000);
        assert!(ranges[1].0 >= 2_000_000);
    }
}
