use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryKind {
    Move,
    Down,
    Up,
    Click,
    Scroll,
}

/// Telemetry event format conforming to Section 3.2 of the Master Plan.
/// High-frequency cursor movement and click metadata logged out-of-band.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TelemetryEvent {
    pub version: u32,
    pub seq: u64,
    pub t_us: u64,
    pub geometry_id: String,
    pub kind: TelemetryKind,
    pub norm_x: f32,
    pub norm_y: f32,
    pub inside_source: bool,
    pub visible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_id: Option<String>,
}

impl TelemetryEvent {
    pub const CURRENT_VERSION: u32 = 1;

    pub fn new(
        seq: u64,
        t_us: u64,
        geometry_id: String,
        kind: TelemetryKind,
        norm_x: f32,
        norm_y: f32,
        inside_source: bool,
        visible: bool,
        cursor_id: Option<String>,
    ) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            seq,
            t_us,
            geometry_id,
            kind,
            norm_x,
            norm_y,
            inside_source,
            visible,
            cursor_id,
        }
    }
}

/// Geometry revision record capturing source bounds, crop, scale factor, and physical dimensions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeometryRecord {
    pub geometry_id: String,
    pub t_us: u64,
    pub physical_width: u32,
    pub physical_height: u32,
    pub scale_factor: f32,
    pub crop_x: u32,
    pub crop_y: u32,
    pub crop_width: u32,
    pub crop_height: u32,
}
