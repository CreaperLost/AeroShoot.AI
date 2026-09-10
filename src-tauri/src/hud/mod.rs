//! HUD-only window identity and Rust-owned camera/HUD settings.
//!
//! The studio (`main`) and overlay (`camera_overlay`) share one revisioned
//! snapshot. Zustand is a view. Closing the HUD must not stop recording or
//! start an independent capture session.
use crate::playback::PreviewHitMode;
use serde::{Deserialize, Serialize};

pub const STUDIO_WINDOW_LABEL: &str = "main";
pub const HUD_WINDOW_LABEL: &str = "camera_overlay";
pub const HUD_SETTINGS_EVENT: &str = "hud-settings";
const MAX_EVENT_LOG: usize = 32;
const MAX_CAMERAS: usize = 64;
const MAX_DIAGNOSTICS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UiRootKind {
    Studio,
    Hud,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WindowIdentity {
    pub label: String,
    pub ui_root: Option<UiRootKind>,
    pub rejected: bool,
}

/// Verified window labels only. Unknown identities must not receive the studio.
pub fn resolve_ui_root(label: &str) -> Result<UiRootKind, String> {
    match label {
        STUDIO_WINDOW_LABEL => Ok(UiRootKind::Studio),
        HUD_WINDOW_LABEL => Ok(UiRootKind::Hud),
        other => Err(format!(
            "Rejected window identity '{other}': not a privileged AeroShoot root"
        )),
    }
}

pub fn window_identity_from_label(label: &str) -> WindowIdentity {
    match resolve_ui_root(label) {
        Ok(kind) => WindowIdentity {
            label: label.to_string(),
            ui_root: Some(kind),
            rejected: false,
        },
        Err(_) => WindowIdentity {
            label: label.to_string(),
            ui_root: None,
            rejected: true,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HudShape {
    Rect,
    Circle,
    Squircle,
    #[serde(rename = "rect_16_9")]
    Rect16x9,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HudSize {
    Sm,
    Md,
    Lg,
    Xl,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HudSettings {
    pub enabled: bool,
    pub shape: HudShape,
    pub size: HudSize,
    pub mirror: bool,
    pub border_color: String,
    pub border_width: u32,
    pub shadow: bool,
}

impl Default for HudSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            shape: HudShape::Circle,
            size: HudSize::Md,
            mirror: true,
            border_color: "#6366f1".into(),
            border_width: 3,
            shadow: false,
        }
    }
}

impl HudSettings {
    pub fn hit_mode(&self) -> PreviewHitMode {
        match self.shape {
            HudShape::Circle => PreviewHitMode::CirclePassThrough,
            HudShape::Squircle => PreviewHitMode::SquirclePassThrough,
            HudShape::Rect | HudShape::Rect16x9 => PreviewHitMode::PassThrough,
        }
    }

    pub fn window_size_css_px(&self) -> (u32, u32) {
        let edge = match self.size {
            HudSize::Sm => 160,
            HudSize::Md => 240,
            HudSize::Lg => 320,
            HudSize::Xl => 400,
        };
        match self.shape {
            HudShape::Rect16x9 => (edge + 80, ((edge + 80) * 9) / 16),
            _ => (edge, edge),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HudSettingsPatch {
    pub enabled: Option<bool>,
    pub shape: Option<HudShape>,
    pub size: Option<HudSize>,
    pub mirror: Option<bool>,
    pub border_color: Option<String>,
    pub border_width: Option<u32>,
    pub shadow: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HudCameraInfo {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HudSnapshot {
    pub revision: u64,
    pub settings: HudSettings,
    pub camera_id: Option<String>,
    pub camera_name: Option<String>,
    pub camera_available: bool,
    pub hud_attached: bool,
    pub hud_visible: bool,
    pub exclusion_established: bool,
    pub hide_during_record: bool,
    pub session_recording: bool,
    pub capture_session_alive: bool,
    pub started_independent_capture: bool,
    pub hit_mode: PreviewHitMode,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HudEvent {
    pub revision: u64,
    pub snapshot: HudSnapshot,
}

#[derive(Clone, Debug)]
pub struct HudSubscriber {
    pub last_seen: u64,
    pub view: HudSettings,
    pub recovered_from_snapshot: bool,
}

impl HudSubscriber {
    pub fn new() -> Self {
        Self {
            last_seen: 0,
            view: HudSettings::default(),
            recovered_from_snapshot: false,
        }
    }

    pub fn reconnect(&mut self, snapshot: &HudSnapshot) {
        self.last_seen = snapshot.revision;
        self.view = snapshot.settings.clone();
        self.recovered_from_snapshot = true;
    }

    /// Apply a revisioned event. A gap or out-of-order revision fetches a snapshot.
    pub fn on_event(&mut self, event: &HudEvent, snapshot: &HudSnapshot) {
        let expected = self.last_seen.saturating_add(1);
        if event.revision != expected {
            self.reconnect(snapshot);
            return;
        }
        self.last_seen = event.revision;
        self.view = event.snapshot.settings.clone();
        self.recovered_from_snapshot = false;
    }
}

impl Default for HudSubscriber {
    fn default() -> Self {
        Self::new()
    }
}

pub struct HudOwner {
    revision: u64,
    settings: HudSettings,
    camera_id: Option<String>,
    camera_name: Option<String>,
    known_cameras: Vec<HudCameraInfo>,
    camera_available: bool,
    /// Recording filters exclude this process for display capture and include
    /// only the chosen external source for window/application capture.
    exclusion_established: bool,
    hud_attached: bool,
    requested_visible: bool,
    started_independent_capture: bool,
    events: Vec<HudEvent>,
}

impl HudOwner {
    pub fn new() -> Self {
        Self {
            revision: 0,
            settings: HudSettings::default(),
            camera_id: None,
            camera_name: None,
            known_cameras: Vec::new(),
            camera_available: false,
            // Every recording filter captures either a single external window/app
            // or excludes this process from display capture by PID.
            exclusion_established: true,
            hud_attached: false,
            requested_visible: false,
            started_independent_capture: false,
            events: Vec::new(),
        }
    }

    pub fn snapshot(&self, session_recording: bool, capture_session_alive: bool) -> HudSnapshot {
        let hide_during_record = false;
        let hud_visible = self.desired_visible(session_recording);
        let mut diagnostics = Vec::new();
        if self.camera_id.is_some() && !self.camera_available {
            diagnostics.push("Selected camera is unavailable".into());
        }
        if diagnostics.len() > MAX_DIAGNOSTICS {
            diagnostics.truncate(MAX_DIAGNOSTICS);
        }
        HudSnapshot {
            revision: self.revision,
            settings: self.settings.clone(),
            camera_id: self.camera_id.clone(),
            camera_name: self.camera_name.clone(),
            camera_available: self.camera_available,
            hud_attached: self.hud_attached,
            hud_visible,
            exclusion_established: self.exclusion_established,
            hide_during_record,
            session_recording,
            capture_session_alive,
            started_independent_capture: self.started_independent_capture,
            hit_mode: self.settings.hit_mode(),
            diagnostics,
        }
    }

    pub fn desired_visible(&self, session_recording: bool) -> bool {
        if !session_recording || !self.settings.enabled || !self.requested_visible {
            return false;
        }
        true
    }

    pub fn update(
        &mut self,
        expected_revision: u64,
        patch: HudSettingsPatch,
        session_recording: bool,
        capture_session_alive: bool,
    ) -> Result<HudSnapshot, String> {
        if expected_revision != self.revision {
            return Err("Stale HUD settings revision".into());
        }
        let mut next = self.settings.clone();
        if let Some(enabled) = patch.enabled {
            next.enabled = enabled;
        }
        if let Some(shape) = patch.shape {
            next.shape = shape;
        }
        if let Some(size) = patch.size {
            next.size = size;
        }
        if let Some(mirror) = patch.mirror {
            next.mirror = mirror;
        }
        if let Some(shadow) = patch.shadow {
            next.shadow = shadow;
        }
        if let Some(border_width) = patch.border_width {
            if border_width > 8 {
                return Err("HUD border width must be 0–8 px".into());
            }
            next.border_width = border_width;
        }
        if let Some(border_color) = patch.border_color {
            validate_color(&border_color)?;
            next.border_color = border_color;
        }
        if next == self.settings {
            return Ok(self.snapshot(session_recording, capture_session_alive));
        }
        self.settings = next;
        self.bump(session_recording, capture_session_alive)
    }

    pub fn reconcile_cameras(
        &mut self,
        cameras: Vec<HudCameraInfo>,
        selected_camera_id: Option<String>,
        session_recording: bool,
        capture_session_alive: bool,
    ) -> Result<HudSnapshot, String> {
        if cameras.len() > MAX_CAMERAS {
            return Err("Too many cameras".into());
        }
        for camera in &cameras {
            if camera.id.is_empty() || camera.id.len() > 256 {
                return Err("Invalid camera id".into());
            }
        }
        self.known_cameras = cameras;
        let previous_available = self.camera_available;
        let previous_id = self.camera_id.clone();
        if let Some(selected) = selected_camera_id {
            if selected.is_empty() || selected.len() > 256 {
                return Err("Invalid selected camera id".into());
            }
            self.camera_id = Some(selected);
        }
        if let Some(id) = &self.camera_id {
            if let Some(found) = self.known_cameras.iter().find(|c| &c.id == id) {
                self.camera_available = true;
                self.camera_name = Some(found.name.clone());
            } else {
                self.camera_available = false;
                self.camera_name = None;
            }
        } else if let Some(first) = self.known_cameras.first() {
            self.camera_id = Some(first.id.clone());
            self.camera_name = Some(first.name.clone());
            self.camera_available = true;
        } else {
            self.camera_available = false;
            self.camera_name = None;
        }
        let changed = previous_available != self.camera_available || previous_id != self.camera_id;
        if changed {
            self.bump(session_recording, capture_session_alive)
        } else {
            Ok(self.snapshot(session_recording, capture_session_alive))
        }
    }

    pub fn attach_preview(
        &mut self,
        window_label: &str,
        session_recording: bool,
        capture_session_alive: bool,
    ) -> Result<HudSnapshot, String> {
        if window_label != HUD_WINDOW_LABEL {
            return Err(format!(
                "HUD preview can only attach to '{HUD_WINDOW_LABEL}'"
            ));
        }
        self.hud_attached = true;
        self.started_independent_capture = false;
        self.bump(session_recording, capture_session_alive)
    }

    /// Detach the HUD surface. Must not stop recording or drop the capture session.
    pub fn close(
        &mut self,
        session_recording: bool,
        capture_session_alive: bool,
    ) -> HudSnapshot {
        self.hud_attached = false;
        self.requested_visible = false;
        self.started_independent_capture = false;
        match self.bump(session_recording, capture_session_alive) {
            Ok(snapshot) => snapshot,
            Err(_) => self.snapshot(session_recording, capture_session_alive),
        }
    }

    pub fn set_requested_visible(
        &mut self,
        visible: bool,
        session_recording: bool,
        capture_session_alive: bool,
    ) -> Result<HudSnapshot, String> {
        if self.requested_visible == visible {
            return Ok(self.snapshot(session_recording, capture_session_alive));
        }
        self.requested_visible = visible;
        self.bump(session_recording, capture_session_alive)
    }

    pub fn last_event(&self) -> Option<&HudEvent> {
        self.events.last()
    }

    pub fn events(&self) -> &[HudEvent] {
        &self.events
    }

    fn bump(
        &mut self,
        session_recording: bool,
        capture_session_alive: bool,
    ) -> Result<HudSnapshot, String> {
        self.revision = self.revision.checked_add(1).ok_or("Revision overflow")?;
        let snapshot = self.snapshot(session_recording, capture_session_alive);
        self.events.push(HudEvent {
            revision: self.revision,
            snapshot: snapshot.clone(),
        });
        if self.events.len() > MAX_EVENT_LOG {
            self.events.remove(0);
        }
        Ok(snapshot)
    }
}

impl Default for HudOwner {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_color(value: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    if bytes.len() != 7 || bytes[0] != b'#' {
        return Err("HUD border color must be #RRGGBB".into());
    }
    if !bytes[1..].iter().all(|b| b.is_ascii_hexdigit()) {
        return Err("HUD border color must be #RRGGBB".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_identity_maps_only_verified_labels() {
        assert_eq!(resolve_ui_root("main").unwrap(), UiRootKind::Studio);
        assert_eq!(
            resolve_ui_root("camera_overlay").unwrap(),
            UiRootKind::Hud
        );
        assert!(resolve_ui_root("other").unwrap_err().contains("Rejected"));
        assert!(resolve_ui_root("").unwrap_err().contains("Rejected"));
        let rejected = window_identity_from_label("webview-2");
        assert!(rejected.rejected);
        assert!(rejected.ui_root.is_none());
        let hud = window_identity_from_label(HUD_WINDOW_LABEL);
        assert!(!hud.rejected);
        assert_eq!(hud.ui_root, Some(UiRootKind::Hud));
    }

    #[test]
    fn stale_revision_is_rejected_and_both_windows_converge() {
        let mut owner = HudOwner::new();
        let first = owner
            .update(
                0,
                HudSettingsPatch {
                    shape: Some(HudShape::Squircle),
                    ..HudSettingsPatch::default()
                },
                false,
                false,
            )
            .unwrap();
        assert_eq!(first.revision, 1);
        assert_eq!(first.settings.shape, HudShape::Squircle);
        let err = owner
            .update(
                0,
                HudSettingsPatch {
                    mirror: Some(false),
                    ..HudSettingsPatch::default()
                },
                false,
                false,
            )
            .unwrap_err();
        assert!(err.contains("Stale"));
        let studio = owner
            .update(
                1,
                HudSettingsPatch {
                    size: Some(HudSize::Lg),
                    ..HudSettingsPatch::default()
                },
                false,
                false,
            )
            .unwrap();
        let overlay = owner.snapshot(false, false);
        assert_eq!(studio.revision, overlay.revision);
        assert_eq!(studio.settings.size, HudSize::Lg);
        assert_eq!(overlay.settings.size, HudSize::Lg);
    }

    #[test]
    fn reconnect_and_dropped_events_recover_from_snapshot() {
        let mut owner = HudOwner::new();
        owner
            .update(
                0,
                HudSettingsPatch {
                    mirror: Some(false),
                    ..HudSettingsPatch::default()
                },
                false,
                false,
            )
            .unwrap();
        owner
            .update(
                1,
                HudSettingsPatch {
                    size: Some(HudSize::Xl),
                    ..HudSettingsPatch::default()
                },
                false,
                false,
            )
            .unwrap();
        let mut reconnecting = HudSubscriber::new();
        reconnecting.reconnect(&owner.snapshot(false, false));
        assert_eq!(reconnecting.last_seen, 2);
        assert_eq!(reconnecting.view.size, HudSize::Xl);
        assert!(!reconnecting.view.mirror);

        let mut live = HudSubscriber::new();
        live.on_event(owner.events().first().unwrap(), &owner.snapshot(false, false));
        assert_eq!(live.last_seen, 1);
        assert!(!live.recovered_from_snapshot);
        // Drop revision 2; a later event (if any) or the current snapshot recovers.
        let dropped = owner.snapshot(false, false);
        live.on_event(
            &HudEvent {
                revision: 9,
                snapshot: dropped.clone(),
            },
            &dropped,
        );
        assert!(live.recovered_from_snapshot);
        assert_eq!(live.last_seen, 2);
        assert_eq!(live.view.size, HudSize::Xl);
    }

    #[test]
    fn camera_removal_is_visible_and_close_does_not_start_capture() {
        let mut owner = HudOwner::new();
        owner
            .reconcile_cameras(
                vec![HudCameraInfo {
                    id: "cam-1".into(),
                    name: "FaceTime HD".into(),
                }],
                None,
                false,
                true,
            )
            .unwrap();
        assert!(owner.snapshot(false, true).camera_available);
        owner
            .attach_preview(HUD_WINDOW_LABEL, true, true)
            .unwrap();
        let removed = owner
            .reconcile_cameras(Vec::new(), None, true, true)
            .unwrap();
        assert!(!removed.camera_available);
        assert_eq!(removed.camera_id.as_deref(), Some("cam-1"));
        assert!(removed
            .diagnostics
            .iter()
            .any(|d| d.contains("unavailable")));
        let closed = owner.close(true, true);
        assert!(!closed.hud_attached);
        assert!(closed.capture_session_alive);
        assert!(closed.session_recording);
        assert!(!closed.started_independent_capture);
        assert!(resolve_ui_root("camera_overlay").is_ok());
    }

    #[test]
    fn hud_is_hidden_while_idle_and_visible_only_for_recording() {
        let mut owner = HudOwner::new();
        owner
            .update(
                0,
                HudSettingsPatch {
                    enabled: Some(true),
                    ..HudSettingsPatch::default()
                },
                false,
                false,
            )
            .unwrap();
        owner
            .set_requested_visible(true, false, false)
            .unwrap();
        let idle = owner.snapshot(false, false);
        assert!(!idle.hud_visible);
        assert!(idle.exclusion_established);
        assert!(!idle.hide_during_record);
        let recording = owner.snapshot(true, true);
        assert!(recording.hud_visible);
        let json = serde_json::to_value(&recording).unwrap();
        assert_eq!(json["hideDuringRecord"], false);
        assert_eq!(json["exclusionEstablished"], true);
        assert!(json.get("pixels").is_none());
    }

    #[test]
    fn hud_preview_rejects_studio_window_label() {
        let mut owner = HudOwner::new();
        assert!(owner
            .attach_preview(STUDIO_WINDOW_LABEL, false, false)
            .unwrap_err()
            .contains("camera_overlay"));
        assert!(!owner.snapshot(false, false).hud_attached);
    }
}
