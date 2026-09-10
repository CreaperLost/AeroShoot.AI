//! Native preview surface session. Geometry and lifetime are owned here;
//! pixels are presented only through the platform adapter, never Tauri IPC.
use serde::{Deserialize, Serialize};

pub const ARRANGEMENT: &str = "child_overlay";
pub const MAX_VIEWPORT: f64 = 8_192.0;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewViewport {
    pub window_label: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub backing_scale: f64,
    pub visible: bool,
    pub occluded: bool,
    pub revision: u64,
    #[serde(default)]
    pub generation: u64,
    #[serde(default)]
    pub clip: Option<[f64; 4]>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreviewHitMode {
    Consume,
    Circle,
    PassThrough,
    CirclePassThrough,
    SquirclePassThrough,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewStatus {
    pub attached: bool,
    pub window_label: Option<String>,
    pub generation: u64,
    pub layout_revision: u64,
    pub arrangement: String,
    pub supported: bool,
    pub presented_kind: String,
    pub copies_per_present: u32,
    pub copies: u64,
    pub presented_bytes: u64,
    pub backing_scale: f64,
    pub physical: Option<PhysicalRect>,
    pub visible: bool,
    pub occluded: bool,
    pub hit_mode: PreviewHitMode,
    pub diagnostics: Vec<String>,
}

impl PreviewViewport {
    pub fn physical(&self) -> Result<PhysicalRect, String> {
        validate_viewport(self)?;
        let scale = self.backing_scale;
        Ok(PhysicalRect {
            x: (self.x * scale).round() as i32,
            y: (self.y * scale).round() as i32,
            width: (self.width * scale).round() as u32,
            height: (self.height * scale).round() as u32,
        })
    }
}

pub fn validate_viewport(viewport: &PreviewViewport) -> Result<(), String> {
    if viewport.window_label.trim().is_empty() {
        return Err("Preview window label is required".into());
    }
    if [
        viewport.x,
        viewport.y,
        viewport.width,
        viewport.height,
        viewport.backing_scale,
    ]
    .iter()
    .any(|v| !v.is_finite())
        || viewport
            .clip
            .is_some_and(|c| c.iter().any(|v| !v.is_finite()) || c[2] < 0.0 || c[3] < 0.0)
        || viewport.width <= 0.0
        || viewport.height <= 0.0
        || viewport.backing_scale <= 0.0
        || viewport.width > MAX_VIEWPORT
        || viewport.height > MAX_VIEWPORT
        || viewport.backing_scale > 8.0
    {
        return Err("Invalid preview viewport".into());
    }
    Ok(())
}

/// Circle hit region used by the HUD spike: corners pass through to whatever is below.
pub fn circle_consumes(width: f64, height: f64, x: f64, y: f64) -> bool {
    let rx = width / 2.0;
    let ry = height / 2.0;
    if rx <= 0.0 || ry <= 0.0 {
        return false;
    }
    let dx = (x - rx) / rx;
    let dy = (y - ry) / ry;
    dx * dx + dy * dy <= 1.0
}

pub struct PreviewOwner {
    generation: u64,
    window_label: Option<String>,
    viewport: Option<PreviewViewport>,
    attached: bool,
    native: Option<std::ptr::NonNull<std::ffi::c_void>>,
    presented_kind: String,
    copies: u64,
    presented_bytes: u64,
    hit_mode: PreviewHitMode,
    supported: bool,
}

impl PreviewOwner {
    pub fn new() -> Self {
        Self {
            generation: 0,
            window_label: None,
            viewport: None,
            attached: false,
            native: None,
            presented_kind: "none".into(),
            copies: 0,
            presented_bytes: 0,
            hit_mode: PreviewHitMode::Consume,
            supported: cfg!(target_os = "macos"),
        }
    }

    pub fn status(&self) -> PreviewStatus {
        PreviewStatus {
            attached: self.attached,
            window_label: self.window_label.clone(),
            generation: self.generation,
            layout_revision: self.viewport.as_ref().map(|v| v.revision).unwrap_or(0),
            arrangement: ARRANGEMENT.into(),
            supported: self.supported,
            presented_kind: self.presented_kind.clone(),
            copies_per_present: 1,
            copies: self.copies,
            presented_bytes: self.presented_bytes,
            backing_scale: self
                .viewport
                .as_ref()
                .map(|v| v.backing_scale)
                .unwrap_or(1.0),
            physical: self.viewport.as_ref().and_then(|v| v.physical().ok()),
            visible: self.viewport.as_ref().map(|v| v.visible).unwrap_or(false),
            occluded: self.viewport.as_ref().map(|v| v.occluded).unwrap_or(false),
            hit_mode: self.hit_mode,
            diagnostics: Vec::new(),
        }
    }

    pub fn attach(
        &mut self,
        window_label: String,
        hit_mode: PreviewHitMode,
        native_window: Option<*mut std::ffi::c_void>,
    ) -> Result<PreviewStatus, String> {
        if !self.supported {
            return Err("Native preview is not implemented on this platform".into());
        }
        self.detach();
        self.generation = self.generation.saturating_add(1).max(1);
        self.window_label = Some(window_label);
        self.hit_mode = hit_mode;
        self.presented_kind = "none".into();
        self.copies = 0;
        self.presented_bytes = 0;
        if let Some(window) = native_window {
            let handle = super::native::attach(window, self.generation)?;
            super::native::set_hit_mode(handle, hit_mode)?;
            self.native = Some(handle);
            self.attached = true;
        } else {
            self.attached = true;
        }
        Ok(self.status())
    }

    pub fn layout(&mut self, viewport: PreviewViewport) -> Result<PreviewStatus, String> {
        self.ensure_attached(&viewport.window_label)?;
        self.ensure_generation(viewport.generation)?;
        validate_viewport(&viewport)?;
        if let Some(current) = &self.viewport {
            if viewport.revision < current.revision {
                return Err("Stale preview layout revision".into());
            }
        }
        if let Some(handle) = self.native {
            super::native::set_geometry(handle, &viewport, self.generation)?;
        }
        self.viewport = Some(viewport);
        Ok(self.status())
    }

    pub fn present_fixed(
        &mut self,
        r: f32,
        g: f32,
        b: f32,
        generation: u64,
    ) -> Result<PreviewStatus, String> {
        self.ensure_open()?;
        self.ensure_generation(generation)?;
        if let Some(handle) = self.native {
            super::native::present_fixed(handle, r, g, b, self.generation)?;
        }
        self.presented_kind = "fixed".into();
        self.copies = self.copies.saturating_add(1);
        self.presented_bytes = 0;
        Ok(self.status())
    }

    pub fn present_fixture(
        &mut self,
        path: &str,
        generation: u64,
    ) -> Result<PreviewStatus, String> {
        self.ensure_open()?;
        self.ensure_generation(generation)?;
        if path.trim().is_empty() {
            return Err("Decode fixture path is required".into());
        }
        let handle = self
            .native
            .ok_or("Native preview view is required to decode a fixture")?;
        super::native::decode_present(handle, path, self.generation)?;
        self.presented_kind = "decoded".into();
        self.copies = self.copies.saturating_add(1);
        Ok(self.status())
    }

    pub fn present_frame(
        &mut self,
        frame: &crate::media::VideoFrame,
        generation: u64,
    ) -> Result<(), String> {
        self.ensure_open()?;
        self.ensure_generation(generation)?;
        let native = self.native.ok_or("No native surface")?;
        super::native::present_frame(native, frame, generation)?;
        self.presented_kind = "project".into();
        self.copies += 1;
        self.presented_bytes = frame.data.len() as u64;
        Ok(())
    }

    pub fn hit_test(&self, x: f64, y: f64) -> bool {
        let Some(viewport) = &self.viewport else {
            return false;
        };
        if !viewport.visible || viewport.occluded {
            return false;
        }
        if let Some([cx, cy, w, h]) = viewport.clip {
            if x < cx || y < cy || x >= cx + w || y >= cy + h {
                return false;
            }
        }
        match self.hit_mode {
            PreviewHitMode::Consume => {
                x >= 0.0 && y >= 0.0 && x < viewport.width && y < viewport.height
            }
            PreviewHitMode::Circle => circle_consumes(viewport.width, viewport.height, x, y),
            PreviewHitMode::PassThrough
            | PreviewHitMode::CirclePassThrough
            | PreviewHitMode::SquirclePassThrough => false,
        }
    }

    pub fn detach(&mut self) {
        if let Some(handle) = self.native.take() {
            super::native::detach(handle);
        }
        self.attached = false;
        self.window_label = None;
        self.viewport = None;
        self.presented_kind = "none".into();
        self.generation = self.generation.saturating_add(1);
    }

    fn ensure_open(&self) -> Result<(), String> {
        if !self.attached {
            return Err("Preview is not attached".into());
        }
        Ok(())
    }

    fn ensure_attached(&self, window_label: &str) -> Result<(), String> {
        self.ensure_open()?;
        if self.window_label.as_deref() != Some(window_label) {
            return Err("Stale preview window label".into());
        }
        Ok(())
    }

    fn ensure_generation(&self, generation: u64) -> Result<(), String> {
        if generation != 0 && generation != self.generation {
            return Err("Stale preview generation".into());
        }
        Ok(())
    }
}

impl Default for PreviewOwner {
    fn default() -> Self {
        Self::new()
    }
}

// The adapter pointer is only used through Swift `onMain`; Mutex serializes Rust access.
unsafe impl Send for PreviewOwner {}
unsafe impl Sync for PreviewOwner {}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport(revision: u64) -> PreviewViewport {
        PreviewViewport {
            window_label: "main".into(),
            x: 10.0,
            y: 20.0,
            width: 320.0,
            height: 180.0,
            backing_scale: 2.0,
            visible: true,
            occluded: false,
            revision,
            generation: 0,
            clip: None,
        }
    }

    #[test]
    fn logical_points_map_to_physical_pixels() {
        let physical = viewport(1).physical().unwrap();
        assert_eq!(physical.x, 20);
        assert_eq!(physical.y, 40);
        assert_eq!(physical.width, 640);
        assert_eq!(physical.height, 360);
    }

    #[test]
    fn stale_layout_and_generation_are_rejected() {
        let mut owner = PreviewOwner::new();
        owner
            .attach("main".into(), PreviewHitMode::Consume, None)
            .unwrap();
        owner.layout(viewport(2)).unwrap();
        assert!(owner.layout(viewport(1)).unwrap_err().contains("Stale"));
        let generation = owner.status().generation;
        owner.present_fixed(0.1, 0.2, 0.3, generation).unwrap();
        assert!(owner
            .present_fixed(1.0, 0.0, 0.0, generation + 9)
            .unwrap_err()
            .contains("Stale"));
        let json = serde_json::to_value(owner.status()).unwrap();
        assert!(json.get("pixels").is_none());
        assert!(json.get("samples").is_none());
        assert_eq!(json["arrangement"], "child_overlay");
        assert_eq!(json["copiesPerPresent"], 1);
        assert_eq!(owner.status().presented_kind, "fixed");
        assert!(owner
            .present_fixture("/tmp/missing-f1.mp4", generation)
            .unwrap_err()
            .contains("Native preview view"));
        owner.detach();
        owner
            .attach("main".into(), PreviewHitMode::Consume, None)
            .unwrap();
        assert_ne!(owner.status().generation, generation);
        owner.detach();
        assert!(!owner.status().attached);
    }

    #[test]
    fn circle_hit_mode_passes_transparent_corners() {
        let mut owner = PreviewOwner::new();
        owner
            .attach("camera_overlay".into(), PreviewHitMode::Circle, None)
            .unwrap();
        owner
            .layout(PreviewViewport {
                window_label: "camera_overlay".into(),
                x: 0.0,
                y: 0.0,
                width: 120.0,
                height: 120.0,
                backing_scale: 1.0,
                visible: true,
                occluded: false,
                revision: 1,
                generation: 0,
                clip: None,
            })
            .unwrap();
        assert!(owner.hit_test(60.0, 60.0));
        assert!(!owner.hit_test(1.0, 1.0));
        owner.detach();
        owner
            .attach("camera_overlay".into(), PreviewHitMode::Circle, None)
            .unwrap();
        owner.detach();
        assert!(!owner.status().attached);
    }
}
