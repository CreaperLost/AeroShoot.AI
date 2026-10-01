use aeroshoot_lib::commands::*;
use aeroshoot_lib::hud::{PreviewHitMode, PreviewViewport, STUDIO_WINDOW_LABEL};
use tempfile::tempdir;

fn studio_viewport(revision: u64) -> PreviewViewport {
    PreviewViewport {
        window_label: STUDIO_WINDOW_LABEL.into(),
        x: 0.0,
        y: 0.0,
        width: 320.0,
        height: 180.0,
        backing_scale: 1.0,
        visible: true,
        occluded: false,
        revision,
        generation: 0,
        clip: None,
    }
}

#[test]
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    ignore = "native preview is not implemented on this platform yet"
)]
fn studio_preview_attaches_lays_out_and_detaches() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    let status = studio_preview_attach_impl(
        &state,
        STUDIO_WINDOW_LABEL.into(),
        PreviewHitMode::Consume,
        None,
    )
    .unwrap();
    assert!(status.attached);
    let generation = status.generation;
    let layout = studio_preview_layout_impl(
        &state,
        PreviewViewport {
            generation,
            ..studio_viewport(1)
        },
    )
    .unwrap();
    assert_eq!(layout.layout_revision, 1);
    assert!(studio_preview_status_impl(&state).attached);
    let detached = studio_preview_detach_impl(&state).unwrap();
    assert!(!detached.attached);
    assert_ne!(detached.generation, generation);
}
