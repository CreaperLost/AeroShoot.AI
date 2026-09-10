use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JournalRecord {
    SegmentCommitted {
        seq: u64,
        track_id: String,
        relative_path: String,
        start_us: u64,
        end_us: u64,
        size_bytes: u64,
        is_keyframe_start: bool,
        /// Native media-clock timescale (e.g. 90_000 for H.264) parsed from
        /// the segment's `mdhd` box. Zero when the segment is not fMP4.
        #[serde(default)]
        media_timescale: u32,
        /// Media-clock value at which the segment starts. Signed because
        /// `elst` edit lists can carry a non-zero media_time offset. Zero
        /// when the segment is not fMP4.
        #[serde(default)]
        media_start_value: i64,
        /// Host-clock anchor (epoch microseconds) at which the segment's
        /// media start_value was published. Lets the editor re-establish
        /// the same media → host mapping on replay.
        #[serde(default)]
        host_anchor_us: i64,
    },
    Discontinuity {
        seq: u64,
        track_id: String,
        t_us: u64,
        reason: String,
    },
    PauseStarted {
        seq: u64,
        t_us: u64,
    },
    PauseEnded {
        seq: u64,
        start_us: u64,
        end_us: u64,
    },
    UnindexedSegmentRecovered {
        seq: u64,
        track_id: String,
        relative_path: String,
        start_us: u64,
        end_us: u64,
        size_bytes: u64,
        /// Same timescale metadata as `SegmentCommitted` when known.
        #[serde(default)]
        media_timescale: u32,
        #[serde(default)]
        media_start_value: i64,
        #[serde(default)]
        host_anchor_us: i64,
    },
    Checkpoint {
        seq: u64,
        t_us: u64,
        total_duration_us: u64,
    },
    RuntimeError {
        seq: u64,
        track_id: String,
        error_code: i32,
        message: String,
        t_us: u64,
        recoverable: bool,
    },
}

impl JournalRecord {
    pub fn seq(&self) -> u64 {
        match self {
            JournalRecord::SegmentCommitted { seq, .. } => *seq,
            JournalRecord::Discontinuity { seq, .. } => *seq,
            JournalRecord::PauseStarted { seq, .. } => *seq,
            JournalRecord::PauseEnded { seq, .. } => *seq,
            JournalRecord::UnindexedSegmentRecovered { seq, .. } => *seq,
            JournalRecord::Checkpoint { seq, .. } => *seq,
            JournalRecord::RuntimeError { seq, .. } => *seq,
        }
    }

    pub fn set_seq(&mut self, new_seq: u64) {
        match self {
            JournalRecord::SegmentCommitted { seq, .. } => *seq = new_seq,
            JournalRecord::Discontinuity { seq, .. } => *seq = new_seq,
            JournalRecord::PauseStarted { seq, .. } => *seq = new_seq,
            JournalRecord::PauseEnded { seq, .. } => *seq = new_seq,
            JournalRecord::UnindexedSegmentRecovered { seq, .. } => *seq = new_seq,
            JournalRecord::Checkpoint { seq, .. } => *seq = new_seq,
            JournalRecord::RuntimeError { seq, .. } => *seq = new_seq,
        }
    }
}

#[derive(Error, Debug)]
pub enum JournalError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Corrupt journal: unparseable line {0}: {1}")]
    CorruptLine(usize, String),
    #[error("injected failure: {0}")]
    Injected(&'static str),
}

/// Durable, append-only journal writer managing journal.jsonl with crash tail repair
pub struct ProjectJournal {
    path: PathBuf,
    file: Mutex<File>,
    next_seq: Mutex<u64>,
    fail_next_appends: Mutex<u32>,
}

impl ProjectJournal {
    pub fn open_or_create<P: AsRef<Path>>(project_dir: P) -> Result<Self, JournalError> {
        let path = project_dir.as_ref().join("journal.jsonl");

        // Repair truncated tail if file exists and does not end with a newline
        if path.exists() {
            Self::repair_truncated_tail(&path)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)?;

        let existing_records = Self::read_records_from_path(&path)?;
        let next_seq = existing_records.last().map(|r| r.seq() + 1).unwrap_or(0);

        Ok(Self {
            path,
            file: Mutex::new(file),
            next_seq: Mutex::new(next_seq),
            fail_next_appends: Mutex::new(0),
        })
    }

    /// Contract-test hook: the next `n` `append` calls fail without writing.
    /// Does not consume sequence numbers. Production callers leave this at 0.
    pub fn inject_fail_next_appends(&self, n: u32) {
        *self.fail_next_appends.lock().unwrap() = n;
    }

    /// Repairs an incomplete final line left by sudden termination/crash.
    /// If the file does not end with `\n`, truncates back to the last newline boundary.
    pub fn repair_truncated_tail(path: &Path) -> Result<(), std::io::Error> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        if bytes.is_empty() {
            return Ok(());
        }

        if !bytes.ends_with(b"\n") {
            // Find last newline
            let last_newline_pos = bytes.iter().rposition(|&b| b == b'\n');
            match last_newline_pos {
                Some(pos) => {
                    file.set_len((pos + 1) as u64)?;
                }
                None => {
                    // No newline anywhere; entire first line was incomplete
                    file.set_len(0)?;
                }
            }
            file.sync_data()?;
        }

        Ok(())
    }

    pub fn append(&self, mut record: JournalRecord) -> Result<u64, JournalError> {
        {
            let mut remaining = self.fail_next_appends.lock().unwrap();
            if *remaining > 0 {
                *remaining -= 1;
                return Err(JournalError::Injected("journal append"));
            }
        }
        let mut seq_guard = self.next_seq.lock().unwrap();
        let current_seq = *seq_guard;

        record.set_seq(current_seq);

        let serialized = serde_json::to_string(&record)?;
        let mut file = self.file.lock().unwrap();
        writeln!(file, "{}", serialized)?;
        file.sync_data()?;

        *seq_guard += 1;
        Ok(current_seq)
    }

    pub fn read_all(&self) -> Result<Vec<JournalRecord>, JournalError> {
        Self::read_records_from_path(&self.path)
    }

    pub fn read_records_from_path<P: AsRef<Path>>(
        path: P,
    ) -> Result<Vec<JournalRecord>, JournalError> {
        let p = path.as_ref();
        if !p.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(p)?;
        let reader = BufReader::new(file);
        let mut records = Vec::new();

        for (line_idx, line) in reader.lines().enumerate() {
            let line_str = line?;
            let trimmed = line_str.trim();
            if trimmed.is_empty() {
                continue;
            }

            match serde_json::from_str::<JournalRecord>(trimmed) {
                Ok(record) => records.push(record),
                Err(err) => {
                    return Err(JournalError::CorruptLine(line_idx + 1, err.to_string()));
                }
            }
        }

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_journal_durability_and_sequence() {
        let dir = tempdir().unwrap();
        let journal = ProjectJournal::open_or_create(dir.path()).unwrap();

        let s1 = journal
            .append(JournalRecord::SegmentCommitted {
                seq: 0,
                track_id: "screen".into(),
                relative_path: "media/screen/000001.mp4".into(),
                start_us: 0,
                end_us: 2_000_000,
                size_bytes: 4096,
                is_keyframe_start: true,
                media_timescale: 90_000,
                media_start_value: 0,
                host_anchor_us: 0,
            })
            .unwrap();

        let s2 = journal
            .append(JournalRecord::PauseStarted {
                seq: 0,
                t_us: 2_000_000,
            })
            .unwrap();

        assert_eq!(s1, 0);
        assert_eq!(s2, 1);

        let records = journal.read_all().unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn test_truncated_tail_repair() {
        let dir = tempdir().unwrap();
        let journal_path = dir.path().join("journal.jsonl");

        // Write a valid record followed by a truncated partial record without newline
        let valid_record = r#"{"type":"pause_started","seq":0,"t_us":1000}"#;
        let truncated_tail = r#"{"type":"segment_committed","seq":1,"track_id":"scre"#;
        std::fs::write(
            &journal_path,
            format!("{}\n{}", valid_record, truncated_tail),
        )
        .unwrap();

        // Reopening journal should repair the tail
        let journal = ProjectJournal::open_or_create(dir.path()).unwrap();

        // Now append a new record
        let s2 = journal
            .append(JournalRecord::PauseEnded {
                seq: 0,
                start_us: 1000,
                end_us: 3000,
            })
            .unwrap();

        assert_eq!(
            s2, 1,
            "Repaired journal should sequence after valid records"
        );

        // Reading all should return exactly the 2 valid records without corrupt line errors
        let records = journal.read_all().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].seq(), 0);
        assert_eq!(records[1].seq(), 1);
    }
}
