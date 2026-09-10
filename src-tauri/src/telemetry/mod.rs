pub mod event;
pub mod native;
pub mod reader;

pub use event::{GeometryRecord, TelemetryEvent, TelemetryKind};
pub use native::{MouseTelemetryPermission, NativeMouseLogger};
pub use reader::{CanonicalEvent, CanonicalGeometry, CanonicalKind, TelemetryStream};

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TelemetryError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Durable telemetry logger that writes telemetry/events.jsonl and telemetry/geometry.jsonl
pub struct TelemetryLogger {
    events_file: Mutex<BufWriter<File>>,
    geometry_file: Mutex<BufWriter<File>>,
    next_seq: AtomicU64,
}

impl TelemetryLogger {
    pub fn open_or_create<P: AsRef<Path>>(telemetry_dir: P) -> Result<Self, TelemetryError> {
        let dir = telemetry_dir.as_ref();
        std::fs::create_dir_all(dir)?;

        let events_path = dir.join("events.jsonl");
        let geometry_path = dir.join("geometry.jsonl");

        let events_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(events_path)?;
        let geometry_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(geometry_path)?;

        Ok(Self {
            events_file: Mutex::new(BufWriter::new(events_file)),
            geometry_file: Mutex::new(BufWriter::new(geometry_file)),
            next_seq: AtomicU64::new(0),
        })
    }

    pub fn log_event(
        &self,
        t_us: u64,
        geometry_id: String,
        kind: TelemetryKind,
        norm_x: f32,
        norm_y: f32,
        inside_source: bool,
        visible: bool,
        cursor_id: Option<String>,
    ) -> Result<u64, TelemetryError> {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let event = TelemetryEvent::new(
            seq,
            t_us,
            geometry_id,
            kind,
            norm_x,
            norm_y,
            inside_source,
            visible,
            cursor_id,
        );

        let json = serde_json::to_string(&event)?;
        let mut writer = self.events_file.lock().unwrap();
        writeln!(writer, "{}", json)?;
        Ok(seq)
    }

    pub fn log_geometry(&self, record: GeometryRecord) -> Result<(), TelemetryError> {
        let json = serde_json::to_string(&record)?;
        let mut writer = self.geometry_file.lock().unwrap();
        writeln!(writer, "{}", json)?;
        writer.flush()?;
        Ok(())
    }

    pub fn flush(&self) -> Result<(), TelemetryError> {
        self.events_file.lock().unwrap().flush()?;
        self.geometry_file.lock().unwrap().flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_telemetry_logging() {
        let dir = tempdir().unwrap();
        let logger = TelemetryLogger::open_or_create(dir.path()).unwrap();

        let s0 = logger
            .log_event(
                1000,
                "g1".into(),
                TelemetryKind::Move,
                0.5,
                0.5,
                true,
                true,
                Some("arrow".into()),
            )
            .unwrap();

        let s1 = logger
            .log_event(
                2000,
                "g1".into(),
                TelemetryKind::Click,
                0.52,
                0.51,
                true,
                true,
                None,
            )
            .unwrap();

        assert_eq!(s0, 0);
        assert_eq!(s1, 1);
        logger.flush().unwrap();
    }
}
