//! Deterministic telemetry-driven zoom suggestions (Z1).
//!
//! Interest comes from known primary button-down transitions, v1 click records
//! that are not paired with downs, and cursor dwell. Auxiliary buttons, off-source
//! coordinates, unsupported geometry, and uncertainty/gap intervals never become
//! targets. Camera viewport clamping does not rewrite recorded coordinates.
mod eval;

pub use eval::{
    clamp_camera, cubic_bezier_unit, evaluate_at_edited, evaluate_at_source, CameraTransform,
};

use crate::telemetry::reader::{CanonicalKind, TelemetryStream};
use crate::timeline::TimelineMapper;
use serde::{Deserialize, Serialize};

pub const ZOOM_GENERATION_VERSION: u32 = 1;
pub const MAX_SUGGESTIONS: usize = 512;
pub const MAX_PRIMARY_BUTTONS: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ZoomConfig {
    /// Must match [`ZOOM_GENERATION_VERSION`]. Unknown versions are rejected.
    pub generation_version: u32,
    pub dwell_radius_norm: f64,
    pub min_dwell_us: u64,
    pub cluster_radius_norm: f64,
    pub cluster_gap_us: u64,
    /// Primary downs inside this window merge. Auxiliary buttons never generate
    /// interest. V1 `click` records are used only when the stream has no downs.
    pub rapid_click_window_us: u64,
    pub min_hold_us: u64,
    pub transition_us: u64,
    pub max_scale: f64,
    pub click_scale: f64,
    pub dwell_scale: f64,
    pub geometry_uncertainty_us: u64,
    pub primary_buttons: Vec<u32>,
    pub viewport_margin: f64,
}

impl Default for ZoomConfig {
    fn default() -> Self {
        Self {
            generation_version: ZOOM_GENERATION_VERSION,
            dwell_radius_norm: 0.04,
            min_dwell_us: 700_000,
            cluster_radius_norm: 0.12,
            cluster_gap_us: 800_000,
            rapid_click_window_us: 350_000,
            min_hold_us: 1_200_000,
            transition_us: 400_000,
            max_scale: 2.0,
            click_scale: 2.0,
            dwell_scale: 1.5,
            geometry_uncertainty_us: 100_000,
            primary_buttons: vec![0],
            viewport_margin: 0.0,
        }
    }
}

impl ZoomConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.generation_version != ZOOM_GENERATION_VERSION {
            return Err(format!(
                "Unsupported zoom generation version {}",
                self.generation_version
            ));
        }
        let finite_positive = |name: &str, value: f64| {
            if value.is_finite() && value > 0.0 {
                Ok(())
            } else {
                Err(format!("{name} must be a finite positive number"))
            }
        };
        finite_positive("dwellRadiusNorm", self.dwell_radius_norm)?;
        finite_positive("clusterRadiusNorm", self.cluster_radius_norm)?;
        finite_positive("maxScale", self.max_scale)?;
        finite_positive("clickScale", self.click_scale)?;
        finite_positive("dwellScale", self.dwell_scale)?;
        if self.dwell_radius_norm > 1.0 || self.cluster_radius_norm > 1.0 {
            return Err("Spatial radii must be within one normalized screen".into());
        }
        if !(1.0..=8.0).contains(&self.max_scale) {
            return Err("maxScale must be between 1 and 8".into());
        }
        if self.click_scale > self.max_scale || self.dwell_scale > self.max_scale {
            return Err("Zoom scales cannot exceed maxScale".into());
        }
        if self.click_scale < 1.0 || self.dwell_scale < 1.0 {
            return Err("Zoom scales must be at least 1".into());
        }
        if self.min_dwell_us == 0 || self.min_hold_us == 0 || self.transition_us == 0 {
            return Err("Dwell, hold and transition durations must be nonzero".into());
        }
        if self.transition_us > 10_000_000 || self.min_hold_us > 60_000_000 {
            return Err("Hold/transition durations exceed supported bounds".into());
        }
        if self.primary_buttons.len() > MAX_PRIMARY_BUTTONS {
            return Err("Too many primary buttons".into());
        }
        if !self.viewport_margin.is_finite()
            || self.viewport_margin < 0.0
            || self.viewport_margin >= 0.5
        {
            return Err("viewportMargin must be in [0, 0.5)".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ZoomOrigin {
    Click,
    Dwell,
    Cluster,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EditedRange {
    pub start_us: u64,
    pub end_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ZoomSuggestion {
    pub id: String,
    pub source_start_us: u64,
    pub source_end_us: u64,
    pub center_x: f64,
    pub center_y: f64,
    pub scale: f64,
    pub transition_us: u64,
    pub origin: ZoomOrigin,
    pub contributing_event_seqs: Vec<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edited_ranges: Vec<EditedRange>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ZoomSource {
    Generated,
    Manual,
}

pub const MAX_ZOOMS: usize = 512;
pub const MAX_DISMISSED_ZOOMS: usize = 1_024;

/// Source-anchored zoom persisted in `project.json`. Regeneration never
/// overwrites an existing id or a dismissed generated suggestion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ZoomKeyframe {
    pub id: String,
    pub source_start_us: u64,
    pub source_end_us: u64,
    pub center_x: f64,
    pub center_y: f64,
    pub scale: f64,
    pub transition_us: u64,
    pub origin: ZoomOrigin,
    #[serde(default)]
    pub contributing_event_seqs: Vec<u64>,
    pub source: ZoomSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edited_ranges: Vec<EditedRange>,
}

impl ZoomKeyframe {
    pub fn from_suggestion(suggestion: ZoomSuggestion, source: ZoomSource) -> Self {
        Self {
            id: suggestion.id,
            source_start_us: suggestion.source_start_us,
            source_end_us: suggestion.source_end_us,
            center_x: suggestion.center_x,
            center_y: suggestion.center_y,
            scale: suggestion.scale,
            transition_us: suggestion.transition_us,
            origin: suggestion.origin,
            contributing_event_seqs: suggestion.contributing_event_seqs,
            source,
            edited_ranges: suggestion.edited_ranges,
        }
    }

    pub fn as_suggestion(&self) -> ZoomSuggestion {
        ZoomSuggestion {
            id: self.id.clone(),
            source_start_us: self.source_start_us,
            source_end_us: self.source_end_us,
            center_x: self.center_x,
            center_y: self.center_y,
            scale: self.scale,
            transition_us: self.transition_us,
            origin: self.origin,
            contributing_event_seqs: self.contributing_event_seqs.clone(),
            edited_ranges: self.edited_ranges.clone(),
        }
    }
}

pub fn validate_zooms(zooms: &[ZoomKeyframe]) -> Result<(), String> {
    if zooms.len() > MAX_ZOOMS {
        return Err("Too many zoom keyframes".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for zoom in zooms {
        if zoom.id.is_empty() || zoom.id.len() > 128 {
            return Err("Invalid zoom id".into());
        }
        if !seen.insert(&zoom.id) {
            return Err(format!("Duplicate zoom id {}", zoom.id));
        }
        if zoom.source_end_us <= zoom.source_start_us {
            return Err("Zoom must be a half-open source range".into());
        }
        if zoom.source_end_us > 9_007_199_254_740_991 {
            return Err("Zoom timestamp exceeds supported precision".into());
        }
        if !zoom.center_x.is_finite()
            || !zoom.center_y.is_finite()
            || !zoom.scale.is_finite()
            || !(0.0..=1.0).contains(&zoom.center_x)
            || !(0.0..=1.0).contains(&zoom.center_y)
            || !(1.0..=8.0).contains(&zoom.scale)
        {
            return Err("Zoom center/scale is out of range".into());
        }
        if zoom.transition_us == 0
            || zoom.transition_us >= zoom.source_end_us.saturating_sub(zoom.source_start_us)
        {
            return Err("Zoom transition must fit inside the source range".into());
        }
    }
    Ok(())
}

pub fn attach_zoom_edited_ranges(zooms: &mut [ZoomKeyframe], mapper: &TimelineMapper) {
    for zoom in zooms {
        zoom.edited_ranges = mapper
            .source_range_to_edited(zoom.source_start_us, zoom.source_end_us)
            .into_iter()
            .map(|(start_us, end_us)| EditedRange { start_us, end_us })
            .collect();
    }
}

pub fn eval_config_for(zooms: &[ZoomKeyframe]) -> ZoomConfig {
    let mut config = ZoomConfig::default();
    let max = zooms
        .iter()
        .map(|z| z.scale)
        .fold(config.max_scale, f64::max)
        .clamp(1.0, 8.0);
    config.max_scale = max;
    config
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ZoomGeneration {
    pub version: u32,
    pub config: ZoomConfig,
    pub suggestions: Vec<ZoomSuggestion>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone)]
struct InterestPoint {
    t_us: u64,
    end_us: u64,
    seq: u64,
    seqs: Vec<u64>,
    x: f64,
    y: f64,
    origin: ZoomOrigin,
}

struct Interval {
    start: u64,
    end: u64,
}

impl Interval {
    fn contains(&self, t: u64) -> bool {
        t >= self.start && t < self.end
    }
}

/// Build source-anchored zoom suggestions. Identical input and config yield
/// identical IDs, ordering and transforms.
pub fn generate_zoom_suggestions(
    stream: &TelemetryStream,
    config: &ZoomConfig,
) -> Result<ZoomGeneration, String> {
    config.validate()?;
    let mut diagnostics = stream.diagnostics.clone();
    let uncertainty = collect_uncertainty(stream, config);
    let ignore_v1_clicks = stream
        .events
        .iter()
        .any(|event| matches!(event.kind, CanonicalKind::ButtonDown { .. }));
    let interest = extract_interest(
        stream,
        config,
        &uncertainty,
        ignore_v1_clicks,
        &mut diagnostics,
    );
    let mut suggestions = cluster_interest(&interest, config);
    suggestions.truncate(MAX_SUGGESTIONS);
    if suggestions.is_empty()
        && !diagnostics
            .iter()
            .any(|d| d.contains("No zoom interest") || d.contains("No telemetry"))
    {
        diagnostics.push(
            "No zoom interest; telemetry is empty, denied, gapped, or has no usable targets".into(),
        );
    }
    diagnostics.truncate(256);
    Ok(ZoomGeneration {
        version: config.generation_version,
        config: config.clone(),
        suggestions,
        diagnostics,
    })
}

pub fn attach_edited_ranges(generation: &mut ZoomGeneration, mapper: &TimelineMapper) {
    for suggestion in &mut generation.suggestions {
        suggestion.edited_ranges = mapper
            .source_range_to_edited(suggestion.source_start_us, suggestion.source_end_us)
            .into_iter()
            .map(|(start_us, end_us)| EditedRange { start_us, end_us })
            .collect();
    }
}

fn collect_uncertainty(stream: &TelemetryStream, config: &ZoomConfig) -> Vec<Interval> {
    let mut intervals = Vec::new();
    for geometry in stream.geometries.values() {
        let span = geometry
            .sampling_interval_us
            .max(config.geometry_uncertainty_us);
        intervals.push(Interval {
            start: geometry.t_us,
            end: geometry.t_us.saturating_add(span),
        });
    }
    for event in &stream.events {
        if let CanonicalKind::Gap {
            start_us, end_us, ..
        } = event.kind
        {
            intervals.push(Interval {
                start: start_us,
                end: end_us.max(start_us.saturating_add(1)),
            });
        }
    }
    intervals.sort_by_key(|i| (i.start, i.end));
    intervals
}

fn in_uncertainty(intervals: &[Interval], t: u64) -> bool {
    intervals.iter().any(|interval| interval.contains(t))
}

fn is_primary(button: Option<u32>, config: &ZoomConfig) -> bool {
    match button {
        None => true,
        Some(button) => config.primary_buttons.contains(&button),
    }
}

fn usable_target(
    stream: &TelemetryStream,
    geometry_id: Option<&String>,
    x: f64,
    y: f64,
    inside: bool,
    t_us: u64,
    uncertainty: &[Interval],
) -> bool {
    if !inside || !x.is_finite() || !y.is_finite() {
        return false;
    }
    if !(0.0..1.0).contains(&x) || !(0.0..1.0).contains(&y) {
        return false;
    }
    if in_uncertainty(uncertainty, t_us) {
        return false;
    }
    let Some(id) = geometry_id else {
        return false;
    };
    match stream.geometries.get(id) {
        Some(geometry) if geometry.supported => true,
        _ => false,
    }
}

fn dist(ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let dx = ax - bx;
    let dy = ay - by;
    (dx * dx + dy * dy).sqrt()
}

fn extract_interest(
    stream: &TelemetryStream,
    config: &ZoomConfig,
    uncertainty: &[Interval],
    ignore_v1_clicks: bool,
    diagnostics: &mut Vec<String>,
) -> Vec<InterestPoint> {
    let mut interest = Vec::new();
    let mut held: Option<Vec<u32>> = None;
    let mut dwell: Option<Dwell> = None;

    let flush_dwell = |dwell: &mut Option<Dwell>,
                       end_us: u64,
                       interest: &mut Vec<InterestPoint>| {
        if let Some(active) = dwell.take() {
            if end_us.saturating_sub(active.start_us) >= config.min_dwell_us && active.count > 0 {
                interest.push(InterestPoint {
                    t_us: active.start_us,
                    end_us,
                    seq: active.seqs[0],
                    seqs: active.seqs,
                    x: active.sum_x / active.count as f64,
                    y: active.sum_y / active.count as f64,
                    origin: ZoomOrigin::Dwell,
                });
            }
        }
    };

    for event in &stream.events {
        match &event.kind {
            CanonicalKind::Gap { start_us, .. } => {
                let end = dwell
                    .as_ref()
                    .map(|active| active.last_us.min(*start_us))
                    .unwrap_or(*start_us);
                flush_dwell(&mut dwell, end, &mut interest);
                held = None;
            }
            CanonicalKind::Move {
                norm_x,
                norm_y,
                inside_source,
            }
            | CanonicalKind::ButtonDown {
                norm_x,
                norm_y,
                inside_source,
                ..
            }
            | CanonicalKind::ButtonUp {
                norm_x,
                norm_y,
                inside_source,
                ..
            }
            | CanonicalKind::Click {
                norm_x,
                norm_y,
                inside_source,
            } => {
                let usable = usable_target(
                    stream,
                    event.geometry_id.as_ref(),
                    *norm_x,
                    *norm_y,
                    *inside_source,
                    event.t_us,
                    uncertainty,
                );
                if usable {
                    match &mut dwell {
                        Some(active)
                            if dist(active.anchor_x, active.anchor_y, *norm_x, *norm_y)
                                <= config.dwell_radius_norm =>
                        {
                            active.sum_x += *norm_x;
                            active.sum_y += *norm_y;
                            active.count += 1;
                            active.seqs.push(event.seq);
                            active.last_us = event.t_us;
                        }
                        _ => {
                            let end = dwell.as_ref().map(|d| d.last_us).unwrap_or(event.t_us);
                            flush_dwell(&mut dwell, end, &mut interest);
                            dwell = Some(Dwell {
                                start_us: event.t_us,
                                last_us: event.t_us,
                                anchor_x: *norm_x,
                                anchor_y: *norm_y,
                                sum_x: *norm_x,
                                sum_y: *norm_y,
                                count: 1,
                                seqs: vec![event.seq],
                            });
                        }
                    }
                } else {
                    let end = dwell.as_ref().map(|d| d.last_us).unwrap_or(event.t_us);
                    flush_dwell(&mut dwell, end, &mut interest);
                    if *inside_source == false
                        && diagnostics.len() < 256
                        && !diagnostics.iter().any(|d| d.contains("off-source"))
                    {
                        diagnostics.push(
                            "Off-source coordinates ignored; they are not clamped into edge clicks"
                                .into(),
                        );
                    }
                }

                match &event.kind {
                    CanonicalKind::ButtonDown { button, .. } => {
                        match &mut held {
                            None => held = Some(button.into_iter().copied().collect()),
                            Some(buttons) => {
                                if let Some(button) = button {
                                    if !buttons.contains(button) {
                                        buttons.push(*button);
                                        buttons.sort_unstable();
                                    }
                                }
                            }
                        }
                        if usable && is_primary(*button, config) {
                            interest.push(InterestPoint {
                                t_us: event.t_us,
                                end_us: event.t_us,
                                seq: event.seq,
                                seqs: vec![event.seq],
                                x: *norm_x,
                                y: *norm_y,
                                origin: ZoomOrigin::Click,
                            });
                        }
                    }
                    CanonicalKind::ButtonUp { button, .. } => {
                        if let Some(buttons) = held.as_mut() {
                            if let Some(button) = button {
                                buttons.retain(|b| b != button);
                            } else {
                                buttons.clear();
                            }
                        }
                    }
                    CanonicalKind::Click { .. } => {
                        if usable && !ignore_v1_clicks {
                            interest.push(InterestPoint {
                                t_us: event.t_us,
                                end_us: event.t_us,
                                seq: event.seq,
                                seqs: vec![event.seq],
                                x: *norm_x,
                                y: *norm_y,
                                origin: ZoomOrigin::Click,
                            });
                        }
                    }
                    _ => {}
                }
            }
            CanonicalKind::Scroll { .. } => {}
        }
    }
    let end = dwell.as_ref().map(|d| d.last_us).unwrap_or(0);
    flush_dwell(&mut dwell, end, &mut interest);
    interest.sort_by_key(|p| (p.t_us, p.seq));
    interest
}

struct Dwell {
    start_us: u64,
    last_us: u64,
    anchor_x: f64,
    anchor_y: f64,
    sum_x: f64,
    sum_y: f64,
    count: u32,
    seqs: Vec<u64>,
}

fn cluster_interest(points: &[InterestPoint], config: &ZoomConfig) -> Vec<ZoomSuggestion> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut clusters: Vec<Vec<&InterestPoint>> = Vec::new();
    for point in points {
        let merge_gap = if point.origin == ZoomOrigin::Click {
            config.rapid_click_window_us.max(config.cluster_gap_us)
        } else {
            config.cluster_gap_us
        };
        if let Some(cluster) = clusters.last_mut() {
            let last = *cluster.last().unwrap();
            let last_end = cluster
                .iter()
                .map(|p| p.end_us)
                .max()
                .unwrap_or(last.end_us);
            let cx: f64 = cluster.iter().map(|p| p.x).sum::<f64>() / cluster.len() as f64;
            let cy: f64 = cluster.iter().map(|p| p.y).sum::<f64>() / cluster.len() as f64;
            if point.t_us.saturating_sub(last_end) <= merge_gap
                && dist(cx, cy, point.x, point.y) <= config.cluster_radius_norm
            {
                cluster.push(point);
                continue;
            }
        }
        clusters.push(vec![point]);
    }

    let mut windows: Vec<ZoomSuggestion> = clusters
        .into_iter()
        .filter_map(|cluster| suggestion_from_cluster(&cluster, config))
        .collect();
    windows.sort_by(|a, b| {
        a.source_start_us
            .cmp(&b.source_start_us)
            .then(a.contributing_event_seqs.cmp(&b.contributing_event_seqs))
    });
    merge_overlapping(windows, config)
}

fn suggestion_from_cluster(
    cluster: &[&InterestPoint],
    config: &ZoomConfig,
) -> Option<ZoomSuggestion> {
    if cluster.is_empty() {
        return None;
    }
    let mut seqs: Vec<u64> = cluster
        .iter()
        .flat_map(|p| p.seqs.iter().copied())
        .collect();
    seqs.sort_unstable();
    seqs.dedup();
    let has_click = cluster.iter().any(|p| p.origin == ZoomOrigin::Click);
    let has_dwell = cluster.iter().any(|p| p.origin == ZoomOrigin::Dwell);
    let origin = match (has_click, has_dwell) {
        (true, true) => ZoomOrigin::Cluster,
        (true, false) => ZoomOrigin::Click,
        (false, true) => ZoomOrigin::Dwell,
        (false, false) => ZoomOrigin::Cluster,
    };
    let scale = if has_click {
        config.click_scale.min(config.max_scale)
    } else {
        config.dwell_scale.min(config.max_scale)
    };
    let n = cluster.len() as f64;
    let center_x = cluster.iter().map(|p| p.x).sum::<f64>() / n;
    let center_y = cluster.iter().map(|p| p.y).sum::<f64>() / n;
    let first_t = cluster.iter().map(|p| p.t_us).min().unwrap();
    let last_t = cluster.iter().map(|p| p.end_us).max().unwrap();
    let hold_start = first_t;
    let hold_end = hold_start.saturating_add(config.min_hold_us).max(last_t);
    let source_start = hold_start.saturating_sub(config.transition_us);
    let source_end = hold_end.saturating_add(config.transition_us);
    if source_end <= source_start {
        return None;
    }
    Some(ZoomSuggestion {
        id: format!("z-{}-n{}", seqs[0], seqs.len()),
        source_start_us: source_start,
        source_end_us: source_end,
        center_x,
        center_y,
        scale,
        transition_us: config.transition_us,
        origin,
        contributing_event_seqs: seqs,
        edited_ranges: Vec::new(),
    })
}

fn merge_overlapping(windows: Vec<ZoomSuggestion>, config: &ZoomConfig) -> Vec<ZoomSuggestion> {
    let mut merged: Vec<ZoomSuggestion> = Vec::new();
    for window in windows {
        if let Some(previous) = merged.last_mut() {
            if window.source_start_us < previous.source_end_us
                && dist(
                    previous.center_x,
                    previous.center_y,
                    window.center_x,
                    window.center_y,
                ) <= config.cluster_radius_norm
            {
                let seqs_a = previous.contributing_event_seqs.len() as f64;
                let seqs_b = window.contributing_event_seqs.len() as f64;
                let total = (seqs_a + seqs_b).max(1.0);
                previous.center_x = (previous.center_x * seqs_a + window.center_x * seqs_b) / total;
                previous.center_y = (previous.center_y * seqs_a + window.center_y * seqs_b) / total;
                previous.source_end_us = previous.source_end_us.max(window.source_end_us);
                previous.scale = previous.scale.max(window.scale).min(config.max_scale);
                previous.origin = ZoomOrigin::Cluster;
                previous
                    .contributing_event_seqs
                    .extend(window.contributing_event_seqs);
                previous.contributing_event_seqs.sort_unstable();
                previous.contributing_event_seqs.dedup();
                previous.id = format!(
                    "z-{}-n{}",
                    previous.contributing_event_seqs[0],
                    previous.contributing_event_seqs.len()
                );
                continue;
            }
        }
        merged.push(window);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::reader::{CanonicalEvent, CanonicalGeometry, CanonicalKind};
    use crate::timeline::{SourceInterval, TimelineMapper};
    use std::collections::BTreeMap;

    fn geo(id: &str) -> CanonicalGeometry {
        CanonicalGeometry {
            geometry_id: id.into(),
            t_us: 0,
            version: 2,
            supported: true,
            unsupported_reason: None,
            sampling_interval_us: 100_000,
            source_id: Some("display:1".into()),
        }
    }

    fn stream(events: Vec<CanonicalEvent>) -> TelemetryStream {
        let mut geometries = BTreeMap::new();
        geometries.insert("g1".into(), geo("g1"));
        TelemetryStream {
            events,
            geometries,
            diagnostics: Vec::new(),
        }
    }

    fn move_at(seq: u64, t: u64, x: f64, y: f64) -> CanonicalEvent {
        CanonicalEvent {
            version: 2,
            seq,
            t_us: t,
            geometry_id: Some("g1".into()),
            kind: CanonicalKind::Move {
                norm_x: x,
                norm_y: y,
                inside_source: true,
            },
        }
    }

    fn down(seq: u64, t: u64, button: u32, x: f64, y: f64) -> CanonicalEvent {
        CanonicalEvent {
            version: 2,
            seq,
            t_us: t,
            geometry_id: Some("g1".into()),
            kind: CanonicalKind::ButtonDown {
                button: Some(button),
                norm_x: x,
                norm_y: y,
                inside_source: true,
            },
        }
    }

    fn up(seq: u64, t: u64, button: u32, x: f64, y: f64) -> CanonicalEvent {
        CanonicalEvent {
            version: 2,
            seq,
            t_us: t,
            geometry_id: Some("g1".into()),
            kind: CanonicalKind::ButtonUp {
                button: Some(button),
                norm_x: x,
                norm_y: y,
                inside_source: true,
            },
        }
    }

    fn gap(seq: u64, start: u64, end: u64, reason: &str) -> CanonicalEvent {
        CanonicalEvent {
            version: 2,
            seq,
            t_us: end,
            geometry_id: None,
            kind: CanonicalKind::Gap {
                reason: reason.into(),
                start_us: start,
                end_us: end,
                dropped_events: 0,
            },
        }
    }

    #[test]
    fn identical_input_is_deterministic() {
        let config = ZoomConfig::default();
        let data = stream(vec![
            move_at(0, 1_000_000, 0.4, 0.4),
            down(1, 1_100_000, 0, 0.41, 0.39),
            up(2, 1_150_000, 0, 0.41, 0.39),
        ]);
        let a = generate_zoom_suggestions(&data, &config).unwrap();
        let b = generate_zoom_suggestions(&data, &config).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.suggestions.len(), 1);
        assert_eq!(a.suggestions[0].id, "z-1-n1");
        assert_eq!(a.version, ZOOM_GENERATION_VERSION);
    }

    #[test]
    fn empty_denied_and_gapped_telemetry_do_not_fabricate_zooms() {
        let config = ZoomConfig::default();
        let empty = generate_zoom_suggestions(&stream(vec![]), &config).unwrap();
        assert!(empty.suggestions.is_empty());
        let denied = generate_zoom_suggestions(
            &stream(vec![gap(0, 0, 5_000_000, "input_monitoring_unavailable")]),
            &config,
        )
        .unwrap();
        assert!(denied.suggestions.is_empty());
        let revoked = generate_zoom_suggestions(
            &stream(vec![gap(0, 0, 5_000_000, "input_monitoring_revoked")]),
            &config,
        )
        .unwrap();
        assert!(revoked.suggestions.is_empty());
        let gapped = generate_zoom_suggestions(
            &stream(vec![
                down(0, 1_000_000, 0, 0.2, 0.2),
                gap(1, 1_050_000, 4_000_000, "queue_overflow"),
                up(2, 4_100_000, 0, 0.8, 0.8),
            ]),
            &config,
        )
        .unwrap();
        assert_eq!(gapped.suggestions.len(), 1);
        assert!(gapped.suggestions[0].contributing_event_seqs.contains(&0));
        assert!(!gapped.suggestions[0].contributing_event_seqs.contains(&2));
    }

    #[test]
    fn corrupt_interior_gap_does_not_join_dwells() {
        let config = ZoomConfig::default();
        let data = stream(vec![
            move_at(0, 200_000, 0.5, 0.5),
            gap(1, 200_000, 1_200_000, "corrupt_record"),
            move_at(2, 1_200_000, 0.5, 0.5),
        ]);
        let generated = generate_zoom_suggestions(&data, &config).unwrap();
        assert!(
            generated.suggestions.iter().all(|suggestion| {
                suggestion.source_end_us.saturating_sub(suggestion.source_start_us) < 1_000_000
            }),
            "dwell must not span a corrupt telemetry gap: {:?}",
            generated.suggestions
        );
    }

    #[test]
    fn geometry_uncertainty_interval_skips_clicks_inside_poll_window() {
        let config = ZoomConfig::default();
        assert_eq!(config.geometry_uncertainty_us, 100_000);
        let data = stream(vec![
            gap(0, 1_000_000, 1_100_000, "geometry_changed"),
            down(1, 1_050_000, 0, 0.4, 0.4),
            up(2, 1_080_000, 0, 0.4, 0.4),
            down(3, 1_500_000, 0, 0.5, 0.5),
            up(4, 1_540_000, 0, 0.5, 0.5),
        ]);
        let generated = generate_zoom_suggestions(&data, &config).unwrap();
        assert_eq!(generated.suggestions.len(), 1);
        assert_eq!(generated.suggestions[0].contributing_event_seqs, vec![3]);
    }

    #[test]
    fn auxiliary_buttons_are_ignored_and_rapid_clicks_merge() {
        let config = ZoomConfig::default();
        let data = stream(vec![
            down(0, 1_000_000, 4, 0.5, 0.5),
            up(1, 1_050_000, 4, 0.5, 0.5),
            down(2, 2_000_000, 0, 0.30, 0.30),
            up(3, 2_040_000, 0, 0.30, 0.30),
            down(4, 2_200_000, 0, 0.31, 0.29),
            up(5, 2_240_000, 0, 0.31, 0.29),
        ]);
        let generated = generate_zoom_suggestions(&data, &config).unwrap();
        assert_eq!(generated.suggestions.len(), 1);
        assert_eq!(generated.suggestions[0].origin, ZoomOrigin::Click);
        assert_eq!(generated.suggestions[0].contributing_event_seqs, vec![2, 4]);
        assert_eq!(generated.suggestions[0].scale, config.click_scale);
    }

    #[test]
    fn v1_click_is_not_double_counted_with_downs() {
        let config = ZoomConfig::default();
        let mixed = stream(vec![
            CanonicalEvent {
                version: 1,
                seq: 0,
                t_us: 1_000_000,
                geometry_id: Some("g1".into()),
                kind: CanonicalKind::ButtonDown {
                    button: None,
                    norm_x: 0.4,
                    norm_y: 0.4,
                    inside_source: true,
                },
            },
            CanonicalEvent {
                version: 1,
                seq: 1,
                t_us: 1_010_000,
                geometry_id: Some("g1".into()),
                kind: CanonicalKind::Click {
                    norm_x: 0.4,
                    norm_y: 0.4,
                    inside_source: true,
                },
            },
        ]);
        let generated = generate_zoom_suggestions(&mixed, &config).unwrap();
        assert_eq!(generated.suggestions.len(), 1);
        assert_eq!(generated.suggestions[0].contributing_event_seqs, vec![0]);

        let click_only = stream(vec![CanonicalEvent {
            version: 1,
            seq: 7,
            t_us: 2_000_000,
            geometry_id: Some("g1".into()),
            kind: CanonicalKind::Click {
                norm_x: 0.6,
                norm_y: 0.6,
                inside_source: true,
            },
        }]);
        let from_click = generate_zoom_suggestions(&click_only, &config).unwrap();
        assert_eq!(from_click.suggestions.len(), 1);
        assert_eq!(from_click.suggestions[0].contributing_event_seqs, vec![7]);
    }

    #[test]
    fn off_source_and_unsupported_geometry_are_not_clamped_into_targets() {
        let config = ZoomConfig::default();
        let mut data = stream(vec![CanonicalEvent {
            version: 2,
            seq: 0,
            t_us: 1_000_000,
            geometry_id: Some("g1".into()),
            kind: CanonicalKind::ButtonDown {
                button: Some(0),
                norm_x: -0.2,
                norm_y: 1.4,
                inside_source: false,
            },
        }]);
        data.geometries.insert(
            "win".into(),
            CanonicalGeometry {
                geometry_id: "win".into(),
                t_us: 0,
                version: 2,
                supported: false,
                unsupported_reason: Some("window geometry is missing physical transforms".into()),
                sampling_interval_us: 100_000,
                source_id: Some("window:3".into()),
            },
        );
        data.events.push(CanonicalEvent {
            version: 2,
            seq: 1,
            t_us: 2_000_000,
            geometry_id: Some("win".into()),
            kind: CanonicalKind::ButtonDown {
                button: Some(0),
                norm_x: 0.5,
                norm_y: 0.5,
                inside_source: true,
            },
        });
        let generated = generate_zoom_suggestions(&data, &config).unwrap();
        assert!(generated.suggestions.is_empty());
        assert!(generated
            .diagnostics
            .iter()
            .any(|d| d.contains("Off-source")));
    }

    #[test]
    fn dwell_emits_lower_scale_than_clicks() {
        let config = ZoomConfig::default();
        let data = stream(vec![
            move_at(0, 1_000_000, 0.5, 0.5),
            move_at(1, 1_400_000, 0.51, 0.49),
            move_at(2, 1_800_000, 0.50, 0.50),
        ]);
        let generated = generate_zoom_suggestions(&data, &config).unwrap();
        assert_eq!(generated.suggestions.len(), 1);
        assert_eq!(generated.suggestions[0].origin, ZoomOrigin::Dwell);
        assert_eq!(generated.suggestions[0].scale, config.dwell_scale);
    }

    #[test]
    fn bezier_evaluator_respects_cuts_and_clamps_viewport_not_source() {
        let config = ZoomConfig::default();
        let suggestion = ZoomSuggestion {
            id: "z-1-n1".into(),
            source_start_us: 1_000_000,
            source_end_us: 3_000_000,
            center_x: 0.05,
            center_y: 0.05,
            scale: 2.0,
            transition_us: 400_000,
            origin: ZoomOrigin::Click,
            contributing_event_seqs: vec![1],
            edited_ranges: Vec::new(),
        };
        let identity = evaluate_at_source(&[suggestion.clone()], 0, &config);
        assert_eq!(identity.scale, 1.0);
        assert_eq!(identity.center_x, 0.5);

        let hold = evaluate_at_source(&[suggestion.clone()], 1_600_000, &config);
        assert!((hold.scale - 2.0).abs() < 1e-9);
        assert!(hold.center_x > 0.05);
        assert!(hold.center_x >= 0.25 - 1e-9);
        assert!(hold.center_x <= 0.75 + 1e-9);

        let mapper = TimelineMapper::try_new(vec![
            SourceInterval::new("a".into(), 0, 1_200_000),
            SourceInterval::new("b".into(), 2_000_000, 4_000_000),
        ])
        .unwrap();
        let before_cut =
            evaluate_at_edited(&[suggestion.clone()], &mapper, 1_199_999, &config).unwrap();
        let after_cut =
            evaluate_at_edited(&[suggestion.clone()], &mapper, 1_200_000, &config).unwrap();
        let source_before = evaluate_at_source(&[suggestion.clone()], 1_199_999, &config);
        let source_after = evaluate_at_source(&[suggestion], 2_000_000, &config);
        assert_eq!(before_cut, source_before);
        assert_eq!(after_cut, source_after);
        assert_ne!(before_cut.scale, after_cut.scale);
        assert!((cubic_bezier_unit(0.0) - 0.0).abs() < 1e-12);
        assert!((cubic_bezier_unit(1.0) - 1.0).abs() < 1e-12);
        assert!((cubic_bezier_unit(0.5) - 0.5).abs() < 1e-12);

        let identity_uv = CameraTransform::identity().uv_rect();
        assert_eq!(identity_uv, (0.0, 0.0, 1.0, 1.0));
        let hold_uv = hold.uv_rect();
        assert!((hold_uv.0 - (hold.center_x as f32 - hold_uv.2 * 0.5)).abs() < 1e-5);
        assert!((hold_uv.2 - 0.5).abs() < 1e-5);
        assert!((hold_uv.3 - 0.5).abs() < 1e-5);
        let corner_uv = CameraTransform {
            center_x: 0.05,
            center_y: 0.05,
            scale: 2.0,
        }
        .uv_rect();
        assert_eq!(corner_uv.0, 0.0);
        assert_eq!(corner_uv.1, 0.0);
        assert!((corner_uv.2 - 0.5).abs() < 1e-5);
    }

    #[test]
    fn attach_edited_ranges_omits_removed_source_time() {
        let mut generation = ZoomGeneration {
            version: 1,
            config: ZoomConfig::default(),
            suggestions: vec![ZoomSuggestion {
                id: "z-1-n1".into(),
                source_start_us: 1_000_000,
                source_end_us: 6_000_000,
                center_x: 0.4,
                center_y: 0.4,
                scale: 2.0,
                transition_us: 400_000,
                origin: ZoomOrigin::Click,
                contributing_event_seqs: vec![1],
                edited_ranges: Vec::new(),
            }],
            diagnostics: Vec::new(),
        };
        let mapper = TimelineMapper::try_new(vec![
            SourceInterval::new("a".into(), 0, 2_000_000),
            SourceInterval::new("b".into(), 5_000_000, 10_000_000),
        ])
        .unwrap();
        attach_edited_ranges(&mut generation, &mapper);
        assert_eq!(
            generation.suggestions[0].edited_ranges,
            vec![
                EditedRange {
                    start_us: 1_000_000,
                    end_us: 2_000_000
                },
                EditedRange {
                    start_us: 2_000_000,
                    end_us: 3_000_000
                }
            ]
        );
    }
}
