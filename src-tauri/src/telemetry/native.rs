//! Version 2 native mouse stream. V1 clicks remain unchanged; v2 stores
//! authoritative transitions and never invents derived click provenance.
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

/// Live geometry poll / uncertainty interval. H2 does not tighten this without
/// a measured adapter.
pub const GEOMETRY_SAMPLING_INTERVAL_US: u64 = 100_000;

pub const GAP_INPUT_MONITORING_UNAVAILABLE: &str = "input_monitoring_unavailable";
pub const GAP_INPUT_MONITORING_REVOKED: &str = "input_monitoring_revoked";
pub const GAP_EVENT_TAP_UNAVAILABLE: &str = "event_tap_unavailable";
pub const GAP_EVENT_TAP_DISABLED: &str = "event_tap_disabled";
pub const GAP_QUEUE_OVERFLOW: &str = "queue_overflow";
pub const GAP_UNSUPPORTED_SOURCE_GEOMETRY: &str = "unsupported_source_geometry";
pub const GAP_GEOMETRY_CHANGED: &str = "geometry_changed";
pub const GAP_RECORDING_PAUSED: &str = "recording_paused";
pub const GAP_INITIAL_BUTTON_STATE_UNKNOWN: &str = "initial_button_state_unknown";

/// Honest boolean pair for Input Monitoring. This is not a capture
/// `PermissionStatus` bundle (screen/camera/microphone string states).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseTelemetryPermission {
    pub supported: bool,
    pub authorized: bool,
}

impl MouseTelemetryPermission {
    pub fn macos(authorized: bool) -> Self {
        Self {
            supported: true,
            authorized,
        }
    }

    pub fn unsupported() -> Self {
        Self {
            supported: false,
            authorized: false,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MousePayload {
    Move,
    ButtonDown {
        button: u32,
    },
    ButtonUp {
        button: u32,
    },
    Scroll {
        delta_x: f64,
        delta_y: f64,
        units: ScrollUnits,
        precise: bool,
        phase: i64,
        momentum_phase: i64,
    },
    Gap {
        reason: String,
        start_us: u64,
        end_us: u64,
        dropped_events: u64,
    },
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollUnits {
    Pixels,
    Lines,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MouseEvent {
    pub version: u32,
    pub seq: u64,
    pub t_us: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub norm_x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub norm_y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside_source: Option<bool>,
    pub payload: MousePayload,
    // Visibility and modifiers are unknown in this adapter, not fabricated.
}
#[derive(Debug, Deserialize, Serialize)]
pub struct MouseBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct MouseGeometry {
    pub version: u32,
    pub geometry_id: String,
    pub t_us: u64,
    pub coordinate_space: String,
    pub source_id: String,
    pub bounds: MouseBounds,
    pub output_width: u32,
    pub output_height: u32,
    pub sampling_interval_us: u64,
    pub cursor_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_height: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_degrees: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_to_physical_scale_x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_to_physical_scale_y: Option<f64>,
}
#[derive(Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum NativeRecord {
    Event(MouseEvent),
    Geometry(MouseGeometry),
    Flush,
}

pub struct NativeMouseLogger {
    events: BufWriter<File>,
    geometry: BufWriter<File>,
    current_geometry: Option<String>,
    next_seq: u64,
}
impl NativeMouseLogger {
    /// Native sessions own fresh project bundles. Never append a new sequence
    /// zero to an existing stream or follow a preexisting telemetry symlink.
    pub fn create(root: &Path) -> Result<Self, String> {
        let dir = root.join("telemetry");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        if std::fs::symlink_metadata(&dir)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Telemetry directory is a symlink".into());
        }
        let open = |name: &str| {
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(dir.join(name))
                .map(BufWriter::new)
                .map_err(|e| e.to_string())
        };
        Ok(Self {
            events: open("events.jsonl")?,
            geometry: open("geometry.jsonl")?,
            current_geometry: None,
            next_seq: 0,
        })
    }
    pub fn append(&mut self, json: &str) -> Result<(), String> {
        if json.len() > 16_384 {
            return Err("Oversized mouse record".into());
        }
        let record: NativeRecord = serde_json::from_str(json).map_err(|e| e.to_string())?;
        match record {
            NativeRecord::Flush => {
                self.events.flush().map_err(|e| e.to_string())?;
            }
            NativeRecord::Geometry(record) => {
                if record.version != 2
                    || record.geometry_id.is_empty()
                    || record.geometry_id.len() > 128
                    || record.coordinate_space != "quartz_global"
                    || record.cursor_mode != "baked"
                    || record.sampling_interval_us != GEOMETRY_SAMPLING_INTERVAL_US
                    || record.bounds.width <= 0.0
                    || record.bounds.height <= 0.0
                    || record.output_width == 0
                    || record.output_height == 0
                {
                    return Err("Invalid mouse geometry".into());
                }
                serde_json::to_writer(&mut self.geometry, &record).map_err(|e| e.to_string())?;
                self.geometry.write_all(b"\n").map_err(|e| e.to_string())?;
                self.geometry.flush().map_err(|e| e.to_string())?;
                self.current_geometry = Some(record.geometry_id);
            }
            NativeRecord::Event(record) => {
                if record.version != 2 || record.seq != self.next_seq {
                    return Err("Invalid mouse version/sequence".into());
                }
                match &record.payload {
                    MousePayload::Gap {
                        start_us,
                        end_us,
                        reason,
                        ..
                    } => {
                        if start_us > end_us || *end_us != record.t_us || reason.len() > 256 {
                            return Err("Invalid mouse gap".into());
                        }
                    }
                    _ => {
                        if record.geometry_id.is_none()
                            || record.geometry_id != self.current_geometry
                        {
                            return Err("Unknown mouse geometry".into());
                        }
                        let (Some(x), Some(y), Some(inside)) =
                            (record.norm_x, record.norm_y, record.inside_source)
                        else {
                            return Err("Missing mouse coordinates".into());
                        };
                        if !x.is_finite()
                            || !y.is_finite()
                            || inside != ((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y))
                        {
                            return Err("Invalid mouse coordinates".into());
                        }
                    }
                }
                serde_json::to_writer(&mut self.events, &record).map_err(|e| e.to_string())?;
                self.events.write_all(b"\n").map_err(|e| e.to_string())?;
                self.next_seq += 1;
            }
        }
        Ok(())
    }
    pub fn finish(&mut self) -> Result<(), String> {
        self.geometry
            .flush()
            .and_then(|_| self.geometry.get_ref().sync_all())
            .map_err(|e| e.to_string())?;
        self.events
            .flush()
            .and_then(|_| self.events.get_ref().sync_all())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn geometry() -> serde_json::Value {
        json!({"record":"geometry","version":2,"geometry_id":"g1","t_us":0,
            "coordinate_space":"quartz_global","source_id":"display:1",
            "bounds":{"x":-1920,"y":0,"width":1920,"height":1080},
            "output_width":1920,"output_height":1080,"sampling_interval_us":100000,"cursor_mode":"baked"})
    }
    fn event(seq: u64, payload: serde_json::Value) -> serde_json::Value {
        json!({"record":"event","version":2,"seq":seq,"t_us":5000,
            "geometry_id":"g1","norm_x":-0.25,"norm_y":1.5,"inside_source":false,"payload":payload})
    }
    #[test]
    fn preserves_unclamped_positions_transitions_scroll_and_gaps() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = NativeMouseLogger::create(dir.path()).unwrap();
        logger.append(&geometry().to_string()).unwrap();
        for (seq, payload) in [json!({"kind":"move"}),
            json!({"kind":"button_down","button":4}), json!({"kind":"button_up","button":4}),
            json!({"kind":"scroll","delta_x":-1.5,"delta_y":2.0,"units":"pixels","precise":true,"phase":1,"momentum_phase":0}),
            json!({"kind":"gap","reason":"queue_overflow","start_us":4000,"end_us":5000,"dropped_events":3})].into_iter().enumerate() {
            logger.append(&event(seq as u64, payload).to_string()).unwrap();
        }
        logger.finish().unwrap();
        let text = std::fs::read_to_string(dir.path().join("telemetry/events.jsonl")).unwrap();
        let events: Vec<MouseEvent> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(events.len(), 5);
        assert_eq!(events[0].norm_x, Some(-0.25));
        assert!(matches!(
            events[1].payload,
            MousePayload::ButtonDown { button: 4 }
        ));
        assert!(matches!(
            events[4].payload,
            MousePayload::Gap {
                dropped_events: 3,
                ..
            }
        ));
        assert!(!text.contains("\"record\""));
        assert!(NativeMouseLogger::create(dir.path()).is_err());
    }
    #[test]
    fn rejects_bad_versions_geometry_sequences_and_inconsistent_coordinates() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = NativeMouseLogger::create(dir.path()).unwrap();
        let valid = event(0, json!({"kind":"move"}));
        assert!(logger.append(&valid.to_string()).is_err());
        logger.append(&geometry().to_string()).unwrap();
        for (key, value) in [
            ("version", json!(1)),
            ("seq", json!(3)),
            ("geometry_id", json!("missing")),
            ("inside_source", json!(true)),
        ] {
            let mut invalid = valid.clone();
            invalid[key] = value;
            assert!(logger.append(&invalid.to_string()).is_err());
        }
        logger.append(&valid.to_string()).unwrap();
        assert!(logger.append(&valid.to_string()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_telemetry_directory() {
        let dir = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(external.path(), dir.path().join("telemetry")).unwrap();
        assert!(NativeMouseLogger::create(dir.path()).is_err());
        assert!(!external.path().join("events.jsonl").exists());
    }

    #[test]
    fn permission_payload_is_boolean_pair_not_capture_bundle() {
        let denied = serde_json::to_value(MouseTelemetryPermission::macos(false)).unwrap();
        assert_eq!(denied["supported"], true);
        assert_eq!(denied["authorized"], false);
        assert!(denied.get("screenRecording").is_none());
        assert!(denied.get("screen_recording").is_none());
        assert!(denied.get("camera").is_none());
        assert!(denied.get("microphone").is_none());
        let unsupported = serde_json::to_value(MouseTelemetryPermission::unsupported()).unwrap();
        assert_eq!(unsupported, json!({"supported": false, "authorized": false}));
    }

    #[test]
    fn permission_and_health_gaps_round_trip_without_geometry_or_clicks() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = NativeMouseLogger::create(dir.path()).unwrap();
        logger.append(&geometry().to_string()).unwrap();
        for (seq, reason) in [
            GAP_INITIAL_BUTTON_STATE_UNKNOWN,
            GAP_INPUT_MONITORING_UNAVAILABLE,
            GAP_INPUT_MONITORING_REVOKED,
            GAP_EVENT_TAP_UNAVAILABLE,
            GAP_EVENT_TAP_DISABLED,
            GAP_UNSUPPORTED_SOURCE_GEOMETRY,
            GAP_QUEUE_OVERFLOW,
            GAP_GEOMETRY_CHANGED,
        ]
        .into_iter()
        .enumerate()
        {
            let t = 1_000 + seq as u64;
            let gap = json!({
                "record":"event","version":2,"seq":seq,"t_us":t,
                "payload":{"kind":"gap","reason":reason,"start_us":t.saturating_sub(100_000),"end_us":t,"dropped_events":0}
            });
            logger.append(&gap.to_string()).unwrap();
        }
        logger.finish().unwrap();
        let text = std::fs::read_to_string(dir.path().join("telemetry/events.jsonl")).unwrap();
        assert!(!text.contains("button_down"));
        assert!(!text.contains("\"kind\":\"click\""));
        for reason in [
            GAP_INPUT_MONITORING_UNAVAILABLE,
            GAP_INPUT_MONITORING_REVOKED,
            GAP_QUEUE_OVERFLOW,
        ] {
            assert!(text.contains(reason));
        }
    }

    #[test]
    fn rejects_tighter_geometry_sampling_and_replace_cursor_mode() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = NativeMouseLogger::create(dir.path()).unwrap();
        let mut tight = geometry();
        tight["sampling_interval_us"] = json!(1_000);
        assert!(logger.append(&tight.to_string()).is_err());
        let mut replace = geometry();
        replace["cursor_mode"] = json!("replace");
        assert!(logger.append(&replace.to_string()).is_err());
        logger.append(&geometry().to_string()).unwrap();
        let geo_text = std::fs::read_to_string(dir.path().join("telemetry/geometry.jsonl")).unwrap();
        assert!(geo_text.contains("\"sampling_interval_us\":100000"));
        assert!(geo_text.contains("\"cursor_mode\":\"baked\""));
    }
}
