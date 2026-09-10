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
        Self::try_new(intervals).unwrap_or_else(|_| Self {
            intervals: Vec::new(),
        })
    }

    pub fn try_new(mut intervals: Vec<SourceInterval>) -> Result<Self, String> {
        intervals.retain(|i| i.end_us > i.start_us);
        intervals.sort_by_key(|i| i.start_us);
        for pair in intervals.windows(2) {
            if pair[1].start_us < pair[0].end_us {
                return Err("Retained intervals must be non-overlapping".into());
            }
        }
        Ok(Self { intervals })
    }

    pub fn intervals(&self) -> &[SourceInterval] {
        &self.intervals
    }

    /// Total edited timeline duration across all retained intervals.
    pub fn total_edited_duration_us(&self) -> u64 {
        self.intervals.iter().map(|i| i.duration_us()).sum()
    }

    /// Maps an edited timeline position to source time for a decodable sample.
    /// The exclusive edited duration is not a sample and returns `None`.
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

        None
    }

    /// Playhead may sit on the exclusive edited end; that must not be decoded.
    pub fn playhead_source_us(&self, edited_us: u64) -> Option<u64> {
        self.edited_to_source_us(edited_us)
    }

    /// Maps a half-open edited range onto source ranges without using the exclusive end as a sample.
    pub fn edited_range_to_source(&self, start_us: u64, end_us: u64) -> Vec<(u64, u64)> {
        let mut ranges = Vec::new();
        let mut edited_cursor = 0u64;
        for interval in &self.intervals {
            let duration = interval.duration_us();
            let interval_end = edited_cursor + duration;
            let a = start_us.max(edited_cursor);
            let b = end_us.min(interval_end);
            if a < b {
                let source_a = interval.start_us + (a - edited_cursor);
                let source_b = interval.start_us + (b - edited_cursor);
                ranges.push((source_a, source_b));
            }
            edited_cursor = interval_end;
        }
        ranges
    }

    /// Ripple-cuts a half-open edited range. One shared interval list applies to every track.
    pub fn ripple_cut_edited(
        &mut self,
        edited_start_us: u64,
        edited_end_us: u64,
    ) -> Result<(), String> {
        if edited_start_us >= edited_end_us {
            return Err("Cut must be a half-open interval".into());
        }
        let duration = self.total_edited_duration_us();
        if edited_end_us > duration {
            return Err("Cut exceeds edited duration".into());
        }
        let source_ranges = self.edited_range_to_source(edited_start_us, edited_end_us);
        for (start, end) in source_ranges {
            self.apply_cut(start, end);
        }
        Ok(())
    }

    /// Keeps only the half-open edited range `[start, end)`.
    pub fn trim_edited(&mut self, edited_start_us: u64, edited_end_us: u64) -> Result<(), String> {
        if edited_start_us >= edited_end_us {
            return Err("Trim must be a half-open interval".into());
        }
        let duration = self.total_edited_duration_us();
        if edited_end_us > duration {
            return Err("Trim exceeds edited duration".into());
        }
        let kept = self.edited_range_to_source(edited_start_us, edited_end_us);
        self.intervals = kept
            .into_iter()
            .enumerate()
            .filter(|(_, (a, b))| b > a)
            .map(|(i, (start_us, end_us))| {
                SourceInterval::new(format!("trim-{i}"), start_us, end_us)
            })
            .collect();
        Ok(())
    }

    /// Maps a half-open source range onto edited ranges. Cuts split the result;
    /// removed source time is omitted rather than interpolated.
    pub fn source_range_to_edited(&self, start_us: u64, end_us: u64) -> Vec<(u64, u64)> {
        let mut ranges = Vec::new();
        let mut edited_cursor = 0u64;
        for interval in &self.intervals {
            let duration = interval.duration_us();
            let a = start_us.max(interval.start_us);
            let b = end_us.min(interval.end_us);
            if a < b {
                let edited_a = edited_cursor + (a - interval.start_us);
                let edited_b = edited_cursor + (b - interval.start_us);
                ranges.push((edited_a, edited_b));
            }
            edited_cursor += duration;
        }
        ranges
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
        let mut mapper =
            TimelineMapper::new(vec![SourceInterval::new("int-1".into(), 0, 10_000_000)]);

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

    #[test]
    fn retained_example_and_exclusive_end_are_not_decoded() {
        let mapper = TimelineMapper::try_new(vec![
            SourceInterval::new("a".into(), 0, 2_000_000),
            SourceInterval::new("b".into(), 5_000_000, 10_000_000),
        ])
        .unwrap();
        assert_eq!(mapper.total_edited_duration_us(), 7_000_000);
        assert_eq!(mapper.edited_to_source_us(2_000_000), Some(5_000_000));
        assert_eq!(mapper.edited_to_source_us(2_500_000), Some(5_500_000));
        assert_eq!(mapper.edited_to_source_us(7_000_000), None);
        assert_eq!(mapper.edited_to_source_us(6_999_999), Some(9_999_999));

        let mut cut = mapper.clone();
        cut.ripple_cut_edited(1_000_000, 2_000_000).unwrap();
        assert_eq!(cut.total_edited_duration_us(), 6_000_000);
        assert_eq!(cut.edited_to_source_us(1_000_000), Some(5_000_000));
        assert_eq!(
            mapper.source_range_to_edited(1_000_000, 6_000_000),
            vec![(1_000_000, 2_000_000), (2_000_000, 3_000_000)]
        );
    }
}
