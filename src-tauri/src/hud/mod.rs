//! Record-scene preview surface: geometry and lifetime ownership (`preview`)
//! and the macOS AppKit child-view adapter (`native`).
pub mod native;
pub mod preview;
pub use preview::{PreviewHitMode, PreviewOwner, PreviewStatus, PreviewViewport};

/// The application's only window; the record scene preview attaches to it.
pub const STUDIO_WINDOW_LABEL: &str = "main";
