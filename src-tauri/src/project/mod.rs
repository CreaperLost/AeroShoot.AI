pub mod journal;
pub mod lock;
pub mod manifest;
pub mod media_validator;
pub mod pcm;
pub mod reader;
pub mod recovery;
pub mod revision;
pub mod segment_writer;
pub mod waveform;

pub use journal::{JournalError, JournalRecord, ProjectJournal};
pub use lock::{LockError, ProjectLock};
pub use manifest::{ManifestError, PauseInterval, ProjectManifest, TrackDescriptor, TrackType};
pub use media_validator::{MediaValidationError, MediaValidationInfo, MediaValidator};
pub use reader::{
    OpenedProject, ProjectReader, RetainedInterval, SegmentPage, SegmentSummary, TrackSummary,
};
pub use recovery::{ProjectRecoveryReport, RecoveryEngine, RecoveryError, TrackRecoveryReport};
pub use revision::{EditDocument, EditHistory};
pub use segment_writer::{SegmentCommitResult, SegmentWriterError, TrackSegmentWriter};
pub use waveform::{WaveformPage, WaveformTrackContext};

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ProjectError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Manifest error: {0}")]
    Manifest(#[from] ManifestError),
    #[error("Journal error: {0}")]
    Journal(#[from] JournalError),
    #[error("Lock error: {0}")]
    Lock(#[from] LockError),
    #[error("Project bundle already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Manages an `.aero` project bundle directory structure with exclusive writer ownership
pub struct ProjectBundle {
    root_path: PathBuf,
    manifest: ProjectManifest,
    journal: Arc<ProjectJournal>,
    _lock: ProjectLock,
}

impl ProjectBundle {
    /// Creates a new `.aero` project bundle on disk with exclusive directory ownership.
    /// Fails if the bundle directory already exists to prevent accidental manifest overwrites.
    pub fn create_new<P: AsRef<Path>>(
        base_dir: P,
        session_id: &str,
        project_name: &str,
    ) -> Result<Self, ProjectError> {
        let bundle_name = format!("Project_Session_{}.aero", session_id);
        let root_path = base_dir.as_ref().join(bundle_name);

        if root_path.exists() {
            return Err(ProjectError::AlreadyExists(root_path));
        }

        // Create standard directory tree
        fs::create_dir_all(root_path.join("telemetry"))?;
        fs::create_dir_all(root_path.join("media").join("screen"))?;
        fs::create_dir_all(root_path.join("media").join("webcam"))?;
        fs::create_dir_all(root_path.join("media").join("system"))?;
        fs::create_dir_all(root_path.join("media").join("mic"))?;
        fs::create_dir_all(root_path.join("cache"))?;

        // Acquire exclusive project writer lock
        let lock = ProjectLock::acquire(&root_path)?;

        let manifest = ProjectManifest::new(session_id.to_string(), project_name.to_string());
        let manifest_path = root_path.join("manifest.json");
        manifest.save_with_backup(&manifest_path)?;

        // Open append-only journal
        let journal = Arc::new(ProjectJournal::open_or_create(&root_path)?);

        Ok(Self {
            root_path,
            manifest,
            journal,
            _lock: lock,
        })
    }

    /// Opens an existing project bundle, acquiring the writer lock and verifying the manifest.
    pub fn open_existing<P: AsRef<Path>>(project_dir: P) -> Result<Self, ProjectError> {
        let root_path = project_dir.as_ref().to_path_buf();
        let lock = ProjectLock::acquire(&root_path)?;

        let manifest_path = root_path.join("manifest.json");
        let data = fs::read_to_string(&manifest_path)?;
        let manifest: ProjectManifest = serde_json::from_str(&data)?;
        manifest.validate()?;

        let journal = Arc::new(ProjectJournal::open_or_create(&root_path)?);

        Ok(Self {
            root_path,
            manifest,
            journal,
            _lock: lock,
        })
    }

    /// Creates a new segment writer for a specific track within this project bundle.
    pub fn create_segment_writer(
        &self,
        track_id: &str,
        track_type: TrackType,
        codec: &str,
    ) -> TrackSegmentWriter {
        TrackSegmentWriter::new(
            &self.root_path,
            track_id.to_string(),
            track_type,
            codec.to_string(),
        )
    }

    /// Saves updated manifest state with durable snapshot replacement and prior revision backup.
    pub fn update_manifest(&mut self, manifest: ProjectManifest) -> Result<(), ProjectError> {
        let manifest_path = self.root_path.join("manifest.json");
        manifest.save_with_backup(&manifest_path)?;
        self.manifest = manifest;
        Ok(())
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn manifest(&self) -> &ProjectManifest {
        &self.manifest
    }

    pub fn manifest_mut(&mut self) -> &mut ProjectManifest {
        &mut self.manifest
    }

    pub fn journal(&self) -> &ProjectJournal {
        &self.journal
    }

    pub fn journal_arc(&self) -> Arc<ProjectJournal> {
        Arc::clone(&self.journal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_create_new_rejects_existing_bundle() {
        let dir = tempdir().unwrap();
        let session_id = "test-existing-1";

        let b1 = ProjectBundle::create_new(dir.path(), session_id, "Project 1").unwrap();
        assert!(b1.root_path().exists());

        // Second creation with same session_id in same dir must fail
        let b2 = ProjectBundle::create_new(dir.path(), session_id, "Project 1 Again");
        assert!(matches!(b2, Err(ProjectError::AlreadyExists(_))));
    }

    #[test]
    fn test_snapshot_backup_preserves_prior_revision() {
        let dir = tempdir().unwrap();
        let session_id = "test-snapshot-1";

        let mut bundle = ProjectBundle::create_new(dir.path(), session_id, "Rev 1").unwrap();
        let manifest_path = bundle.root_path().join("manifest.json");
        let bak_path = bundle.root_path().join("manifest.bak");

        assert!(manifest_path.exists());
        assert!(!bak_path.exists());

        // Update manifest
        let mut new_manifest = bundle.manifest().clone();
        new_manifest.duration_us = 5_000_000;
        bundle.update_manifest(new_manifest).unwrap();

        // Backup file must now exist and preserve Rev 1
        assert!(bak_path.exists());
        let bak_content = fs::read_to_string(&bak_path).unwrap();
        assert!(bak_content.contains("Rev 1"));
    }
}
