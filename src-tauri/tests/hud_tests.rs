use aeroshoot_lib::capture::preview::{LiveFrame, LivePreviewFlags, PREVIEW_BYTES};
use aeroshoot_lib::commands::*;
use aeroshoot_lib::hud::{
    resolve_ui_root, window_identity_from_label, HudCameraInfo, HudSettingsPatch, HudShape,
    HudSize, HudSubscriber, PreviewHitMode, PreviewViewport, UiRootKind, HUD_WINDOW_LABEL,
    STUDIO_WINDOW_LABEL,
};
use aeroshoot_lib::session::SessionState;
use tempfile::tempdir;

fn opts() -> StartRecordingOptions {
    StartRecordingOptions {
        source_id: "screen-main".into(),
        capture_screen: true,
        camera_id: Some("cam-1".into()),
        mic_id: None,
        capture_system_audio: false,
        fps: 30,
        resolution: "1080p".into(),
        layout: None,
        project_name: None,
        project_dir: None,
        mic_gain_db: None,
        video_bitrate_bps: None,
        capture_mouse: true,
    }
}

#[test]
fn window_label_routing_rejects_unknown_identities() {
    assert_eq!(
        resolve_ui_root(STUDIO_WINDOW_LABEL).unwrap(),
        UiRootKind::Studio
    );
    assert_eq!(resolve_ui_root(HUD_WINDOW_LABEL).unwrap(), UiRootKind::Hud);
    assert!(window_identity_from_label("main").ui_root == Some(UiRootKind::Studio));
    assert!(!window_identity_from_label(HUD_WINDOW_LABEL).rejected);
    for label in ["", "overlay", "webview", "MAIN", "camera-overlay"] {
        let identity = window_identity_from_label(label);
        assert!(identity.rejected, "{label} must not receive the studio");
        assert!(identity.ui_root.is_none());
        assert!(resolve_ui_root(label).is_err());
    }
}

#[test]
fn settings_converge_on_one_revision_and_reject_stale() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    let studio = hud_update_impl(
        &state,
        0,
        HudSettingsPatch {
            shape: Some(HudShape::Circle),
            size: Some(HudSize::Lg),
            ..HudSettingsPatch::default()
        },
    )
    .unwrap();
    assert_eq!(studio.revision, 1);
    let overlay = hud_update_impl(
        &state,
        1,
        HudSettingsPatch {
            mirror: Some(false),
            ..HudSettingsPatch::default()
        },
    )
    .unwrap();
    assert_eq!(overlay.revision, 2);
    assert!(!overlay.settings.mirror);
    assert_eq!(overlay.settings.size, HudSize::Lg);
    let stale = hud_update_impl(
        &state,
        1,
        HudSettingsPatch {
            size: Some(HudSize::Sm),
            ..HudSettingsPatch::default()
        },
    )
    .unwrap_err();
    assert!(stale.contains("Stale"));
    let snap = hud_snapshot_impl(&state);
    assert_eq!(snap.revision, 2);
    assert_eq!(snap.settings.size, HudSize::Lg);
    assert!(!snap.settings.mirror);
}

#[test]
fn dropped_events_and_reconnect_use_snapshot() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    hud_update_impl(
        &state,
        0,
        HudSettingsPatch {
            shape: Some(HudShape::Squircle),
            ..HudSettingsPatch::default()
        },
    )
    .unwrap();
    hud_update_impl(
        &state,
        1,
        HudSettingsPatch {
            size: Some(HudSize::Xl),
            ..HudSettingsPatch::default()
        },
    )
    .unwrap();
    let mut subscriber = HudSubscriber::new();
    let first = state.hud.lock().events()[0].clone();
    subscriber.on_event(&first, &hud_snapshot_impl(&state));
    assert_eq!(subscriber.last_seen, 1);
    // Miss revision 2, then reconnect.
    subscriber.reconnect(&hud_snapshot_impl(&state));
    assert!(subscriber.recovered_from_snapshot);
    assert_eq!(subscriber.last_seen, 2);
    assert_eq!(subscriber.view.size, HudSize::Xl);
    assert_eq!(subscriber.view.shape, HudShape::Squircle);
}

#[test]
fn camera_removal_is_visible_in_hud_snapshot() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    hud_reconcile_cameras_impl(
        &state,
        vec![HudCameraInfo {
            id: "cam-1".into(),
            name: "Studio".into(),
        }],
        Some("cam-1".into()),
    )
    .unwrap();
    assert!(hud_snapshot_impl(&state).camera_available);
    let removed = hud_reconcile_cameras_impl(&state, Vec::new(), None).unwrap();
    assert!(!removed.camera_available);
    assert_eq!(removed.camera_id.as_deref(), Some("cam-1"));
    assert!(removed
        .diagnostics
        .iter()
        .any(|d| d.contains("unavailable")));
}

#[test]
fn closing_hud_does_not_stop_recording_or_start_a_second_session() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    let started = start_recording_impl(&state, opts()).unwrap();
    assert_eq!(started.state, SessionState::Recording);
    hud_preview_attach_impl(
        &state,
        HUD_WINDOW_LABEL.into(),
        PreviewHitMode::Circle,
        None,
    )
    .unwrap();
    hud_set_visible_impl(&state, true).unwrap();
    let before = get_session_status_impl(&state);
    assert_eq!(before.state, SessionState::Recording);
    let attached = hud_snapshot_impl(&state);
    assert!(attached.hud_attached);
    assert!(attached.capture_session_alive);
    assert!(!attached.started_independent_capture);
    assert!(attached.hud_visible);
    assert!(attached.exclusion_established);
    let hidden = hud_set_visible_impl(&state, false).unwrap();
    assert!(!hidden.hud_visible);
    assert!(hidden.hud_attached);
    assert_eq!(
        get_session_status_impl(&state).state,
        SessionState::Recording
    );
    let reshown = hud_set_visible_impl(&state, true).unwrap();
    assert!(reshown.hud_visible);
    assert!(reshown.hud_attached);
    let closed = hud_close_impl(&state).unwrap();
    assert!(!closed.hud_attached);
    assert!(!closed.started_independent_capture);
    assert!(closed.capture_session_alive);
    assert!(closed.session_recording);
    let after = get_session_status_impl(&state);
    assert_eq!(after.state, SessionState::Recording);
    assert!(!hud_preview_status_impl(&state).attached);
    assert!(state.active_session.read().is_some());
    let stopped = stop_recording_impl(&state).unwrap();
    assert_eq!(stopped.session_id, started.session_id);
}

#[test]
fn hud_preview_rejects_studio_label_and_does_not_use_studio_surface() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    assert!(hud_preview_attach_impl(
        &state,
        STUDIO_WINDOW_LABEL.into(),
        PreviewHitMode::Consume,
        None,
    )
    .unwrap_err()
    .contains("camera_overlay"));
    assert!(!hud_snapshot_impl(&state).hud_attached);
    assert!(!hud_preview_status_impl(&state).attached);
}

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
fn studio_preview_accepts_main_and_rejects_hud_label() {
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
    let studio_generation = status.generation;
    let layout = studio_preview_layout_impl(
        &state,
        PreviewViewport {
            generation: studio_generation,
            ..studio_viewport(1)
        },
    )
    .unwrap();
    assert_eq!(layout.layout_revision, 1);
    assert!(studio_preview_status_impl(&state).attached);
    assert!(studio_preview_attach_impl(
        &state,
        HUD_WINDOW_LABEL.into(),
        PreviewHitMode::PassThrough,
        None,
    )
    .unwrap_err()
    .contains("camera_overlay"));
    assert!(!hud_preview_status_impl(&state).attached);
    let detached = studio_preview_detach_impl(&state).unwrap();
    assert!(!detached.attached);
    assert_ne!(detached.generation, studio_generation);
}

#[test]
fn studio_preview_present_frame_updates_kind_and_bytes() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    let status = studio_preview_attach_impl(
        &state,
        STUDIO_WINDOW_LABEL.into(),
        PreviewHitMode::Consume,
        None,
    )
    .unwrap();
    let generation = status.generation;
    let frame = LiveFrame::new(LivePreviewFlags::BOTH, vec![0u8; PREVIEW_BYTES]);
    state
        .studio_preview
        .lock()
        .present_frame(&frame, generation)
        .unwrap();
    let after = studio_preview_status_impl(&state);
    assert_eq!(after.presented_kind, "live");
    assert_eq!(after.presented_bytes as usize, PREVIEW_BYTES);
    assert_eq!(after.copies, 1);
    assert!(state
        .studio_preview
        .lock()
        .present_frame(&frame, generation + 9)
        .unwrap_err()
        .contains("Stale"));
    studio_preview_detach_impl(&state).unwrap();
    assert_eq!(studio_preview_status_impl(&state).presented_kind, "none");
}

#[test]
fn studio_and_hud_surfaces_have_independent_generations() {
    let dir = tempdir().unwrap();
    let state = AppState::new_test(dir.path().to_path_buf());
    let studio = studio_preview_attach_impl(
        &state,
        STUDIO_WINDOW_LABEL.into(),
        PreviewHitMode::Consume,
        None,
    )
    .unwrap();
    let _hud = hud_preview_attach_impl(
        &state,
        HUD_WINDOW_LABEL.into(),
        PreviewHitMode::CirclePassThrough,
        None,
    )
    .unwrap();
    // The two `PreviewOwner` instances are independent. Each `attach` first
    // calls `detach` (which bumps generation), then bumps again — so a fresh
    // owner's first attach lands at generation 2. What we actually want to
    // verify is that detaching one surface does not move the other.
    let hud_status_before = hud_preview_status_impl(&state);
    let studio_status_before = studio_preview_status_impl(&state);
    assert_eq!(studio_status_before.generation, studio.generation);
    assert_eq!(hud_status_before.generation, 2);
    hud_close_impl(&state).unwrap();
    let studio_after = studio_preview_status_impl(&state);
    assert!(studio_after.attached);
    assert_eq!(studio_after.generation, studio.generation);
    // Studio detach bumps its own generation; HUD was already detached.
    let detached = studio_preview_detach_impl(&state).unwrap();
    assert!(!detached.attached);
    assert_ne!(detached.generation, studio.generation);
}
