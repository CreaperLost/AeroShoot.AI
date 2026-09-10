use crate::project::journal::{JournalError, JournalRecord, ProjectJournal};
use crate::project::manifest::TrackType;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// One-shot durability fault for contract tests. Production callers leave this
/// at `None`. Injected failures must not be reported as successful commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DurabilityFault {
    #[default]
    None,
    /// Fail after `flush` / before data is published.
    FailSync,
    /// Fail the journal append. If the file was already published, the writer
    /// retains a pending publication so a retry cannot claim success without
    /// actually journaling.
    FailJournalAppend,
}

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
    #[error("Unjournaled publication pending; retry commit before opening a new segment")]
    PendingPublication,
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

#[derive(Debug, Clone)]
struct PendingPublication {
    seq: u64,
    relative_path: String,
    start_us: u64,
    end_us: u64,
    size_bytes: u64,
    is_keyframe_start: bool,
    media_timescale: u32,
    media_start_value: i64,
    host_anchor_us: i64,
}

/// Rust `TrackSegmentWriter` is owned by the session command thread for the
/// synthetic (non-native) path. Native AVAssetWriter containers are owned by
/// Swift `RotatingMediaWriter` on capture/rotation queues; this type only
/// publishes a finished temporary file from the synchronous C callback.
///
/// Publication contract (both `commit_segment` and `commit_native_segment`):
/// 1. durable flush of the source bytes
/// 2. no-overwrite hard-link into the committed path
/// 3. directory sync
/// 4. journal append
/// A journal failure after step 2/3 leaves `pending_publication` so a retry
/// cannot treat the earlier failure as success or overwrite the file.
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
    pending_publication: Option<PendingPublication>,
    injected_fault: DurabilityFault,
    /// When native PTS is mach-absolute, the first host anchor is boot uptime.
    /// Subsequent native commits subtract this origin so journal times stay
    /// session-relative.
    native_host_origin: Option<u64>,
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
            pending_publication: None,
            injected_fault: DurabilityFault::None,
            native_host_origin: None,
        }
    }

    pub fn inject_fault(&mut self, fault: DurabilityFault) {
        self.injected_fault = fault;
    }

    pub fn has_pending_publication(&self) -> bool {
        self.pending_publication.is_some()
    }

    fn take_fault(&mut self, expected: DurabilityFault) -> bool {
        if self.injected_fault == expected {
            self.injected_fault = DurabilityFault::None;
            true
        } else {
            false
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

    /// Accept a finalized native container. Called from the Swift rotation
    /// thread via the C ABI (not the Tauri command lock). The caller holds the
    /// project lease; native writers surrender this path until publication
    /// has completed. Unlike `commit_segment`, sequence IDs come from Swift's
    /// `index` and there is no in-process File handle to restore on failure.
    ///
    /// The native callback must reuse a long-lived writer: a journal failure
    /// after publish is stashed on `pending_publication` and retried here.
    pub fn commit_native_segment(
        &mut self,
        temp_path: &Path,
        index: u32,
        host_anchor_us: i64,
        timescale: u32,
        media_start_value: i64,
        session_elapsed_us: u64,
        journal: &ProjectJournal,
    ) -> Result<SegmentCommitResult, SegmentWriterError> {
        use crate::project::{manifest::ProjectManifest, media_validator::MediaValidator};
        let invalid = |message: String| SegmentWriterError::Io(message);
        let expected_seq = u64::from(index) + 1;
        if let Some(pending) = self.pending_publication.clone() {
            let pending_seq = pending.seq;
            let result = self.finish_pending_journal(journal, pending)?;
            if pending_seq == expected_seq {
                return Ok(result);
            }
        }
        if host_anchor_us < 0 || timescale == 0 {
            return Err(invalid("Invalid native clock anchor".into()));
        }
        let filename = format!("{:06}.{}", u64::from(index) + 1, self.extension);
        let relative_path = format!("media/{}/{}", self.track_id, filename);
        let temp_relative = format!("{relative_path}.tmp");
        ProjectManifest::validate_path_in_root(&self.project_dir, &temp_relative)
            .map_err(|e| invalid(e.to_string()))?;
        ProjectManifest::validate_path_in_root(&self.project_dir, &relative_path)
            .map_err(|e| invalid(e.to_string()))?;
        let expected = self.project_dir.join(&temp_relative);
        let same_file = temp_path == expected
            || match (fs::canonicalize(temp_path), fs::canonicalize(&expected)) {
                (Ok(left), Ok(right)) => left == right,
                _ => false,
            };
        if !same_file || fs::symlink_metadata(temp_path)?.file_type().is_symlink() {
            return Err(invalid("Unexpected native segment path".into()));
        }
        let info = MediaValidator::validate(temp_path, self.track_type)
            .map_err(|e| invalid(e.to_string()))?;
        if info.duration_us == 0 || !info.is_keyframe_start {
            return Err(invalid(
                "Native segment has no duration or independent start".into(),
            ));
        }
        if self.take_fault(DurabilityFault::FailSync) {
            return Err(invalid("injected sync failure".into()));
        }
        let start_us = self.session_start_us(host_anchor_us, session_elapsed_us)?;
        let end_us = start_us
            .checked_add(info.duration_us)
            .ok_or_else(|| invalid("Native segment time overflow".into()))?;
        File::open(temp_path)?.sync_all()?;
        let destination = self.project_dir.join(&relative_path);
        publish_no_overwrite(temp_path, &destination)?;
        let pending = PendingPublication {
            seq: u64::from(index) + 1,
            relative_path,
            start_us,
            end_us,
            size_bytes: info.size_bytes,
            is_keyframe_start: info.is_keyframe_start,
            media_timescale: timescale,
            media_start_value,
            host_anchor_us: start_us as i64,
        };
        self.finish_pending_journal(journal, pending)
    }

    /// Journal times are session-relative. ScreenCaptureKit PTS is often
    /// mach-absolute boot uptime; if that value is far ahead of the session
    /// clock, rebase this writer onto the first such anchor.
    fn session_start_us(
        &mut self,
        host_anchor_us: i64,
        session_elapsed_us: u64,
    ) -> Result<u64, SegmentWriterError> {
        if host_anchor_us < 0 {
            return Err(SegmentWriterError::Io("Invalid native clock anchor".into()));
        }
        let host = host_anchor_us as u64;
        if let Some(origin) = self.native_host_origin {
            return Ok(host.saturating_sub(origin));
        }
        const SLACK_US: u64 = 5_000_000;
        const ABSOLUTE_FLOOR_US: u64 = 30_000_000;
        if host > session_elapsed_us.saturating_add(SLACK_US) && host > ABSOLUTE_FLOOR_US {
            self.native_host_origin = Some(host);
            return Ok(0);
        }
        Ok(host)
    }

    fn finish_pending_journal(
        &mut self,
        journal: &ProjectJournal,
        pending: PendingPublication,
    ) -> Result<SegmentCommitResult, SegmentWriterError> {
        // The file is already published. Stash before any journal attempt so a
        // real append error (not only DurabilityFault::FailJournalAppend) still
        // leaves a retryable pending publication.
        self.pending_publication = Some(pending.clone());
        if self.take_fault(DurabilityFault::FailJournalAppend) {
            return Err(SegmentWriterError::Journal(
                "injected journal append failure".into(),
            ));
        }
        if let Err(error) = journal.append(JournalRecord::SegmentCommitted {
            seq: pending.seq,
            track_id: self.track_id.clone(),
            relative_path: pending.relative_path.clone(),
            start_us: pending.start_us,
            end_us: pending.end_us,
            size_bytes: pending.size_bytes,
            is_keyframe_start: pending.is_keyframe_start,
            media_timescale: pending.media_timescale,
            media_start_value: pending.media_start_value,
            host_anchor_us: pending.host_anchor_us,
        }) {
            return Err(error.into());
        }
        self.pending_publication = None;
        if pending.seq >= self.current_seq {
            self.current_seq = pending.seq + 1;
        }
        Ok(SegmentCommitResult {
            seq: pending.seq,
            relative_path: pending.relative_path,
            start_us: pending.start_us,
            end_us: pending.end_us,
            size_bytes: pending.size_bytes,
        })
    }

    /// Opens a new temporary segment file within the project directory tree.
    /// Exclusively creates temporary file and ensures current_seq is strictly unused.
    pub fn begin_segment(&mut self, start_us: u64) -> Result<PathBuf, SegmentWriterError> {
        if self.pending_publication.is_some() {
            return Err(SegmentWriterError::PendingPublication);
        }
        let track_dir = self.project_dir.join("media").join(&self.track_id);
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

    /// Commits the active segment. Publication uses the same no-overwrite
    /// hard-link as `commit_native_segment`; a rename that replaces an
    /// existing destination is not used. Failures before publication restore
    /// the in-process file handle so a retry still owns the bytes.
    pub fn commit_segment(
        &mut self,
        end_us: u64,
        is_keyframe_start: bool,
        journal: &ProjectJournal,
    ) -> Result<SegmentCommitResult, SegmentWriterError> {
        if let Some(pending) = self.pending_publication.clone() {
            return self.finish_pending_journal(journal, pending);
        }
        {
            let file = self
                .active_temp_file
                .as_mut()
                .ok_or(SegmentWriterError::NoActiveSegment)?;
            file.flush()?;
            file.sync_data()?;
        }
        if self.take_fault(DurabilityFault::FailSync) {
            return Err(SegmentWriterError::Io("injected sync failure".into()));
        }
        let temp_path = self
            .active_temp_path
            .clone()
            .ok_or(SegmentWriterError::NoActiveSegment)?;

        let committed_filename = format!("{:06}.{}", self.current_seq, self.extension);
        let committed_path = self
            .project_dir
            .join("media")
            .join(&self.track_id)
            .join(&committed_filename);
        let relative_path = format!("media/{}/{}", self.track_id, committed_filename);
        let size_bytes = self.active_bytes;
        let start_us = self.active_start_us;
        let seq = self.current_seq;

        drop(self.active_temp_file.take());
        match publish_no_overwrite(&temp_path, &committed_path) {
            Ok(()) => {
                self.active_temp_path = None;
                self.active_bytes = 0;
            }
            Err(error) => {
                self.active_temp_file = OpenOptions::new()
                    .write(true)
                    .append(true)
                    .open(&temp_path)
                    .ok();
                self.active_temp_path = Some(temp_path);
                return Err(error);
            }
        }

        let pending = PendingPublication {
            seq,
            relative_path,
            start_us,
            end_us,
            size_bytes,
            is_keyframe_start,
            media_timescale: 0,
            media_start_value: 0,
            host_anchor_us: 0,
        };
        self.finish_pending_journal(journal, pending)
    }

    /// Finalizes any active in-flight segment upon session stop or pause.
    /// An unjournaled publication is retried before treating the writer as idle;
    /// `Ok(None)` is only returned when there is truly nothing left to commit.
    pub fn finalize(
        &mut self,
        final_us: u64,
        journal: &ProjectJournal,
    ) -> Result<Option<SegmentCommitResult>, SegmentWriterError> {
        if self.pending_publication.is_some() || self.active_temp_file.is_some() {
            let res = self.commit_segment(final_us, false, journal)?;
            Ok(Some(res))
        } else {
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

fn publish_no_overwrite(temp_path: &Path, destination: &Path) -> Result<(), SegmentWriterError> {
    // Same-filesystem hard-link publication is atomic and never replaces
    // an existing destination, including a dangling symlink. `commit_segment`
    // previously used `rename`, which overwrites; native publication never did.
    fs::hard_link(temp_path, destination).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            SegmentWriterError::DestinationAlreadyExists(destination.to_path_buf())
        } else {
            e.into()
        }
    })?;
    let _ = fs::remove_file(temp_path);
    #[cfg(unix)]
    if let Some(parent) = destination.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn native_commit_publishes_and_preserves_clock_and_collision() {
        let dir = tempdir().unwrap();
        let journal = ProjectJournal::open_or_create(dir.path()).unwrap();
        let mut writer =
            TrackSegmentWriter::new(dir.path(), "mic".into(), TrackType::MicAudio, "pcm".into());
        fs::create_dir_all(dir.path().join("media/mic")).unwrap();
        let temp = dir.path().join("media/mic/000001.wav.tmp");
        let data = crate::fixtures::generate_valid_wav_segment(100_000, 48_000, 1);
        fs::write(&temp, &data).unwrap();
        let result = writer
            .commit_native_segment(&temp, 0, 750_000, 48_000, 36_000, 750_000, &journal)
            .unwrap();
        assert_eq!((result.start_us, result.end_us), (750_000, 850_000));
        assert!(!temp.exists());
        let records = journal.read_all().unwrap();
        assert!(matches!(
            &records[0],
            JournalRecord::SegmentCommitted {
                media_timescale: 48_000,
                media_start_value: 36_000,
                host_anchor_us: 750_000,
                ..
            }
        ));
        fs::write(&temp, &data).unwrap();
        assert!(matches!(
            writer.commit_native_segment(&temp, 0, 0, 48_000, 0, 0, &journal),
            Err(SegmentWriterError::DestinationAlreadyExists(_))
        ));
        assert_eq!(
            fs::read(dir.path().join(&result.relative_path)).unwrap(),
            data
        );
        assert_eq!(journal.read_all().unwrap().len(), 1);
    }

    #[test]
    fn native_commit_rejects_invalid_media_and_paths() {
        let dir = tempdir().unwrap();
        let journal = ProjectJournal::open_or_create(dir.path()).unwrap();
        let mut writer =
            TrackSegmentWriter::new(dir.path(), "mic".into(), TrackType::MicAudio, "pcm".into());
        fs::create_dir_all(dir.path().join("media/mic")).unwrap();
        let temp = dir.path().join("media/mic/000001.wav.tmp");
        fs::write(&temp, b"invalid media").unwrap();
        assert!(writer
            .commit_native_segment(&temp, 0, 0, 48_000, 0, 0, &journal)
            .is_err());
        assert!(writer
            .commit_native_segment(&temp, 1, 0, 48_000, 0, 0, &journal)
            .is_err());
        assert!(temp.exists());
        assert!(journal.read_all().unwrap().is_empty());
        #[cfg(unix)]
        {
            let external = tempdir().unwrap();
            let source = external.path().join("source.wav");
            fs::write(
                &source,
                crate::fixtures::generate_valid_wav_segment(100_000, 48_000, 1),
            )
            .unwrap();
            fs::remove_file(&temp).unwrap();
            std::os::unix::fs::symlink(&source, &temp).unwrap();
            assert!(writer
                .commit_native_segment(&temp, 0, 0, 48_000, 0, 0, &journal)
                .is_err());
            assert!(source.exists());
            assert!(journal.read_all().unwrap().is_empty());
        }
    }

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

    #[test]
    fn injected_sync_failure_keeps_ownership_and_prior_commit() {
        let dir = tempdir().unwrap();
        let journal = ProjectJournal::open_or_create(dir.path()).unwrap();
        let mut writer = TrackSegmentWriter::new(
            dir.path(),
            "screen".into(),
            TrackType::Screen,
            "h264".into(),
        );
        writer.begin_segment(0).unwrap();
        writer.write_data(b"FIRST").unwrap();
        let first = writer.commit_segment(1_000_000, true, &journal).unwrap();
        let first_bytes = fs::read(dir.path().join(&first.relative_path)).unwrap();

        writer.begin_segment(1_000_000).unwrap();
        writer.write_data(b"SECOND").unwrap();
        writer.inject_fault(DurabilityFault::FailSync);
        assert!(writer.commit_segment(2_000_000, true, &journal).is_err());
        assert!(writer.has_active_segment());
        assert!(!writer.has_pending_publication());
        assert_eq!(journal.read_all().unwrap().len(), 1);
        assert_eq!(
            fs::read(dir.path().join(&first.relative_path)).unwrap(),
            first_bytes
        );
        assert!(!dir.path().join("media/screen/000002.mp4").exists());

        writer.inject_fault(DurabilityFault::None);
        let second = writer.commit_segment(2_000_000, true, &journal).unwrap();
        assert_eq!(second.relative_path, "media/screen/000002.mp4");
        assert_eq!(journal.read_all().unwrap().len(), 2);
    }

    #[test]
    fn injected_journal_failure_does_not_become_success_on_retry() {
        let dir = tempdir().unwrap();
        let journal = ProjectJournal::open_or_create(dir.path()).unwrap();
        let mut writer =
            TrackSegmentWriter::new(dir.path(), "mic".into(), TrackType::MicAudio, "pcm".into());
        fs::create_dir_all(dir.path().join("media/mic")).unwrap();
        let temp = dir.path().join("media/mic/000001.wav.tmp");
        let data = crate::fixtures::generate_valid_wav_segment(100_000, 48_000, 1);
        fs::write(&temp, &data).unwrap();
        writer.inject_fault(DurabilityFault::FailJournalAppend);
        assert!(writer
            .commit_native_segment(&temp, 0, 0, 48_000, 0, 0, &journal)
            .is_err());
        assert!(writer.has_pending_publication());
        assert!(dir.path().join("media/mic/000001.wav").exists());
        assert!(journal.read_all().unwrap().is_empty());
        assert!(matches!(
            writer.begin_segment(0),
            Err(SegmentWriterError::PendingPublication)
        ));

        writer.inject_fault(DurabilityFault::FailJournalAppend);
        assert!(writer.finalize(100_000, &journal).is_err());
        assert!(writer.has_pending_publication());
        assert!(journal.read_all().unwrap().is_empty());

        writer.inject_fault(DurabilityFault::None);
        let committed = writer.finalize(100_000, &journal).unwrap().unwrap();
        assert_eq!(committed.relative_path, "media/mic/000001.wav");
        assert_eq!(journal.read_all().unwrap().len(), 1);
        assert!(!writer.has_pending_publication());
        assert_eq!(fs::read(dir.path().join(&committed.relative_path)).unwrap(), data);
    }

    #[test]
    fn actual_journal_append_failure_keeps_pending_for_retry() {
        let dir = tempdir().unwrap();
        let journal = ProjectJournal::open_or_create(dir.path()).unwrap();
        let mut writer = TrackSegmentWriter::new(
            dir.path(),
            "screen".into(),
            TrackType::Screen,
            "h264".into(),
        );
        writer.begin_segment(0).unwrap();
        writer.write_data(b"PENDING").unwrap();
        journal.inject_fail_next_appends(1);

        assert!(writer.commit_segment(1_000_000, true, &journal).is_err());
        assert!(
            writer.has_pending_publication(),
            "a real journal.append error must stash pending publication"
        );
        assert!(!writer.has_active_segment());
        assert!(dir.path().join("media/screen/000001.mp4").exists());
        assert!(journal.read_all().unwrap().is_empty());
        assert!(matches!(
            writer.begin_segment(1_000_000),
            Err(SegmentWriterError::PendingPublication)
        ));

        journal.inject_fail_next_appends(1);
        assert!(writer.finalize(1_000_000, &journal).is_err());
        assert!(writer.has_pending_publication());
        assert!(journal.read_all().unwrap().is_empty());

        let committed = writer.finalize(1_000_000, &journal).unwrap().unwrap();
        assert_eq!(committed.relative_path, "media/screen/000001.mp4");
        assert_eq!(journal.read_all().unwrap().len(), 1);
        assert!(!writer.has_pending_publication());
        assert_eq!(
            fs::read(dir.path().join(&committed.relative_path)).unwrap(),
            b"PENDING"
        );
    }

    #[test]
    fn native_journal_append_failure_retries_same_seq_then_publishes_next() {
        let dir = tempdir().unwrap();
        let journal = ProjectJournal::open_or_create(dir.path()).unwrap();
        let mut writer =
            TrackSegmentWriter::new(dir.path(), "mic".into(), TrackType::MicAudio, "pcm".into());
        fs::create_dir_all(dir.path().join("media/mic")).unwrap();
        let first = crate::fixtures::generate_valid_wav_segment(100_000, 48_000, 1);
        let first_temp = dir.path().join("media/mic/000001.wav.tmp");
        fs::write(&first_temp, &first).unwrap();
        journal.inject_fail_next_appends(1);
        assert!(writer
            .commit_native_segment(&first_temp, 0, 0, 48_000, 0, 0, &journal)
            .is_err());
        assert!(writer.has_pending_publication());
        assert!(dir.path().join("media/mic/000001.wav").exists());
        assert!(journal.read_all().unwrap().is_empty());

        let retried = writer
            .commit_native_segment(&first_temp, 0, 0, 48_000, 0, 0, &journal)
            .unwrap();
        assert_eq!(retried.relative_path, "media/mic/000001.wav");
        assert!(!writer.has_pending_publication());
        assert_eq!(journal.read_all().unwrap().len(), 1);

        let second = crate::fixtures::generate_valid_wav_segment(100_000, 48_000, 1);
        let second_temp = dir.path().join("media/mic/000002.wav.tmp");
        fs::write(&second_temp, &second).unwrap();
        journal.inject_fail_next_appends(1);
        assert!(writer
            .commit_native_segment(&second_temp, 1, 100_000, 48_000, 4_800, 100_000, &journal)
            .is_err());
        assert!(writer.has_pending_publication());

        let third = crate::fixtures::generate_valid_wav_segment(100_000, 48_000, 1);
        let third_temp = dir.path().join("media/mic/000003.wav.tmp");
        fs::write(&third_temp, &third).unwrap();
        let committed = writer
            .commit_native_segment(&third_temp, 2, 200_000, 48_000, 9_600, 200_000, &journal)
            .unwrap();
        assert_eq!(committed.relative_path, "media/mic/000003.wav");
        assert_eq!(journal.read_all().unwrap().len(), 3);
        assert!(!writer.has_pending_publication());
        assert!(dir.path().join("media/mic/000002.wav").exists());
        assert!(dir.path().join("media/mic/000003.wav").exists());
    }

    #[test]
    fn native_commit_rebases_boot_absolute_host_anchor() {
        let dir = tempdir().unwrap();
        let journal = ProjectJournal::open_or_create(dir.path()).unwrap();
        let mut writer =
            TrackSegmentWriter::new(dir.path(), "mic".into(), TrackType::MicAudio, "pcm".into());
        fs::create_dir_all(dir.path().join("media/mic")).unwrap();
        let first = crate::fixtures::generate_valid_wav_segment(100_000, 48_000, 1);
        let first_temp = dir.path().join("media/mic/000001.wav.tmp");
        fs::write(&first_temp, &first).unwrap();
        let first_commit = writer
            .commit_native_segment(
                &first_temp,
                0,
                193_799_000_000,
                48_000,
                0,
                2_000_000,
                &journal,
            )
            .unwrap();
        assert_eq!(
            (first_commit.start_us, first_commit.end_us),
            (0, 100_000),
            "boot-absolute host anchors must not become source duration"
        );

        let second = crate::fixtures::generate_valid_wav_segment(100_000, 48_000, 1);
        let second_temp = dir.path().join("media/mic/000002.wav.tmp");
        fs::write(&second_temp, &second).unwrap();
        let second_commit = writer
            .commit_native_segment(
                &second_temp,
                1,
                193_801_000_000,
                48_000,
                4_800,
                4_000_000,
                &journal,
            )
            .unwrap();
        assert_eq!(
            (second_commit.start_us, second_commit.end_us),
            (2_000_000, 2_100_000)
        );
        let records = journal.read_all().unwrap();
        assert!(matches!(
            &records[0],
            JournalRecord::SegmentCommitted {
                host_anchor_us: 0,
                start_us: 0,
                ..
            }
        ));
        assert!(matches!(
            &records[1],
            JournalRecord::SegmentCommitted {
                host_anchor_us: 2_000_000,
                start_us: 2_000_000,
                ..
            }
        ));
    }
}
