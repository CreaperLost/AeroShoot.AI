//! Versioned edit document (`project.json`).
use super::layout::EditLayout;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

pub const EDIT_SCHEMA_VERSION: u32 = 1;
const MAX_EDIT_DOCUMENT_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RetainedInterval {
    pub start_us: u64,
    pub end_us: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EditDocument {
    pub schema_version: u32,
    pub revision: u64,
    pub retained_intervals: Vec<RetainedInterval>,
    #[serde(default)]
    pub layout: EditLayout,
    #[serde(default)]
    pub zooms: Vec<serde_json::Value>,
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
        Ok(Self {
            schema_version: EDIT_SCHEMA_VERSION,
            revision: 0,
            retained_intervals: retained,
            layout: EditLayout::default(),
            zooms: Vec::new(),
            dismissed_zoom_ids: Vec::new(),
        })
    }
}

pub fn save_edit_document(root: &Path, document: &EditDocument) -> Result<(), String> {
    let path = root.join("project.json");
    let json = serde_json::to_string_pretty(document).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_edit_document(root: &Path) -> Result<Option<EditDocument>, String> {
    let path = root.join("project.json");
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file() {
        return Err("project.json is not a regular file".into());
    }
    if metadata.len() > MAX_EDIT_DOCUMENT_BYTES {
        return Err("project.json exceeds the 8 MiB safety limit".into());
    }
    let bytes = fs::read(&path).map_err(|error| error.to_string())?;
    let document: EditDocument =
        serde_json::from_slice(&bytes).map_err(|error| format!("Invalid project.json: {error}"))?;
    if document.schema_version != EDIT_SCHEMA_VERSION {
        return Err(format!(
            "Unsupported project.json schema version {}",
            document.schema_version
        ));
    }
    Ok(Some(document))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_document_is_none_and_saved_document_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(load_edit_document(temp.path()).unwrap(), None);
        let mut expected = EditDocument::default();
        expected.layout.aspect_ratio = "9:16".into();
        save_edit_document(temp.path(), &expected).unwrap();
        assert_eq!(load_edit_document(temp.path()).unwrap(), Some(expected));
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("project.json"),
            r#"{"schemaVersion":999,"revision":0,"retainedIntervals":[]}"#,
        )
        .unwrap();
        assert!(load_edit_document(temp.path())
            .unwrap_err()
            .contains("Unsupported"));
    }
}
