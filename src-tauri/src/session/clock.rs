use std::time::Instant;

/// Monotonic session epoch established *before* starting any capture stream.
/// All capture timestamps (screen PTS, webcam PTS, microphone samples, system audio loopback, mouse telemetry)
/// are mapped to integer microseconds (`t_us`) relative to this epoch.
#[derive(Debug, Clone)]
pub struct SessionEpoch {
    start_instant: Instant,
    start_wall_time_us: i64,
}

impl SessionEpoch {
    /// Creates and establishes a new session epoch.
    pub fn now() -> Self {
        let wall_us = chrono::Utc::now().timestamp_micros();
        Self {
            start_instant: Instant::now(),
            start_wall_time_us: wall_us,
        }
    }

    /// Calculates session-relative integer microseconds from an `Instant`.
    pub fn elapsed_us_at(&self, instant: Instant) -> u64 {
        if instant < self.start_instant {
            0
        } else {
            instant.duration_since(self.start_instant).as_micros() as u64
        }
    }

    /// Current session-relative elapsed microseconds.
    pub fn current_elapsed_us(&self) -> u64 {
        self.elapsed_us_at(Instant::now())
    }

    /// Converts audio sample count at a given sample rate (e.g. 48000 Hz) to session microseconds.
    pub fn samples_to_us(samples: u64, sample_rate: u32) -> u64 {
        if sample_rate == 0 {
            return 0;
        }
        (samples as u128 * 1_000_000 / sample_rate as u128) as u64
    }

    /// Converts session microseconds to audio sample count.
    pub fn us_to_samples(time_us: u64, sample_rate: u32) -> u64 {
        (time_us as u128 * sample_rate as u128 / 1_000_000) as u64
    }

    /// Wall clock timestamp (UTC microseconds) when the session started.
    pub fn start_wall_time_us(&self) -> i64 {
        self.start_wall_time_us
    }
}

/// Estimates audio/video clock drift in parts per million (PPM)
#[derive(Debug, Clone)]
pub struct ClockDriftEstimator {
    sample_rate: u32,
    cumulative_samples: u64,
    start_time_us: u64,
}

impl ClockDriftEstimator {
    pub fn new(sample_rate: u32, start_time_us: u64) -> Self {
        Self {
            sample_rate,
            cumulative_samples: 0,
            start_time_us,
        }
    }

    pub fn update(&mut self, samples_added: u64, current_wall_us: u64) -> f64 {
        self.cumulative_samples += samples_added;
        let expected_us = SessionEpoch::samples_to_us(self.cumulative_samples, self.sample_rate);
        let actual_us = current_wall_us.saturating_sub(self.start_time_us);

        if actual_us == 0 {
            return 0.0;
        }

        let diff = expected_us as f64 - actual_us as f64;
        (diff / actual_us as f64) * 1_000_000.0
    }

    pub fn cumulative_samples(&self) -> u64 {
        self.cumulative_samples
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RationalTimebase {
    pub numerator: u32,
    pub denominator: u32,
}

impl RationalTimebase {
    pub const H264_90KHZ: Self = Self {
        numerator: 1,
        denominator: 90_000,
    };
    pub const AUDIO_48KHZ: Self = Self {
        numerator: 1,
        denominator: 48_000,
    };
    pub const AUDIO_44_1KHZ: Self = Self {
        numerator: 1,
        denominator: 44_100,
    };
    pub const NANOSECONDS: Self = Self {
        numerator: 1,
        denominator: 1_000_000_000,
    };
    pub const HUNDRED_NANOS: Self = Self {
        numerator: 1,
        denominator: 10_000_000,
    };

    pub fn new(numerator: u32, denominator: u32) -> Self {
        Self {
            numerator,
            denominator,
        }
    }

    /// Converts ticks in this rational timebase to session microseconds.
    pub fn ticks_to_us(&self, ticks: u64) -> u64 {
        if self.denominator == 0 {
            return 0;
        }
        ((ticks as u128 * self.numerator as u128 * 1_000_000) / self.denominator as u128) as u64
    }

    /// Converts session microseconds to ticks in this rational timebase.
    pub fn us_to_ticks(&self, us: u64) -> u64 {
        if self.numerator == 0 {
            return 0;
        }
        ((us as u128 * self.denominator as u128) / (self.numerator as u128 * 1_000_000)) as u64
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MappedTimestamp {
    pub native_ticks: u64,
    pub timebase: RationalTimebase,
    pub mapped_us: u64,
    pub is_discontinuity: bool,
}

/// Bridges native platform clock ticks (ScreenCaptureKit host ticks, CoreMedia CMTime, WASAPI device positions)
/// to session epoch microseconds, preserving original timestamps and tracking clock resets/discontinuities.
#[derive(Debug, Clone)]
pub struct NativeTimestampMapper {
    source_id: String,
    timebase: RationalTimebase,
    anchor_native_ticks: u64,
    anchor_session_us: u64,
    last_mapped_us: u64,
}

impl NativeTimestampMapper {
    pub fn new(
        source_id: String,
        timebase: RationalTimebase,
        anchor_native_ticks: u64,
        anchor_session_us: u64,
    ) -> Self {
        Self {
            source_id,
            timebase,
            anchor_native_ticks,
            anchor_session_us,
            last_mapped_us: anchor_session_us,
        }
    }

    pub fn map_native_ticks(&mut self, native_ticks: u64) -> MappedTimestamp {
        let (delta_us, is_discontinuity) = if native_ticks >= self.anchor_native_ticks {
            let delta_ticks = native_ticks - self.anchor_native_ticks;
            let us = self.timebase.ticks_to_us(delta_ticks);
            (us, false)
        } else {
            // Backward jump / clock reset detected
            (0, true)
        };

        let calculated_us = self.anchor_session_us + delta_us;
        let is_discontinuity = is_discontinuity || (calculated_us < self.last_mapped_us);
        let mapped_us = calculated_us.max(self.last_mapped_us);
        self.last_mapped_us = mapped_us;

        MappedTimestamp {
            native_ticks,
            timebase: self.timebase,
            mapped_us,
            is_discontinuity,
        }
    }

    pub fn timebase(&self) -> RationalTimebase {
        self.timebase
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn test_session_epoch_monotonicity() {
        let epoch = SessionEpoch::now();
        let t1 = epoch.current_elapsed_us();
        sleep(Duration::from_millis(15));
        let t2 = epoch.current_elapsed_us();

        assert!(t2 > t1, "Session clock must strictly progress");
        assert!(
            t2 >= 10_000,
            "Should have elapsed at least 10ms (10,000 us)"
        );
    }

    #[test]
    fn test_sample_conversion() {
        let sample_rate = 48_000;
        let samples = 48_000; // 1 second
        let us = SessionEpoch::samples_to_us(samples, sample_rate);
        assert_eq!(us, 1_000_000);

        let roundtrip_samples = SessionEpoch::us_to_samples(us, sample_rate);
        assert_eq!(roundtrip_samples, samples);
    }
}
