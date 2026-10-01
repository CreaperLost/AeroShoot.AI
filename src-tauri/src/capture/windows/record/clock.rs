//! One timebase for every Windows source. WASAPI packet positions, Windows
//! Graphics Capture frame times, and Media Foundation camera sample times are
//! all the performance counter expressed in 100-nanosecond units ("hns").
use ::windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use std::sync::OnceLock;

pub const HNS_PER_SECOND: u64 = 10_000_000;

fn frequency() -> u64 {
    static FREQUENCY: OnceLock<u64> = OnceLock::new();
    *FREQUENCY.get_or_init(|| {
        let mut value = 0i64;
        // Cannot fail on Windows XP and later.
        let _ = unsafe { QueryPerformanceFrequency(&mut value) };
        value.max(1) as u64
    })
}

/// Current performance-counter time in 100-nanosecond units.
pub fn now_hns() -> u64 {
    let mut counter = 0i64;
    let _ = unsafe { QueryPerformanceCounter(&mut counter) };
    ((counter.max(0) as u128 * HNS_PER_SECOND as u128) / frequency() as u128) as u64
}

pub fn hns_to_us(hns: u64) -> u64 {
    hns / 10
}

/// Recording time zero, fixed when the session starts: the end of the
/// countdown. Samples before it only warm sources up and are not written.
#[derive(Clone, Copy, Debug)]
pub struct RecordingClock {
    start_hns: u64,
}

impl RecordingClock {
    pub fn starting_after(delay_ms: u32) -> Self {
        Self {
            start_hns: now_hns() + u64::from(delay_ms) * 10_000,
        }
    }

    pub fn start_hns(&self) -> u64 {
        self.start_hns
    }

    /// Session-relative microseconds for a source timestamp, or `None`
    /// during the countdown.
    pub fn session_us(&self, source_hns: u64) -> Option<u64> {
        source_hns.checked_sub(self.start_hns).map(hns_to_us)
    }

    /// Microseconds since recording began, or `None` during the countdown.
    pub fn started_ago_us(&self) -> Option<u64> {
        self.session_us(now_hns())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_is_monotonic_and_in_hns() {
        let first = now_hns();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let elapsed = now_hns() - first;
        assert!((150_000..2_000_000).contains(&elapsed), "{elapsed}");
    }

    #[test]
    fn countdown_samples_have_no_session_time() {
        let clock = RecordingClock::starting_after(10_000);
        assert_eq!(clock.started_ago_us(), None);
        assert_eq!(clock.session_us(clock.start_hns() - 1), None);
        assert_eq!(clock.session_us(clock.start_hns() + 15), Some(1));
    }
}
