pub mod journal;
pub mod layout;
pub mod lock;
pub mod manifest;
pub mod media_validator;
pub mod pcm;
pub mod reader;
pub mod recovery;
pub mod revision;
pub mod segment_writer;
pub mod silence;
pub mod waveform;

pub use journal::{JournalError, JournalRecord, ProjectJournal};
pub use lock::{LockError, ProjectLock};
pub use manifest::{ManifestError, PauseInterval, ProjectManifest, TrackDescriptor, TrackType};
pub use media_validator::{MediaValidationError, MediaValidationInfo, MediaValidator};
pub use reader::{
    OpenedProject, ProjectReader, RetainedInterval, SegmentPage, SegmentSummary, TrackSummary,
};
pub use layout::EditLayout;
pub use recovery::{ProjectRecoveryReport, RecoveryEngine, RecoveryError, TrackRecoveryReport};
pub use revision::{EditDocument, EditHistory};
pub use segment_writer::{
    DurabilityFault, SegmentCommitResult, SegmentWriterError, TrackSegmentWriter,
};
pub use waveform::{WaveformPage, WaveformTrackContext};

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

const MAX_PROJECT_NAME_CHARS: usize = 80;

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

/// Returns the default dated project name for a given datetime, e.g. "Untitled 9 Sep 2026".
pub fn default_project_name_at<Tz: chrono::TimeZone>(dt: &chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    dt.format("Untitled %-d %b %Y").to_string()
}

/// Returns the default dated project name using local time, e.g. "Untitled 9 Sep 2026".
pub fn default_project_name() -> String {
    default_project_name_at(&chrono::Local::now())
}

/// Display name stored in `manifest.json`. Empty input becomes a dated default, e.g. `Untitled 9 Sep 2026`.
pub fn display_name_from_input(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        default_project_name()
    } else {
        trimmed.chars().take(MAX_PROJECT_NAME_CHARS).collect()
    }
}

pub fn display_name_from_input_at<Tz: chrono::TimeZone>(
    raw: &str,
    dt: &chrono::DateTime<Tz>,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        default_project_name_at(dt)
    } else {
        trimmed.chars().take(MAX_PROJECT_NAME_CHARS).collect()
    }
}

/// Folder name for a new bundle (`Name.aero`). Strips path separators and reserved characters.
pub fn bundle_folder_name(display_name: &str) -> String {
    let mut out = String::new();
    for ch in display_name.chars() {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            continue;
        }
        out.push(ch);
    }
    let out = out.trim().trim_matches('.').to_string();
    if out.is_empty() || out == "." || out == ".." {
        return format!("{}.aero", default_project_name());
    }
    if out.to_ascii_lowercase().ends_with(".aero") {
        out
    } else {
        format!("{out}.aero")
    }
}

/// Next free `{stem}.aero`, `{stem} 2.aero`, … under `parent`.
pub fn unique_bundle_path(parent: &Path, folder_name: &str) -> PathBuf {
    let candidate = parent.join(folder_name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = folder_name
        .strip_suffix(".aero")
        .or_else(|| folder_name.strip_suffix(".Aero"))
        .or_else(|| folder_name.strip_suffix(".AERO"))
        .unwrap_or(folder_name);
    for n in 2..10_000 {
        let next = parent.join(format!("{stem} {n}.aero"));
        if !next.exists() {
            return next;
        }
    }
    parent.join(format!("{stem} {}.aero", uuid::Uuid::new_v4()))
}

impl ProjectBundle {
    /// Creates a new `.aero` project bundle on disk with exclusive directory ownership.
    /// Fails if the bundle directory already exists to prevent accidental manifest overwrites.
    pub fn create_new<P: AsRef<Path>>(
        base_dir: P,
        session_id: &str,
        project_name: &str,
    ) -> Result<Self, ProjectError> {
        Self::create_named(base_dir, session_id, project_name, false)
    }

    /// Like [`create_new`], but `disambiguate` picks `Name 2.aero` instead of failing on a collision.
    pub fn create_named<P: AsRef<Path>>(
        base_dir: P,
        session_id: &str,
        project_name: &str,
        disambiguate: bool,
    ) -> Result<Self, ProjectError> {
        let display_name = display_name_from_input(project_name);
        let folder_name = bundle_folder_name(&display_name);
        let root_path = if disambiguate {
            unique_bundle_path(base_dir.as_ref(), &folder_name)
        } else {
            base_dir.as_ref().join(&folder_name)
        };

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

        let manifest = ProjectManifest::new(session_id.to_string(), display_name);
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
        assert_eq!(b1.root_path().file_name().unwrap(), "Project 1.aero");
        assert!(b1.root_path().exists());

        let b2 = ProjectBundle::create_new(dir.path(), session_id, "Project 1");
        assert!(matches!(b2, Err(ProjectError::AlreadyExists(_))));
    }

    #[test]
    fn named_bundle_strips_path_characters_and_disambiguates() {
        let dir = tempdir().unwrap();
        let first =
            ProjectBundle::create_named(dir.path(), "s1", "  Demo/Take:1  ", false).unwrap();
        assert_eq!(first.root_path().file_name().unwrap(), "DemoTake1.aero");
        assert_eq!(first.manifest().project_name, "Demo/Take:1");

        let second = ProjectBundle::create_named(dir.path(), "s2", "DemoTake1", true).unwrap();
        assert_eq!(second.root_path().file_name().unwrap(), "DemoTake1 2.aero");

        let untitled = ProjectBundle::create_named(dir.path(), "s3", "   ", true).unwrap();
        let expected_name = default_project_name();
        assert_eq!(
            untitled.root_path().file_name().unwrap(),
            format!("{expected_name}.aero").as_str()
        );
        assert_eq!(untitled.manifest().project_name, expected_name);

        let untitled2 = ProjectBundle::create_named(dir.path(), "s4", "", true).unwrap();
        assert_eq!(
            untitled2.root_path().file_name().unwrap(),
            format!("{expected_name} 2.aero").as_str()
        );
        assert_eq!(untitled2.manifest().project_name, expected_name);
    }

    #[test]
    fn test_dated_untitled_name_format() {
        use chrono::TimeZone;
        let dt = chrono::Utc.with_ymd_and_hms(2026, 9, 9, 12, 0, 0).unwrap();
        assert_eq!(default_project_name_at(&dt), "Untitled 9 Sep 2026");
        assert_eq!(
            display_name_from_input_at("   ", &dt),
            "Untitled 9 Sep 2026"
        );
        assert_eq!(
            display_name_from_input_at("Custom Take", &dt),
            "Custom Take"
        );

        let dt2 = chrono::Utc
            .with_ymd_and_hms(2026, 12, 25, 8, 30, 0)
            .unwrap();
        assert_eq!(default_project_name_at(&dt2), "Untitled 25 Dec 2026");
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
