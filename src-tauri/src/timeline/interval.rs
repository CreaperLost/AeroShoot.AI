use serde::{Deserialize, Serialize};

/// Non-destructive half-open retained source interval `[start_us, end_us)`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInterval {
    pub id: String,
    pub start_us: u64,
    pub end_us: u64,
}

impl SourceInterval {
    pub fn new(id: String, start_us: u64, end_us: u64) -> Self {
        assert!(start_us <= end_us, "start_us must be <= end_us");
        Self {
            id,
            start_us,
            end_us,
        }
    }

    pub fn duration_us(&self) -> u64 {
        self.end_us.saturating_sub(self.start_us)
    }

    pub fn contains_source_us(&self, source_us: u64) -> bool {
        source_us >= self.start_us && source_us < self.end_us
    }

    /// Excludes a given cut interval `[cut_start, cut_end)` from this interval.
    /// Returns 0, 1, or 2 resulting sub-intervals.
    pub fn exclude_range(&self, cut_start: u64, cut_end: u64) -> Vec<SourceInterval> {
        if cut_start >= self.end_us || cut_end <= self.start_us {
            // Cut does not overlap this interval at all
            return vec![self.clone()];
        }

        let mut results = Vec::new();

        // Leading portion before the cut
        if cut_start > self.start_us {
            results.push(SourceInterval::new(
                format!("{}-a", self.id),
                self.start_us,
                cut_start.min(self.end_us),
            ));
        }

        // Trailing portion after the cut
        if cut_end < self.end_us {
            results.push(SourceInterval::new(
                format!("{}-b", self.id),
                cut_end.max(self.start_us),
                self.end_us,
            ));
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interval_exclusion() {
        let interval = SourceInterval::new("int-1".into(), 1_000_000, 10_000_000);

        // Middle cut: cuts [3s, 5s), leaving [1s, 3s) and [5s, 10s)
        let parts = interval.exclude_range(3_000_000, 5_000_000);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].start_us, 1_000_000);
        assert_eq!(parts[0].end_us, 3_000_000);
        assert_eq!(parts[1].start_us, 5_000_000);
        assert_eq!(parts[1].end_us, 10_000_000);

        // Entire interval cut: cuts [0s, 15s)
        let empty = interval.exclude_range(0, 15_000_000);
        assert!(empty.is_empty());
    }
}
