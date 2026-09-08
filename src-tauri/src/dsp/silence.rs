use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SilenceConfig {
    pub threshold_db: f32,    // e.g. -38.0 dBFS
    pub min_duration_ms: u32, // e.g. 400 ms
    pub padding_ms: u32,      // e.g. 50 ms
}

impl Default for SilenceConfig {
    fn default() -> Self {
        Self {
            threshold_db: -38.0,
            min_duration_ms: 400,
            padding_ms: 50,
        }
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
}

pub struct SilenceDetector;

impl SilenceDetector {
    /// Computes RMS (Root Mean Square) energy of an audio sample buffer.
    pub fn compute_rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }

        // Sum of squares with vectorization friendliness
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

    /// Scans a monophonic PCM f32 buffer at given sample rate and returns detected silence intervals
    /// conforming to the threshold, minimum duration, and syllable padding parameters.
    pub fn detect_silence(
        samples: &[f32],
        sample_rate: u32,
        config: &SilenceConfig,
    ) -> Vec<SilenceCutInterval> {
        if samples.is_empty() || sample_rate == 0 {
            return Vec::new();
        }

        // 20ms analysis window
        let window_size = (sample_rate as f32 * 0.02) as usize;
        let step_size = window_size / 2; // 50% overlap

        let mut silent_regions: Vec<(u64, u64)> = Vec::new();
        let mut in_silence = false;
        let mut silence_start_us: u64 = 0;

        let total_windows = if samples.len() >= window_size {
            (samples.len() - window_size) / step_size + 1
        } else {
            0
        };

        for w in 0..total_windows {
            let start_idx = w * step_size;
            let end_idx = start_idx + window_size;
            let window = &samples[start_idx..end_idx];

            let rms = Self::compute_rms(window);
            let dbfs = Self::rms_to_dbfs(rms);
            let is_silent = dbfs < config.threshold_db;

            let window_center_sample = start_idx + window_size / 2;
            let current_us = (window_center_sample as u128 * 1_000_000 / sample_rate as u128) as u64;

            if is_silent && !in_silence {
                in_silence = true;
                silence_start_us = current_us;
            } else if !is_silent && in_silence {
                in_silence = false;
                silent_regions.push((silence_start_us, current_us));
            }
        }

        // Flush trailing silence if audio ends during silence
        if in_silence {
            let total_us = (samples.len() as u128 * 1_000_000 / sample_rate as u128) as u64;
            silent_regions.push((silence_start_us, total_us));
        }

        // Filter by minimum duration and apply syllable padding
        let min_duration_us = (config.min_duration_ms as u64) * 1_000;
        let padding_us = (config.padding_ms as u64) * 1_000;
        let mut results = Vec::new();

        for (idx, (start, end)) in silent_regions.into_iter().enumerate() {
            let raw_duration_us = end.saturating_sub(start);
            if raw_duration_us >= min_duration_us {
                // Shrink silence range by padding to protect syllable beginnings/endings
                let padded_start = start.saturating_add(padding_us);
                let padded_end = end.saturating_sub(padding_us);

                if padded_end > padded_start {
                    let duration_us = padded_end - padded_start;
                    results.push(SilenceCutInterval {
                        id: format!("silence-{}", idx + 1),
                        start_us: padded_start,
                        end_us: padded_end,
                        duration_ms: duration_us / 1_000,
                        selected: true,
                    });
                }
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rms_and_dbfs() {
        // Full scale DC signal at 1.0 -> 0 dBFS
        let full_scale = vec![1.0f32; 100];
        let rms = SilenceDetector::compute_rms(&full_scale);
        assert!((rms - 1.0).abs() < 1e-4);
        assert!((SilenceDetector::rms_to_dbfs(rms) - 0.0).abs() < 1e-3);

        // Near silence signal
        let quiet = vec![0.001f32; 100];
        let quiet_rms = SilenceDetector::compute_rms(&quiet);
        let quiet_db = SilenceDetector::rms_to_dbfs(quiet_rms);
        assert!(quiet_db < -55.0);
    }

    #[test]
    fn test_silence_detection_with_synthetic_audio() {
        let sample_rate = 48_000;
        let mut samples = Vec::new();

        // 1 sec active speech (sine wave, amplitude 0.5 ~ -6 dBFS)
        for i in 0..sample_rate {
            let s = ((i as f32 * 440.0 * 2.0 * std::f32::consts::PI) / sample_rate as f32).sin() * 0.5;
            samples.push(s);
        }

        // 1 sec dead silence (0.0)
        for _ in 0..sample_rate {
            samples.push(0.0);
        }

        // 1 sec active speech again
        for i in 0..sample_rate {
            let s = ((i as f32 * 440.0 * 2.0 * std::f32::consts::PI) / sample_rate as f32).sin() * 0.5;
            samples.push(s);
        }

        let config = SilenceConfig {
            threshold_db: -38.0,
            min_duration_ms: 400,
            padding_ms: 50,
        };

        let cuts = SilenceDetector::detect_silence(&samples, sample_rate, &config);
        assert_eq!(cuts.len(), 1, "Should detect exactly 1 silence pocket");

        let cut = &cuts[0];
        // The silence began around 1.0s and ended around 2.0s
        assert!(cut.start_us >= 1_000_000);
        assert!(cut.end_us <= 2_000_000);
        assert!(cut.duration_ms >= 800, "Padded duration should be ~900ms");
    }
}
