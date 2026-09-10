//! Source-anchored cubic Bézier camera evaluator.
//!
//! Cuts are visual discontinuities: map edited time to source time first, then
//! evaluate the original curve at that source instant. Do not reparameterize
//! across removed time. Clamp the camera viewport, never the recorded point.
use super::{ZoomConfig, ZoomSuggestion};
use crate::timeline::TimelineMapper;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraTransform {
    pub center_x: f64,
    pub center_y: f64,
    pub scale: f64,
}

impl CameraTransform {
    pub fn identity() -> Self {
        Self {
            center_x: 0.5,
            center_y: 0.5,
            scale: 1.0,
        }
    }

    /// Source-normalized UV rectangle to crop the screen layer. Identity is the
    /// full frame. The viewport is clamped; recorded click coordinates are not.
    pub fn uv_rect(&self) -> (f32, f32, f32, f32) {
        if !self.scale.is_finite() || self.scale <= 1.0 {
            return (0.0, 0.0, 1.0, 1.0);
        }
        let width = (1.0 / self.scale).clamp(0.0001, 1.0);
        let height = width;
        let x = (self.center_x - width * 0.5).clamp(0.0, 1.0 - width);
        let y = (self.center_y - height * 0.5).clamp(0.0, 1.0 - height);
        (x as f32, y as f32, width as f32, height as f32)
    }
}

/// Smoothstep cubic equivalent to Bézier P0=P1=0, P2=P3=1.
/// Value and first derivative are 0 at t=0 and match the end at t=1.
pub fn cubic_bezier_unit(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn clamp_camera(
    center_x: f64,
    center_y: f64,
    scale: f64,
    config: &ZoomConfig,
) -> CameraTransform {
    let scale = if !scale.is_finite() {
        1.0
    } else {
        scale.clamp(1.0, config.max_scale)
    };
    if scale <= 1.0 {
        return CameraTransform::identity();
    }
    let margin = config.viewport_margin.clamp(0.0, 0.49);
    let half = 0.5 / scale;
    let min_c = half + margin;
    let max_c = 1.0 - half - margin;
    let clamp_axis = |value: f64| {
        if min_c > max_c {
            0.5
        } else {
            value.clamp(min_c, max_c)
        }
    };
    CameraTransform {
        center_x: clamp_axis(center_x),
        center_y: clamp_axis(center_y),
        scale,
    }
}

pub fn evaluate_at_source(
    suggestions: &[ZoomSuggestion],
    source_us: u64,
    config: &ZoomConfig,
) -> CameraTransform {
    let Some(active) = select_suggestion(suggestions, source_us) else {
        return CameraTransform::identity();
    };
    let (center_x, center_y, scale) = sample_suggestion(active, source_us);
    clamp_camera(center_x, center_y, scale, config)
}

pub fn evaluate_at_edited(
    suggestions: &[ZoomSuggestion],
    mapper: &TimelineMapper,
    edited_us: u64,
    config: &ZoomConfig,
) -> Option<CameraTransform> {
    let source_us = mapper.edited_to_source_us(edited_us)?;
    Some(evaluate_at_source(suggestions, source_us, config))
}

fn select_suggestion(suggestions: &[ZoomSuggestion], source_us: u64) -> Option<&ZoomSuggestion> {
    suggestions
        .iter()
        .filter(|item| source_us >= item.source_start_us && source_us < item.source_end_us)
        .max_by(|a, b| {
            a.source_start_us
                .cmp(&b.source_start_us)
                .then(
                    a.scale
                        .partial_cmp(&b.scale)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.id.cmp(&b.id))
        })
}

fn sample_suggestion(suggestion: &ZoomSuggestion, source_us: u64) -> (f64, f64, f64) {
    let transition = suggestion.transition_us;
    let hold_start = suggestion.source_start_us.saturating_add(transition);
    let hold_end = suggestion
        .source_end_us
        .saturating_sub(transition)
        .max(hold_start);
    let scale = if source_us < hold_start {
        let span = hold_start.saturating_sub(suggestion.source_start_us).max(1);
        let t = (source_us.saturating_sub(suggestion.source_start_us) as f64) / span as f64;
        lerp(1.0, suggestion.scale, cubic_bezier_unit(t))
    } else if source_us >= hold_end {
        let span = suggestion.source_end_us.saturating_sub(hold_end).max(1);
        let t = (source_us.saturating_sub(hold_end) as f64) / span as f64;
        lerp(suggestion.scale, 1.0, cubic_bezier_unit(t))
    } else {
        suggestion.scale
    };
    (suggestion.center_x, suggestion.center_y, scale)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}
