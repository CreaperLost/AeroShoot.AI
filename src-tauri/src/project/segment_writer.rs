use crate::project::journal::{JournalError, JournalRecord, ProjectJournal};
use crate::project::manifest::TrackType;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum SegmentWriterError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Journal error: {0}")]
    Journal(String),
    #[error("No active segment in progress")]
    NoActiveSegment,
    #[error("Destination segment already exists: {0:?}")]
    DestinationAlreadyExists(PathBuf),
}

impl From<io::Error> for SegmentWriterError {
    fn from(e: io::Error) -> Self {
        SegmentWriterError::Io(e.to_string())
    }
}

impl From<JournalError> for SegmentWriterError {
    fn from(e: JournalError) -> Self {
        SegmentWriterError::Journal(e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SegmentCommitResult {
    pub seq: u64,
    pub relative_path: String,
    pub start_us: u64,
    pub end_us: u64,
    pub size_bytes: u64,
}

/// Implements Phase 1 segment-writing pipeline:
/// finish buffer -> durable flush (sync_data) -> atomic rename -> journal commit.
pub struct TrackSegmentWriter {
    project_dir: PathBuf,
    track_id: String,
    track_type: TrackType,
    codec: String,
    extension: String,
    target_duration_us: u64,
    current_seq: u64,
    active_temp_file: Option<File>,
    active_temp_path: Option<PathBuf>,
    active_start_us: u64,
    active_bytes: u64,
}

impl TrackSegmentWriter {
    pub fn new<P: AsRef<Path>>(
        project_dir: P,
        track_id: String,
        track_type: TrackType,
        codec: String,
    ) -> Self {
        let extension = match track_type {
            TrackType::Screen | TrackType::Webcam => "mp4".to_string(),
            TrackType::SystemAudio | TrackType::MicAudio => "wav".to_string(),
        };

        // Scan existing track directory to allocate the next unused sequence ID
        let track_dir = project_dir.as_ref().join("media").join(&track_id);
        let mut max_seq = 0u64;
        if let Ok(entries) = fs::read_dir(&track_dir) {
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if let Some(stem) = file_name.split('.').next() {
                    if stem.len() == 6 {
                        if let Ok(seq) = stem.parse::<u64>() {
                            if seq > max_seq {
                                max_seq = seq;
                            }
                        }
                    }
                }
            }
        }
        let current_seq = max_seq + 1;

        Self {
            project_dir: project_dir.as_ref().to_path_buf(),
            track_id,
            track_type,
            codec,
            extension,
            target_duration_us: 2_000_000, // 2-second target segments
            current_seq,
            active_temp_file: None,
            active_temp_path: None,
            active_start_us: 0,
            active_bytes: 0,
        }
    }

    pub fn track_id(&self) -> &str {
        &self.track_id
    }

    pub fn track_type(&self) -> TrackType {
        self.track_type
    }

    pub fn codec(&self) -> &str {
        &self.codec
    }

    pub fn current_seq(&self) -> u64 {
        self.current_seq
    }

    /// Opens a new temporary segment file within the project directory tree.
    /// Exclusively creates temporary file and ensures current_seq is strictly unused.
    pub fn begin_segment(&mut self, start_us: u64) -> Result<PathBuf, SegmentWriterError> {
        let track_dir = self
            .project_dir
            .join("media")
            .join(&self.track_id);
        fs::create_dir_all(&track_dir)?;

        // Find unused sequence ID
        loop {
            let committed_filename = format!("{:06}.{}", self.current_seq, self.extension);
            let committed_path = track_dir.join(&committed_filename);
            let temp_filename = format!("{:06}.tmp", self.current_seq);
            let temp_path = track_dir.join(&temp_filename);
            if committed_path.exists() || temp_path.exists() {
                self.current_seq += 1;
            } else {
                break;
            }
        }

        let temp_filename = format!("{:06}.tmp", self.current_seq);
        let temp_path = track_dir.join(&temp_filename);

        // Symlink safety check: if temp_path is a symlink, safely remove it
        if let Ok(meta) = fs::symlink_metadata(&temp_path) {
            if meta.file_type().is_symlink() {
                fs::remove_file(&temp_path).map_err(|e| SegmentWriterError::Io(e.to_string()))?;
            }
        }

        // Open with exclusive creation (O_CREAT | O_EXCL) to prevent following symlinks
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;

        self.active_temp_file = Some(file);
        self.active_temp_path = Some(temp_path.clone());
        self.active_start_us = start_us;
        self.active_bytes = 0;

        Ok(temp_path)
    }

    /// Writes raw media data to the active temporary segment file.
    pub fn write_data(&mut self, data: &[u8]) -> Result<(), SegmentWriterError> {
        let file = self
            .active_temp_file
            .as_mut()
            .ok_or(SegmentWriterError::NoActiveSegment)?;

        file.write_all(data)?;
        self.active_bytes += data.len() as u64;
        Ok(())
    }

    /// Commits the active segment adhering strictly to the durability contract:
    /// 1. Finish & durable flush (`sync_data`)
    /// 2. Close file handle
    /// 3. Atomically rename within project filesystem (rejecting destination collisions)
    /// 4. Durably append to `journal.jsonl`
    pub fn commit_segment(
        &mut self,
        end_us: u64,
        is_keyframe_start: bool,
        journal: &ProjectJournal,
    ) -> Result<SegmentCommitResult, SegmentWriterError> {
        let mut file = self
            .active_temp_file
            .take()
            .ok_or(SegmentWriterError::NoActiveSegment)?;
        let temp_path = self
            .active_temp_path
            .take()
            .ok_or(SegmentWriterError::NoActiveSegment)?;

        // Step 1: Durable flush
        file.flush()?;
        file.sync_data()?;
        drop(file); // Ensure handle is closed before rename

        // Step 2: Destination collision check and atomic rename
        let committed_filename = format!("{:06}.{}", self.current_seq, self.extension);
        let committed_path = self
            .project_dir
            .join("media")
            .join(&self.track_id)
            .join(&committed_filename);

        if committed_path.exists() || fs::symlink_metadata(&committed_path).is_ok() {
            return Err(SegmentWriterError::DestinationAlreadyExists(committed_path));
        }

        fs::rename(&temp_path, &committed_path)?;

        // Relative path within project bundle
        let relative_path = format!("media/{}/{}", self.track_id, committed_filename);
        let size_bytes = self.active_bytes;

        // Step 3: Durable journal commit
        journal.append(JournalRecord::SegmentCommitted {
            seq: self.current_seq,
            track_id: self.track_id.clone(),
            relative_path: relative_path.clone(),
            start_us: self.active_start_us,
            end_us,
            size_bytes,
            is_keyframe_start,
        })?;

        let result = SegmentCommitResult {
            seq: self.current_seq,
            relative_path,
            start_us: self.active_start_us,
            end_us,
            size_bytes,
        };

        self.current_seq += 1;
        self.active_bytes = 0;

        Ok(result)
    }

    /// Finalizes any active in-flight segment upon session stop or pause.
    pub fn finalize(
        &mut self,
        final_us: u64,
        journal: &ProjectJournal,
    ) -> Result<Option<SegmentCommitResult>, SegmentWriterError> {
        if self.active_temp_file.is_some() {
            let res = self.commit_segment(final_us, false, journal)?;
            Ok(Some(res))
        } else {
            // Clean up empty temp file if any
            if let Some(path) = self.active_temp_path.take() {
                let _ = fs::remove_file(path);
            }
            self.active_temp_file = None;
            Ok(None)
        }
    }

    pub fn has_active_segment(&self) -> bool {
        self.active_temp_file.is_some()
    }

    pub fn target_duration_us(&self) -> u64 {
        self.target_duration_us
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_segment_writer_commit_order() {
        let dir = tempdir().unwrap();
        let journal = ProjectJournal::open_or_create(dir.path()).unwrap();

        let mut writer = TrackSegmentWriter::new(
            dir.path(),
            "screen".into(),
            TrackType::Screen,
            "h264".into(),
        );

        // 1. Begin segment
        let temp_path = writer.begin_segment(0).unwrap();
        assert!(temp_path.exists());
        assert!(temp_path.to_string_lossy().ends_with("000001.tmp"));

        // 2. Write dummy fMP4 data
        let mut ftyp = Vec::new();
        ftyp.extend_from_slice(&32u32.to_be_bytes());
        ftyp.extend_from_slice(b"ftyp");
        ftyp.extend_from_slice(b"isom");
        ftyp.extend_from_slice(&0x0200u32.to_be_bytes());
        ftyp.extend_from_slice(b"isomiso2avc1mp41");
        writer.write_data(&ftyp).unwrap();

        // 3. Commit segment
        let commit = writer.commit_segment(2_000_000, true, &journal).unwrap();
        assert_eq!(commit.seq, 1);
        assert_eq!(commit.relative_path, "media/screen/000001.mp4");
        assert_eq!(commit.size_bytes, 32);

        // Verify temp file is gone and committed file exists
        assert!(!temp_path.exists());
        let committed_path = dir.path().join("media/screen/000001.mp4");
        assert!(committed_path.exists());

        // Verify journal entry
        let records = journal.read_all().unwrap();
        assert_eq!(records.len(), 1);
        match &records[0] {
            JournalRecord::SegmentCommitted {
                track_id,
                relative_path,
                start_us,
                end_us,
                size_bytes,
                ..
            } => {
                assert_eq!(track_id, "screen");
                assert_eq!(relative_path, "media/screen/000001.mp4");
                assert_eq!(*start_us, 0);
                assert_eq!(*end_us, 2_000_000);
                assert_eq!(*size_bytes, 32);
            }
            _ => panic!("Expected SegmentCommitted record"),
        }
    }
}
