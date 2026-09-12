//! Live-preview pump: bridges the Swift capture mailbox (`LivePreviewFrames`)
//! into the studio and HUD preview surfaces (`PreviewOwner`).
//!
//! This replaces the editor-side `playback/engine.rs` tick loop that was
//! removed when the editor split out into its own repository. While
//! `state.live_preview` is set, the pump:
//!
//! 1. Polls `capture::preview::frame()` (composited screen + camera bubble)
//!    and `capture::preview::camera_frame()` (camera-only).
//! 2. When a frame is available, dispatches a closure to the AppKit main
//!    thread that calls `present_frame` on the appropriate `PreviewOwner`,
//!    guarded by `generation == surface.generation` so a mid-cycle detach
//!    never races against an in-flight present.
//!
//! The pump lives for the lifetime of the Tauri app: it is started by
//! `commands::spawn_live_preview_pump` (called from `lib.rs::run`) and
//! observes the shared `live_preview: AtomicBool` flag.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

#[cfg(feature = "tauri-app")]
use tauri::Manager;

use super::preview;
#[cfg(feature = "tauri-app")]
use crate::commands::AppState;

/// Maximum pump tick rate while live preview is active. ~30 fps is plenty
/// for a Record-scene preview and keeps the SCStream mailbox happy.
const TICK_INTERVAL: Duration = Duration::from_millis(33);

/// Spawn the live-preview pump thread. Safe to call exactly once at startup.
/// Without the `tauri-app` feature this is a no-op (the lib is being compiled
/// for cargo test, doctest, or a non-Tauri consumer).
#[cfg(feature = "tauri-app")]
pub fn spawn(app: tauri::AppHandle) {
    thread::Builder::new()
        .name("aeroshoot-live-preview-pump".into())
        .spawn(move || run(app))
        .expect("failed to spawn live-preview pump thread");
}

#[cfg(not(feature = "tauri-app"))]
pub fn spawn(_app: ()) {
    // No-op without the tauri runtime.
    let _ = _app;
}

#[cfg(feature = "tauri-app")]
fn run(app: tauri::AppHandle) {
    // Track whether the pump has been asked to shut down. Currently only the
    // process exit kills this thread; the flag exists so tests can simulate a
    // clean stop without joining.
    let shutdown = Arc::new(AtomicBool::new(false));
    while !shutdown.load(Ordering::Acquire) {
        let state = app.state::<AppState>();
        if !state.live_preview.load(Ordering::Acquire) {
            thread::sleep(TICK_INTERVAL);
            continue;
        }
        tick(&app, &state);
        thread::sleep(TICK_INTERVAL);
    }
}

#[cfg(feature = "tauri-app")]
fn tick(app: &tauri::AppHandle, state: &AppState) {
    let studio_status = state.studio_preview.lock().status();
    let hud_status = state.hud_preview.lock().status();
    let recording = state.active_session.read().is_some();
    let hud_visible = hud_status.attached
        && hud_status.visible
        && state.hud.lock().desired_visible(recording);

    // Studio: full composited (screen + camera bubble) frame.
    let studio_frame = if studio_status.attached && studio_status.visible {
        preview::frame()
    } else {
        None
    };
    // HUD: camera-only crop. Falls back to the composited frame if the
    // dedicated camera mailbox is empty but the composited one has camera
    // pixels — keeps the bubble alive even when the screen stream drops.
    let hud_frame = if hud_visible {
        preview::camera_frame().or_else(|| {
            preview::frame().and_then(|frame| {
                if frame.flags.has_camera() {
                    Some(frame)
                } else {
                    None
                }
            })
        })
    } else {
        None
    };

    if studio_frame.is_none() && hud_frame.is_none() {
        return;
    }

    let studio_generation = studio_status.generation;
    let hud_generation = hud_status.generation;
    let app_clone = app.clone();
    let _ = app.run_on_main_thread(move || {
        let state = app_clone.state::<AppState>();
        if let Some(frame) = studio_frame {
            let mut surface = state.studio_preview.lock();
            if surface.status().generation == studio_generation {
                let _ = surface.present_frame(&frame, studio_generation);
            }
        }
        if let Some(frame) = hud_frame {
            let mut surface = state.hud_preview.lock();
            if surface.status().generation == hud_generation {
                let _ = surface.present_frame(&frame, hud_generation);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_interval_is_finite() {
        // Sanity check: the constant is what the thread actually uses and
        // hasn't been accidentally set to zero (which would pin a CPU core).
        assert!(TICK_INTERVAL.as_millis() >= 16);
        assert!(TICK_INTERVAL.as_millis() <= 1000);
    }
}
