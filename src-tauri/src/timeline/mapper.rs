use crate::timeline::interval::SourceInterval;
use serde::{Deserialize, Serialize};

/// Shared non-destructive timeline time mapper.
/// Maps edited output time to source recording time through the cumulative lengths
/// of retained source intervals.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimelineMapper {
    intervals: Vec<SourceInterval>,
}

impl TimelineMapper {
    pub fn new(intervals: Vec<SourceInterval>) -> Self {
        let mut sorted = intervals;
        sorted.sort_by_key(|i| i.start_us);
        Self { intervals: sorted }
    }

    pub fn intervals(&self) -> &[SourceInterval] {
        &self.intervals
    }

    /// Total edited timeline duration across all retained intervals.
    pub fn total_edited_duration_us(&self) -> u64 {
        self.intervals.iter().map(|i| i.duration_us()).sum()
    }

    /// Maps an edited timeline position `edited_us` to the corresponding source recording timestamp `source_us`.
    pub fn edited_to_source_us(&self, edited_us: u64) -> Option<u64> {
        let mut accumulated_us: u64 = 0;

        for interval in &self.intervals {
            let dur = interval.duration_us();
            if edited_us < accumulated_us + dur {
                let offset_in_interval = edited_us - accumulated_us;
                return Some(interval.start_us + offset_in_interval);
            }
            accumulated_us += dur;
        }

        // Clamped to end of last interval if exact match at duration
        if let Some(last) = self.intervals.last() {
            if edited_us == accumulated_us {
                return Some(last.end_us);
            }
        }

        None
    }

    /// Maps a source recording timestamp `source_us` to its edited timeline position.
    /// Returns `None` if the source timestamp was cut/excluded.
    pub fn source_to_edited_us(&self, source_us: u64) -> Option<u64> {
        let mut accumulated_us: u64 = 0;

        for interval in &self.intervals {
            if interval.contains_source_us(source_us) {
                let offset = source_us - interval.start_us;
                return Some(accumulated_us + offset);
            }
            accumulated_us += interval.duration_us();
        }

        None
    }

    /// Applies a non-destructive ripple cut `[cut_start_us, cut_end_us)`.
    /// Excludes the range and ripples all subsequent material.
    pub fn apply_cut(&mut self, cut_start_us: u64, cut_end_us: u64) {
        let mut new_intervals = Vec::new();
        for interval in &self.intervals {
            let parts = interval.exclude_range(cut_start_us, cut_end_us);
            new_intervals.extend(parts);
        }
        new_intervals.sort_by_key(|i| i.start_us);
        self.intervals = new_intervals;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeline_mapping_and_ripple_cuts() {
        // Initial single 10-second interval: [0, 10_000_000)
        let mut mapper = TimelineMapper::new(vec![SourceInterval::new(
            "int-1".into(),
            0,
            10_000_000,
        )]);

        assert_eq!(mapper.total_edited_duration_us(), 10_000_000);
        assert_eq!(mapper.edited_to_source_us(4_000_000), Some(4_000_000));

        // Cut [2s, 5s) (3 seconds cut out)
        mapper.apply_cut(2_000_000, 5_000_000);

        // Retained intervals are now [0, 2s) and [5s, 10s)
        // Total duration is 2s + 5s = 7s (7_000_000 us)
        assert_eq!(mapper.total_edited_duration_us(), 7_000_000);

        // Edited time 1s maps to source 1s
        assert_eq!(mapper.edited_to_source_us(1_000_000), Some(1_000_000));

        // Edited time 2.5s falls in the second interval: 2s + (2.5s - 2s) = 5.5s in source time!
        assert_eq!(mapper.edited_to_source_us(2_500_000), Some(5_500_000));

        // Cut region (e.g. source 3s) is excluded
        assert_eq!(mapper.source_to_edited_us(3_000_000), None);
    }
}
