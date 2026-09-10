//! Versioned edit document (`project.json`). Source media is never rewritten.
use super::layout::validate_layout;
use super::reader::{open_regular, safe_path, RetainedInterval};
use crate::timeline::{SourceInterval, TimelineMapper};
use crate::zoom::{
    attach_zoom_edited_ranges, validate_zooms, ZoomKeyframe, ZoomSource, ZoomSuggestion,
    MAX_DISMISSED_ZOOMS, MAX_ZOOMS,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

pub use super::layout::EditLayout;

pub const EDIT_SCHEMA_VERSION: u32 = 1;
pub const MAX_EDIT_BYTES: u64 = 1_048_576;
pub const MAX_RETAINED_INTERVALS: usize = 10_000;
pub const MAX_UNDO: usize = 64;
pub const MAX_CUTS_PER_REVISION: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EditDocument {
    pub schema_version: u32,
    pub revision: u64,
    pub retained_intervals: Vec<RetainedInterval>,
    #[serde(default)]
    pub layout: EditLayout,
    #[serde(default)]
    pub zooms: Vec<ZoomKeyframe>,
    #[serde(default)]
    pub dismissed_zoom_ids: Vec<String>,
}

impl Default for EditDocument {
    fn default() -> Self {
        Self {
            schema_version: EDIT_SCHEMA_VERSION,
            revision: 0,
            retained_intervals: Vec::new(),
            layout: EditLayout::default(),
            zooms: Vec::new(),
            dismissed_zoom_ids: Vec::new(),
        }
    }
}

impl EditDocument {
    pub fn from_retained(retained: Vec<RetainedInterval>) -> Result<Self, String> {
        validate_retained(&retained)?;
        Ok(Self {
            schema_version: EDIT_SCHEMA_VERSION,
            revision: 0,
            retained_intervals: retained,
            layout: EditLayout::default(),
            zooms: Vec::new(),
            dismissed_zoom_ids: Vec::new(),
        })
    }

    pub fn mapper(&self) -> Result<TimelineMapper, String> {
        TimelineMapper::try_new(
            self.retained_intervals
                .iter()
                .enumerate()
                .map(|(i, interval)| {
                    SourceInterval::new(format!("ret-{i}"), interval.start_us, interval.end_us)
                })
                .collect(),
        )
    }

    pub fn zoom_suggestions(&self) -> Vec<ZoomSuggestion> {
        self.zooms.iter().map(ZoomKeyframe::as_suggestion).collect()
    }

    pub fn attach_zoom_ranges(&mut self) -> Result<(), String> {
        let mapper = self.mapper()?;
        attach_zoom_edited_ranges(&mut self.zooms, &mapper);
        Ok(())
    }

    pub fn edited_duration_us(&self) -> Result<u64, String> {
        Ok(self.mapper()?.total_edited_duration_us())
    }
}

pub fn validate_retained(retained: &[RetainedInterval]) -> Result<(), String> {
    if retained.len() > MAX_RETAINED_INTERVALS {
        return Err("Too many retained intervals".into());
    }
    let mut previous_end = 0u64;
    for interval in retained {
        if interval.end_us > 9_007_199_254_740_991 {
            return Err("Retained timestamp exceeds supported precision".into());
        }
        if interval.end_us <= interval.start_us {
            return Err("Retained interval must be a half-open range".into());
        }
        if interval.start_us < previous_end {
            return Err("Retained intervals must be sorted and non-overlapping".into());
        }
        previous_end = interval.end_us;
    }
    Ok(())
}

pub fn load_edit_document(root: &Path) -> Result<Option<EditDocument>, String> {
    let path = safe_path(root, "project.json")?;
    if !path.is_file() {
        return Ok(None);
    }
    let file = open_regular(&path)?;
    let mut bytes = Vec::new();
    file.take(MAX_EDIT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_EDIT_BYTES {
        return Err("Edit document exceeds size limit".into());
    }
    let document: EditDocument =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid edit document: {e}"))?;
    if document.schema_version != EDIT_SCHEMA_VERSION {
        return Err(format!(
            "Unsupported edit schema version: {}",
            document.schema_version
        ));
    }
    validate_retained(&document.retained_intervals)?;
    validate_layout(&document.layout)?;
    validate_zooms(&document.zooms)?;
    validate_dismissed(&document.dismissed_zoom_ids)?;
    document.mapper()?;
    Ok(Some(document))
}

pub fn save_edit_document(root: &Path, document: &EditDocument) -> Result<(), String> {
    validate_retained(&document.retained_intervals)?;
    validate_layout(&document.layout)?;
    validate_zooms(&document.zooms)?;
    validate_dismissed(&document.dismissed_zoom_ids)?;
    if document.schema_version != EDIT_SCHEMA_VERSION {
        return Err("Unsupported edit schema version".into());
    }
    let path = safe_path(root, "project.json")?;
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink() {
            return Err("Edit document cannot be a symlink".into());
        }
    }
    let serialized = serde_json::to_vec_pretty(document).map_err(|e| e.to_string())?;
    if serialized.len() as u64 > MAX_EDIT_BYTES {
        return Err("Edit document exceeds size limit".into());
    }
    let mut temp = tempfile::NamedTempFile::new_in(root).map_err(|e| e.to_string())?;
    temp.write_all(&serialized).map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(&path).map_err(|e| e.to_string())?;
    // The document is committed after the atomic rename. Directory sync is best-effort:
    // reporting a failed edit after publication would leave memory and disk divergent.
    if let Ok(directory) = fs::File::open(root) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn validate_dismissed(ids: &[String]) -> Result<(), String> {
    if ids.len() > MAX_DISMISSED_ZOOMS {
        return Err("Too many dismissed zoom ids".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        if id.is_empty() || id.len() > 128 || !seen.insert(id) {
            return Err("Invalid dismissed zoom id".into());
        }
    }
    Ok(())
}

fn persist_revision(root: &Path, expected: u64, next: &EditDocument) -> Result<(), String> {
    let lock_path = safe_path(root, ".edit.lock")?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let lock = options.open(lock_path).map_err(|e| e.to_string())?;
    if !lock.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Invalid edit lock".into());
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("Another editor is saving this project".into());
        }
    }
    let result = (|| {
        if load_edit_document(root)?.map(|d| d.revision).unwrap_or(0) != expected {
            return Err("Stale edit revision on disk; reopen the project".into());
        }
        save_edit_document(root, next)
    })();
    // Explicitly unlock: another thread may have forked a child that briefly
    // inherits this open-file description before exec closes CLOEXEC handles.
    // Merely closing our descriptor can then leave the lock held by the child.
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::flock(lock.as_raw_fd(), libc::LOCK_UN);
        }
    }
    result
}

#[derive(Clone, Debug)]
pub struct EditHistory {
    pub current: EditDocument,
    undo: Vec<EditDocument>,
    redo: Vec<EditDocument>,
}

impl EditHistory {
    pub fn new(current: EditDocument) -> Self {
        Self {
            current,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn undo_available(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn redo_available(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn commit(
        &mut self,
        expected_revision: u64,
        next_retained: Vec<RetainedInterval>,
        persist_root: &Path,
    ) -> Result<&EditDocument, String> {
        let mut next = self.current.clone();
        next.retained_intervals = next_retained;
        self.commit_next(expected_revision, persist_root, next)
    }

    pub fn commit_next(
        &mut self,
        expected_revision: u64,
        persist_root: &Path,
        mut next: EditDocument,
    ) -> Result<&EditDocument, String> {
        if expected_revision != self.current.revision {
            return Err("Stale edit revision".into());
        }
        validate_retained(&next.retained_intervals)?;
        TimelineMapper::try_new(
            next.retained_intervals
                .iter()
                .enumerate()
                .map(|(i, interval)| {
                    SourceInterval::new(format!("ret-{i}"), interval.start_us, interval.end_us)
                })
                .collect(),
        )?;
        validate_layout(&next.layout)?;
        validate_zooms(&next.zooms)?;
        validate_dismissed(&next.dismissed_zoom_ids)?;
        next.schema_version = EDIT_SCHEMA_VERSION;
        next.revision = self
            .current
            .revision
            .checked_add(1)
            .ok_or("Revision overflow")?;
        next.attach_zoom_ranges()?;
        persist_revision(persist_root, expected_revision, &next)?;
        self.undo.push(self.current.clone());
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.current = next;
        Ok(&self.current)
    }

    pub fn update_layout(
        &mut self,
        expected_revision: u64,
        layout: EditLayout,
        persist_root: &Path,
    ) -> Result<&EditDocument, String> {
        if expected_revision != self.current.revision {
            return Err("Stale edit revision".into());
        }
        validate_layout(&layout)?;
        if layout == self.current.layout {
            return Ok(&self.current);
        }
        let mut next = self.current.clone();
        next.layout = layout;
        self.commit_next(expected_revision, persist_root, next)
    }

    pub fn accept_zooms(
        &mut self,
        expected_revision: u64,
        suggestions: &[ZoomSuggestion],
        persist_root: &Path,
    ) -> Result<&EditDocument, String> {
        if suggestions.is_empty() {
            return Err("No zoom suggestions selected".into());
        }
        let mut next = self.current.clone();
        let existing: std::collections::BTreeSet<_> =
            next.zooms.iter().map(|z| z.id.clone()).collect();
        let dismissed: std::collections::BTreeSet<_> =
            next.dismissed_zoom_ids.iter().cloned().collect();
        for suggestion in suggestions {
            if existing.contains(&suggestion.id) || dismissed.contains(&suggestion.id) {
                continue;
            }
            next.zooms.push(ZoomKeyframe::from_suggestion(
                suggestion.clone(),
                ZoomSource::Generated,
            ));
        }
        if next.zooms.len() == self.current.zooms.len() {
            return Err("Those zoom suggestions are already applied or dismissed".into());
        }
        if next.zooms.len() > MAX_ZOOMS {
            return Err("Too many zoom keyframes".into());
        }
        next.zooms
            .sort_by(|a, b| a.source_start_us.cmp(&b.source_start_us).then(a.id.cmp(&b.id)));
        self.commit_next(expected_revision, persist_root, next)
    }

    pub fn dismiss_zooms(
        &mut self,
        expected_revision: u64,
        ids: &[String],
        persist_root: &Path,
    ) -> Result<&EditDocument, String> {
        if ids.is_empty() {
            return Err("No zoom ids selected".into());
        }
        let mut next = self.current.clone();
        let remove: std::collections::BTreeSet<_> = ids.iter().cloned().collect();
        next.zooms.retain(|z| !remove.contains(&z.id));
        for id in ids {
            if !next.dismissed_zoom_ids.iter().any(|d| d == id) {
                next.dismissed_zoom_ids.push(id.clone());
            }
        }
        if next.zooms == self.current.zooms
            && next.dismissed_zoom_ids == self.current.dismissed_zoom_ids
        {
            return Err("Those zoom ids are already dismissed".into());
        }
        self.commit_next(expected_revision, persist_root, next)
    }

    pub fn update_zoom(
        &mut self,
        expected_revision: u64,
        patch: ZoomKeyframe,
        persist_root: &Path,
    ) -> Result<&EditDocument, String> {
        let mut next = self.current.clone();
        let Some(existing) = next.zooms.iter_mut().find(|z| z.id == patch.id) else {
            return Err("Unknown zoom keyframe".into());
        };
        existing.source_start_us = patch.source_start_us;
        existing.source_end_us = patch.source_end_us;
        existing.center_x = patch.center_x;
        existing.center_y = patch.center_y;
        existing.scale = patch.scale;
        existing.transition_us = patch.transition_us;
        // Moving/resizing a generated zoom keeps its id so regeneration cannot
        // replace it, and marks it manual so a later accept cannot reset it.
        existing.source = ZoomSource::Manual;
        self.commit_next(expected_revision, persist_root, next)
    }

    pub fn add_manual_zoom(
        &mut self,
        expected_revision: u64,
        edited_start_us: u64,
        edited_end_us: u64,
        center_x: f64,
        center_y: f64,
        scale: f64,
        persist_root: &Path,
    ) -> Result<&EditDocument, String> {
        if edited_end_us <= edited_start_us {
            return Err("Zoom must be a half-open edited range".into());
        }
        let mapper = self.current.mapper()?;
        let source_start = mapper
            .edited_to_source_us(edited_start_us)
            .ok_or("Zoom start is not on retained media")?;
        let source_end_sample = mapper
            .edited_to_source_us(edited_end_us.saturating_sub(1))
            .ok_or("Zoom end is not on retained media")?;
        let source_end = source_end_sample.saturating_add(1);
        if source_end <= source_start {
            return Err("Zoom range does not map onto source time".into());
        }
        let duration = source_end - source_start;
        if duration < 3 {
            return Err("Zoom range is too short".into());
        }
        let transition_us = (duration / 5).clamp(1, 400_000).min(duration.saturating_sub(1));
        let mut next = self.current.clone();
        let id = format!("m-{}-{}", source_start, next.zooms.len());
        next.zooms.push(ZoomKeyframe {
            id,
            source_start_us: source_start,
            source_end_us: source_end,
            center_x,
            center_y,
            scale,
            transition_us,
            origin: crate::zoom::ZoomOrigin::Click,
            contributing_event_seqs: Vec::new(),
            source: ZoomSource::Manual,
            edited_ranges: Vec::new(),
        });
        next.zooms
            .sort_by(|a, b| a.source_start_us.cmp(&b.source_start_us).then(a.id.cmp(&b.id)));
        self.commit_next(expected_revision, persist_root, next)
    }

    pub fn delete_zoom(
        &mut self,
        expected_revision: u64,
        id: &str,
        persist_root: &Path,
    ) -> Result<&EditDocument, String> {
        let mut next = self.current.clone();
        let Some(index) = next.zooms.iter().position(|z| z.id == id) else {
            return Err("Unknown zoom keyframe".into());
        };
        let removed = next.zooms.remove(index);
        if removed.source == ZoomSource::Generated
            && !next.dismissed_zoom_ids.iter().any(|d| d == id)
        {
            next.dismissed_zoom_ids.push(id.to_string());
        }
        self.commit_next(expected_revision, persist_root, next)
    }

    pub fn ripple_cuts(
        &mut self,
        expected_revision: u64,
        cuts: &[(u64, u64)],
        persist_root: &Path,
    ) -> Result<&EditDocument, String> {
        if cuts.is_empty() {
            return Err("No cuts selected".into());
        }
        if cuts.len() > MAX_CUTS_PER_REVISION {
            return Err("Too many cuts in one revision".into());
        }
        if expected_revision != self.current.revision {
            return Err("Stale edit revision".into());
        }
        let mut mapper = self.current.mapper()?;
        let mut ordered = cuts.to_vec();
        ordered.sort_unstable();
        if ordered.windows(2).any(|w| w[0].1 > w[1].0) {
            return Err("Cut ranges overlap".into());
        }
        ordered.reverse();
        for (start, end) in ordered {
            mapper.ripple_cut_edited(start, end)?;
        }
        let retained = mapper
            .intervals()
            .iter()
            .map(|interval| RetainedInterval {
                start_us: interval.start_us,
                end_us: interval.end_us,
            })
            .collect();
        self.commit(expected_revision, retained, persist_root)
    }

    pub fn undo(
        &mut self,
        expected_revision: u64,
        persist_root: &Path,
    ) -> Result<&EditDocument, String> {
        if expected_revision != self.current.revision {
            return Err("Stale edit revision".into());
        }
        let mut previous = self.undo.last().cloned().ok_or("Nothing to undo")?;
        previous.revision = self
            .current
            .revision
            .checked_add(1)
            .ok_or("Revision overflow")?;
        persist_revision(persist_root, expected_revision, &previous)?;
        self.undo.pop();
        self.redo.push(self.current.clone());
        self.current = previous;
        Ok(&self.current)
    }

    pub fn redo(
        &mut self,
        expected_revision: u64,
        persist_root: &Path,
    ) -> Result<&EditDocument, String> {
        if expected_revision != self.current.revision {
            return Err("Stale edit revision".into());
        }
        let mut next = self.redo.last().cloned().ok_or("Nothing to redo")?;
        next.revision = self
            .current
            .revision
            .checked_add(1)
            .ok_or("Revision overflow")?;
        persist_revision(persist_root, expected_revision, &next)?;
        self.redo.pop();
        self.undo.push(self.current.clone());
        self.current = next;
        Ok(&self.current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn failed_save_preserves_history_and_revision_ids_never_repeat() {
        let dir = tempdir().unwrap();
        let initial = EditDocument::from_retained(vec![RetainedInterval {
            start_us: 0,
            end_us: 1_000_000,
        }])
        .unwrap();
        let mut history = EditHistory::new(initial.clone());
        fs::create_dir(dir.path().join("project.json")).unwrap();
        assert!(history
            .ripple_cuts(0, &[(100_000, 200_000)], dir.path())
            .is_err());
        assert_eq!(history.current, initial);
        assert!(!history.undo_available());
        fs::remove_dir(dir.path().join("project.json")).unwrap();
        history
            .ripple_cuts(0, &[(100_000, 200_000)], dir.path())
            .unwrap();
        history.undo(1, dir.path()).unwrap();
        history
            .ripple_cuts(2, &[(300_000, 400_000)], dir.path())
            .unwrap();
        assert_eq!(history.current.revision, 3);
        assert!(history.ripple_cuts(1, &[(0, 1)], dir.path()).is_err());
        let snapshot = history.current.clone();
        fs::remove_file(dir.path().join("project.json")).unwrap();
        fs::create_dir(dir.path().join("project.json")).unwrap();
        assert!(history.undo(3, dir.path()).is_err());
        assert_eq!(history.current, snapshot);
        assert!(history.undo_available());
        assert!(!history.redo_available());
    }

    #[test]
    fn second_editor_cannot_overwrite_newer_disk_revision() {
        let dir = tempdir().unwrap();
        let initial = EditDocument::from_retained(vec![RetainedInterval {
            start_us: 0,
            end_us: 1_000_000,
        }])
        .unwrap();
        let mut first = EditHistory::new(initial.clone());
        let mut second = EditHistory::new(initial.clone());
        first.ripple_cuts(0, &[(0, 100_000)], dir.path()).unwrap();
        assert!(second
            .ripple_cuts(0, &[(0, 200_000)], dir.path())
            .unwrap_err()
            .contains("Stale"));
        assert_eq!(second.current, initial);
        assert_eq!(
            load_edit_document(dir.path()).unwrap().unwrap(),
            first.current
        );
    }

    #[test]
    fn persist_roundtrip_and_stale_revision() {
        let dir = tempdir().unwrap();
        let mut history = EditHistory::new(
            EditDocument::from_retained(vec![RetainedInterval {
                start_us: 0,
                end_us: 10_000_000,
            }])
            .unwrap(),
        );
        history
            .ripple_cuts(0, &[(2_000_000, 5_000_000)], dir.path())
            .unwrap();
        assert_eq!(history.current.revision, 1);
        assert_eq!(history.current.retained_intervals.len(), 2);
        assert!(history
            .ripple_cuts(0, &[(0, 1_000_000)], dir.path())
            .unwrap_err()
            .contains("Stale"));
        history.undo(1, dir.path()).unwrap();
        assert_eq!(history.current.revision, 2);
        assert_eq!(history.current.retained_intervals.len(), 1);
        history.redo(2, dir.path()).unwrap();
        let loaded = load_edit_document(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.revision, 3);
        assert_eq!(loaded.retained_intervals[0].end_us, 2_000_000);
    }

    #[test]
    fn zoom_edits_undo_and_do_not_revive_dismissed_or_overwrite_manual() {
        use crate::zoom::{ZoomOrigin, ZoomSuggestion};
        let dir = tempdir().unwrap();
        let mut history = EditHistory::new(
            EditDocument::from_retained(vec![RetainedInterval {
                start_us: 0,
                end_us: 10_000_000,
            }])
            .unwrap(),
        );
        let suggestion = ZoomSuggestion {
            id: "z-1-n1".into(),
            source_start_us: 1_000_000,
            source_end_us: 3_000_000,
            center_x: 0.4,
            center_y: 0.4,
            scale: 2.0,
            transition_us: 400_000,
            origin: ZoomOrigin::Click,
            contributing_event_seqs: vec![1],
            edited_ranges: Vec::new(),
        };
        history
            .accept_zooms(0, &[suggestion.clone()], dir.path())
            .unwrap();
        assert_eq!(history.current.zooms.len(), 1);
        assert_eq!(history.current.zooms[0].source, ZoomSource::Generated);
        let mut moved = history.current.zooms[0].clone();
        moved.source_start_us = 1_200_000;
        moved.source_end_us = 3_200_000;
        history.update_zoom(1, moved, dir.path()).unwrap();
        assert_eq!(history.current.zooms[0].source, ZoomSource::Manual);
        history
            .accept_zooms(2, &[suggestion.clone()], dir.path())
            .unwrap_err();
        history.dismiss_zooms(2, &["z-1-n1".into()], dir.path()).unwrap();
        assert!(history.current.zooms.is_empty());
        history
            .accept_zooms(3, &[suggestion], dir.path())
            .unwrap_err();
        history.undo(3, dir.path()).unwrap();
        assert_eq!(history.current.zooms.len(), 1);
        assert_eq!(history.current.zooms[0].source, ZoomSource::Manual);
        let loaded = load_edit_document(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.zooms[0].source_start_us, 1_200_000);
        assert_eq!(loaded.revision, 4);
    }

    #[test]
    fn layout_edits_undo_and_reopen() {
        let dir = tempdir().unwrap();
        let mut history = EditHistory::new(
            EditDocument::from_retained(vec![RetainedInterval {
                start_us: 0,
                end_us: 1_000_000,
            }])
            .unwrap(),
        );
        let mut layout = EditLayout::default();
        layout.aspect_ratio = "9:16".into();
        layout.padding_px = 24;
        layout.background_type = "solid".into();
        layout.color_start = "#ff0000".into();
        layout.webcam_mirror = false;
        layout.webcam_position = "top-left".into();
        history.update_layout(0, layout.clone(), dir.path()).unwrap();
        assert_eq!(history.current.revision, 1);
        assert_eq!(history.current.layout.aspect_ratio, "9:16");
        history.undo(1, dir.path()).unwrap();
        assert_eq!(history.current.layout.aspect_ratio, "16:9");
        history.redo(2, dir.path()).unwrap();
        let loaded = load_edit_document(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.layout.padding_px, 24);
        assert_eq!(loaded.layout.color_start, "#ff0000");
        assert!(!loaded.layout.webcam_mirror);
        assert_eq!(loaded.revision, 3);
        let mut bad = layout;
        bad.padding_px = 999;
        assert!(history.update_layout(3, bad, dir.path()).is_err());
        assert_eq!(history.current.revision, 3);
    }
}
