pub mod capture;
pub mod commands;
pub mod dsp;
pub mod export;
pub mod fixtures;
pub mod hud;
pub mod media;
pub mod playback;
pub mod project;
pub mod render;
pub mod session;
pub mod telemetry;
pub mod timeline;
pub mod zoom;

#[cfg(feature = "tauri-app")]
use commands::*;
#[cfg(feature = "tauri-app")]
use tauri::{Emitter, Manager, State};

#[cfg(feature = "tauri-app")]
fn emit_hud(app: &tauri::AppHandle, snapshot: &hud::HudSnapshot) {
    let _ = app.emit(hud::HUD_SETTINGS_EVENT, snapshot);
}

#[cfg(feature = "tauri-app")]
fn sync_hud_window(app: &tauri::AppHandle, snapshot: &hud::HudSnapshot) {
    if let Some(window) = app.get_webview_window(hud::HUD_WINDOW_LABEL) {
        if snapshot.hud_visible {
            let (width, height) = snapshot.settings.window_size_css_px();
            let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize::new(
                f64::from(width),
                f64::from(height),
            )));
            let _ = window.show();
        } else {
            let _ = window.hide();
        }
    }
    emit_hud(app, snapshot);
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn capture_preview_configure(
    app: tauri::AppHandle,
    enabled: bool,
    source_id: Option<String>,
    camera_id: Option<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _guard = state.command_lock.lock();
        if !enabled {
            // Recording owns the mailbox; React cleanup must not hide those frames
            // or starve editor playback setup after Stop.
            if state.active_session.read().is_some() {
                return Ok(());
            }
            state
                .live_preview
                .store(false, std::sync::atomic::Ordering::Release);
            capture::preview::stop();
            return Ok(());
        }
        state
            .live_preview
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = state.playback.lock().pause();
        if state.active_session.read().is_some() {
            return Ok(());
        }
        let started = capture::preview::start(
            source_id.as_deref().ok_or("Select a screen source")?,
            camera_id.as_deref(),
        );
        if started.is_err() {
            state
                .live_preview
                .store(false, std::sync::atomic::Ordering::Release);
        }
        started
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn list_capture_sources() -> Vec<capture::CaptureSource> {
    commands::list_capture_sources_impl()
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn list_devices() -> commands::DevicesResult {
    commands::list_devices_impl()
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn start_recording(
    app: tauri::AppHandle,
    options: commands::StartRecordingOptions,
) -> Result<commands::StartRecordingResult, String> {
    // Hide the HUD before ScreenCaptureKit admits frames. Post-start sync is
    // too late for the first captured interval.
    if let Some(window) = app.get_webview_window(hud::HUD_WINDOW_LABEL) {
        let _ = window.hide();
    }
    let app_handle = app.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        commands::start_recording_impl(&app_handle.state::<AppState>(), options)
    })
    .await
    .map_err(|e| e.to_string());
    let res = match res {
        Ok(inner) => inner,
        Err(error) => {
            sync_hud_after_session(&app);
            return Err(error);
        }
    };
    if let Some(session) = app.state::<AppState>().active_session.read().as_ref() {
        if let Some(window) = app.get_webview_window("main") {
            let title = commands::window_title_for_recording(Some(&session.project_name));
            let _ = window.set_title(&title);
        }
    }
    let has_webcam = app
        .state::<AppState>()
        .active_session
        .read()
        .as_ref()
        .is_some_and(|session| {
            session
                .project_bundle
                .manifest()
                .tracks
                .iter()
                .any(|track| track.track_type == project::TrackType::Webcam)
        });
    if res.is_ok() {
        let _ = commands::hud_set_visible_impl(&app.state::<AppState>(), has_webcam);
    }
    sync_hud_after_session(&app);
    res
}

#[cfg(feature = "tauri-app")]
fn sync_hud_after_session(app: &tauri::AppHandle) {
    let snapshot = commands::hud_snapshot_impl(&app.state::<AppState>());
    sync_hud_window(app, &snapshot);
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn pause_recording(state: State<'_, AppState>) -> Result<commands::SessionStateResult, String> {
    commands::pause_recording_impl(&state)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn resume_recording(state: State<'_, AppState>) -> Result<commands::SessionStateResult, String> {
    commands::resume_recording_impl(&state)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn stop_recording(app: tauri::AppHandle) -> Result<commands::StopRecordingResult, String> {
    let app_handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        commands::stop_recording_impl(&app_handle.state::<AppState>())
    })
    .await
    .map_err(|e| e.to_string())?;
    sync_hud_after_session(&app);
    result
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_session_status(state: State<'_, AppState>) -> commands::SessionStatusResult {
    commands::get_session_status_impl(&state)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn get_permission_status(
    state: State<'_, AppState>,
) -> Result<capture::PermissionStatus, String> {
    // ScreenCaptureKit probes must not run on the UI thread: waiting there
    // deadlocks the permission callback. Tauri also requires async commands
    // that take `State` to return a Result so the borrowed message is not
    // held across the 'static future.
    let override_status = state.permission_override.read().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(status) = override_status {
            status
        } else {
            crate::capture::check_system_permissions()
        }
    })
    .await
    .map_err(|error| format!("permission check failed: {error}"))
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn request_capture_permissions(
    screen: bool,
    camera: bool,
    microphone: bool,
) -> capture::PermissionStatus {
    tauri::async_runtime::spawn_blocking(move || {
        commands::request_permissions_impl(screen, camera, microphone)
    })
    .await
    .unwrap_or(capture::PermissionStatus {
        screen_recording: capture::PermissionState::Unknown,
        camera: capture::PermissionState::Unknown,
        microphone: capture::PermissionState::Unknown,
    })
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn mouse_telemetry_permission(request: bool) -> crate::telemetry::native::MouseTelemetryPermission {
    #[cfg(target_os = "macos")]
    {
        crate::telemetry::native::MouseTelemetryPermission::macos(
            capture::macos::mouse_permission(request),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = request;
        crate::telemetry::native::MouseTelemetryPermission::unsupported()
    }
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn open_system_privacy_settings(pane: Option<String>) -> commands::OpenSettingsResult {
    commands::open_system_privacy_settings_impl(pane)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn restart_app(app: tauri::AppHandle) {
    app.restart();
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn set_window_title(window: tauri::Window, title: String) -> Result<(), String> {
    window.set_title(&title).map_err(|e| e.to_string())
}

/// Computes the active source / destination geometry for a given
/// `source_id`. The Frontend calls this to preview the rect that will
/// be captured before pressing record, and to surface fit vs. fill
/// decisions in the settings UI.
#[cfg(feature = "tauri-app")]
#[tauri::command]
fn compute_source_geometry(
    source_id: String,
    dest_width: Option<u32>,
    dest_height: Option<u32>,
    fit_mode: Option<capture::FitMode>,
) -> Result<commands::SourceGeometryResult, String> {
    commands::compute_source_geometry_impl(source_id, dest_width, dest_height, fit_mode)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn detect_silence(
    app: tauri::AppHandle,
    project_handle: String,
    track_id: String,
    config: dsp::SilenceConfig,
) -> Result<dsp::SilenceDetectionResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        commands::detect_silence_impl(&app.state::<AppState>(), project_handle, track_id, config)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn open_project(
    app: tauri::AppHandle,
    path: String,
) -> Result<project::OpenedProject, String> {
    let app_handle = app.clone();
    let opened = tauri::async_runtime::spawn_blocking(move || {
        commands::open_project_impl(&app_handle.state::<AppState>(), path)
    })
    .await
    .map_err(|e| e.to_string())??;

    if let Some(window) = app.get_webview_window("main") {
        let title = commands::window_title_for_project(Some(&opened.manifest.project_name));
        let _ = window.set_title(&title);
    }

    Ok(opened)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn close_project(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    project_handle: String,
) -> Result<(), String> {
    commands::close_project_impl(&state, project_handle)?;
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_title(&commands::window_title_for_project(None));
    }
    Ok(())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn project_segments(
    state: State<'_, AppState>,
    project_handle: String,
    track_id: String,
    offset: usize,
    limit: usize,
) -> Result<project::SegmentPage, String> {
    commands::project_segments_impl(&state, project_handle, track_id, offset, limit)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn project_waveform(
    app: tauri::AppHandle,
    project_handle: String,
    track_id: String,
    start_us: u64,
    end_us: u64,
    bucket_count: usize,
) -> Result<project::WaveformPage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        commands::project_waveform_impl(
            &app.state::<AppState>(),
            project_handle,
            track_id,
            start_us,
            end_us,
            bucket_count,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn project_zoom_suggestions(
    app: tauri::AppHandle,
    project_handle: String,
    config: Option<zoom::ZoomConfig>,
) -> Result<zoom::ZoomGeneration, String> {
    tauri::async_runtime::spawn_blocking(move || {
        commands::project_zoom_suggestions_impl(&app.state::<AppState>(), project_handle, config)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn project_zoom_accept(
    state: State<'_, AppState>,
    project_handle: String,
    expected_revision: u64,
    ids: Vec<String>,
) -> Result<project::OpenedProject, String> {
    commands::project_zoom_accept_impl(&state, project_handle, expected_revision, ids)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn project_zoom_dismiss(
    state: State<'_, AppState>,
    project_handle: String,
    expected_revision: u64,
    ids: Vec<String>,
) -> Result<project::OpenedProject, String> {
    commands::project_zoom_dismiss_impl(&state, project_handle, expected_revision, ids)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn project_zoom_update(
    state: State<'_, AppState>,
    project_handle: String,
    expected_revision: u64,
    zoom: zoom::ZoomKeyframe,
) -> Result<project::OpenedProject, String> {
    commands::project_zoom_update_impl(&state, project_handle, expected_revision, zoom)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn project_zoom_add(
    state: State<'_, AppState>,
    project_handle: String,
    expected_revision: u64,
    input: commands::ManualZoomInput,
) -> Result<project::OpenedProject, String> {
    commands::project_zoom_add_impl(&state, project_handle, expected_revision, input)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn project_zoom_delete(
    state: State<'_, AppState>,
    project_handle: String,
    expected_revision: u64,
    id: String,
) -> Result<project::OpenedProject, String> {
    commands::project_zoom_delete_impl(&state, project_handle, expected_revision, id)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn project_layout_update(
    state: State<'_, AppState>,
    project_handle: String,
    expected_revision: u64,
    layout: project::EditLayout,
    wallpaper_source: Option<String>,
) -> Result<project::OpenedProject, String> {
    commands::project_layout_update_impl(
        &state,
        project_handle,
        expected_revision,
        layout,
        wallpaper_source,
    )
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn project_ripple_cuts(
    state: State<'_, AppState>,
    project_handle: String,
    expected_revision: u64,
    cuts: Vec<commands::EditCut>,
) -> Result<project::OpenedProject, String> {
    commands::project_ripple_cuts_impl(&state, project_handle, expected_revision, cuts)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn project_undo(
    state: State<'_, AppState>,
    project_handle: String,
    expected_revision: u64,
) -> Result<project::OpenedProject, String> {
    commands::project_undo_impl(&state, project_handle, expected_revision)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn project_redo(
    state: State<'_, AppState>,
    project_handle: String,
    expected_revision: u64,
) -> Result<project::OpenedProject, String> {
    commands::project_redo_impl(&state, project_handle, expected_revision)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn project_rename(
    state: State<'_, AppState>,
    project_handle: String,
    new_name: String,
) -> Result<project::OpenedProject, String> {
    commands::project_rename_impl(&state, project_handle, new_name)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn playback_status(
    state: State<'_, AppState>,
    project_handle: String,
) -> Result<playback::PlaybackStatus, String> {
    commands::playback_status_impl(&state, project_handle)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn playback_play(
    state: State<'_, AppState>,
    project_handle: String,
) -> Result<playback::PlaybackStatus, String> {
    commands::playback_play_impl(&state, project_handle)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn playback_pause(
    state: State<'_, AppState>,
    project_handle: String,
) -> Result<playback::PlaybackStatus, String> {
    commands::playback_pause_impl(&state, project_handle)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn playback_seek(
    state: State<'_, AppState>,
    project_handle: String,
    edited_us: u64,
) -> Result<playback::PlaybackStatus, String> {
    commands::playback_seek_impl(&state, project_handle, edited_us)
}

#[cfg(feature = "tauri-app")]
fn preview_ns_window(
    app: &tauri::AppHandle,
    window_label: &str,
) -> Result<*mut std::ffi::c_void, String> {
    let window = app
        .get_webview_window(window_label)
        .ok_or_else(|| format!("Unknown window label: {window_label}"))?;
    #[cfg(target_os = "macos")]
    {
        window
            .ns_window()
            .map_err(|e| format!("Failed to get NSWindow: {e}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Err("Native preview is not implemented on this platform".into())
    }
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn preview_attach(
    app: tauri::AppHandle,
    window_label: String,
    hit_mode: playback::PreviewHitMode,
) -> Result<playback::PreviewStatus, String> {
    let ns_window = Some(preview_ns_window(&app, &window_label)?);
    commands::preview_attach_impl(&app.state::<AppState>(), window_label, hit_mode, ns_window)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn preview_layout(
    state: State<'_, AppState>,
    viewport: playback::PreviewViewport,
) -> Result<playback::PreviewStatus, String> {
    commands::preview_layout_impl(&state, viewport)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn preview_present_fixed(
    state: State<'_, AppState>,
    r: f32,
    g: f32,
    b: f32,
    generation: Option<u64>,
) -> Result<playback::PreviewStatus, String> {
    commands::preview_present_fixed_impl(&state, r, g, b, generation.unwrap_or(0))
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn preview_present_fixture(
    state: State<'_, AppState>,
    path: String,
    generation: Option<u64>,
) -> Result<playback::PreviewStatus, String> {
    commands::preview_present_fixture_impl(&state, path, generation.unwrap_or(0))
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn preview_status(state: State<'_, AppState>) -> playback::PreviewStatus {
    commands::preview_status_impl(&state)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn preview_hit_test(state: State<'_, AppState>, x: f64, y: f64) -> bool {
    commands::preview_hit_test_impl(&state, x, y)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn preview_detach(
    state: State<'_, AppState>,
    window_label: String,
    generation: Option<u64>,
) -> Result<playback::PreviewStatus, String> {
    if generation.is_some_and(|g| g != state.preview.lock().status().generation) {
        return Err("Stale preview generation".into());
    }
    commands::preview_detach_impl(&state, window_label)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn window_identity(window: tauri::Window) -> hud::WindowIdentity {
    hud::window_identity_from_label(window.label())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn hud_snapshot(state: State<'_, AppState>) -> hud::HudSnapshot {
    commands::hud_snapshot_impl(&state)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn hud_update(
    app: tauri::AppHandle,
    expected_revision: u64,
    patch: hud::HudSettingsPatch,
) -> Result<hud::HudSnapshot, String> {
    let snapshot = commands::hud_update_impl(&app.state::<AppState>(), expected_revision, patch)?;
    sync_hud_window(&app, &snapshot);
    Ok(snapshot)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn hud_reconcile_cameras(
    app: tauri::AppHandle,
    cameras: Vec<hud::HudCameraInfo>,
    selected_camera_id: Option<String>,
) -> Result<hud::HudSnapshot, String> {
    let snapshot = commands::hud_reconcile_cameras_impl(
        &app.state::<AppState>(),
        cameras,
        selected_camera_id,
    )?;
    emit_hud(&app, &snapshot);
    Ok(snapshot)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn hud_preview_attach(
    app: tauri::AppHandle,
    window_label: String,
    hit_mode: playback::PreviewHitMode,
) -> Result<hud::HudSnapshot, String> {
    let ns_window = Some(preview_ns_window(&app, &window_label)?);
    let snapshot = commands::hud_preview_attach_impl(
        &app.state::<AppState>(),
        window_label,
        hit_mode,
        ns_window,
    )?;
    emit_hud(&app, &snapshot);
    Ok(snapshot)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn hud_preview_layout(
    state: State<'_, AppState>,
    viewport: playback::PreviewViewport,
) -> Result<playback::PreviewStatus, String> {
    commands::hud_preview_layout_impl(&state, viewport)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn hud_preview_status(state: State<'_, AppState>) -> playback::PreviewStatus {
    commands::hud_preview_status_impl(&state)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn hud_close(app: tauri::AppHandle) -> Result<hud::HudSnapshot, String> {
    let snapshot = commands::hud_close_impl(&app.state::<AppState>())?;
    sync_hud_window(&app, &snapshot);
    Ok(snapshot)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn hud_set_visible(app: tauri::AppHandle, visible: bool) -> Result<hud::HudSnapshot, String> {
    let snapshot = commands::hud_set_visible_impl(&app.state::<AppState>(), visible)?;
    sync_hud_window(&app, &snapshot);
    Ok(snapshot)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn media_interop_status(state: State<'_, AppState>) -> media::MediaInteropStatus {
    commands::media_interop_status_impl(&state)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn media_run_parity(state: State<'_, AppState>) -> Result<media::MediaParityReport, String> {
    commands::media_run_parity_impl(&state)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn export_start(
    state: State<'_, AppState>,
    project_handle: String,
    settings: export::ExportSettings,
) -> Result<export::ExportStatus, String> {
    commands::export_start_impl(&state, project_handle, settings)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn export_status(
    state: State<'_, AppState>,
    job_id: Option<String>,
) -> Result<export::ExportStatus, String> {
    commands::export_status_impl(&state, job_id)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn export_cancel(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<export::ExportStatus, String> {
    commands::export_cancel_impl(&state, job_id)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_default_projects_dir() -> String {
    commands::get_default_projects_dir_impl()
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn pick_project_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    pick_directory_dialog(
        app,
        "Open AeroShoot Project",
        commands::default_projects_dir(),
    )
    .await
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn pick_save_directory(app: tauri::AppHandle) -> Result<Option<String>, String> {
    pick_directory_dialog(
        app,
        "Choose where to save recordings",
        commands::default_projects_dir(),
    )
    .await
}

#[cfg(feature = "tauri-app")]
async fn pick_directory_dialog(
    app: tauri::AppHandle,
    title: &'static str,
    directory: std::path::PathBuf,
) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            let mut dialog = rfd::FileDialog::new().set_title(title);
            if directory.is_dir() {
                dialog = dialog.set_directory(&directory);
            }
            let picked = dialog
                .pick_folder()
                .map(|path| path.to_string_lossy().into_owned());
            let _ = tx.send(picked);
        })
        .map_err(|error| error.to_string())?;
        rx.recv().map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn pick_wallpaper_source(app: tauri::AppHandle) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            let picked = rfd::FileDialog::new()
                .set_title("Choose wallpaper")
                .add_filter("Images", &["png", "jpg", "jpeg"])
                .pick_file()
                .map(|path| path.to_string_lossy().into_owned());
            let _ = tx.send(picked);
        })
        .map_err(|error| error.to_string())?;
        rx.recv().map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn pick_export_destination(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    project_handle: Option<String>,
) -> Result<Option<String>, String> {
    let (default_dir, default_filename, bundle_root) = {
        let opened = state.opened_project.lock();
        if let Some(reader) = opened.as_ref() {
            if let Some(ref handle) = project_handle {
                if reader.summary.project_handle != *handle {
                    return Err("Stale project handle".into());
                }
            }
            let root = reader.root().to_path_buf();
            let parent = root.parent().unwrap_or(&root).to_path_buf();
            let project_name = reader.summary.manifest.project_name.clone();
            let filename = crate::export::default_export_filename(&project_name);
            (parent, filename, Some(root))
        } else {
            (
                commands::default_projects_dir(),
                "Untitled.mp4".to_string(),
                None,
            )
        }
    };
    pick_save_file_dialog(
        app,
        "Export Video",
        default_dir,
        default_filename,
        bundle_root,
    )
    .await
}

#[cfg(feature = "tauri-app")]
async fn pick_save_file_dialog(
    app: tauri::AppHandle,
    title: &'static str,
    directory: std::path::PathBuf,
    default_filename: String,
    bundle_root: Option<std::path::PathBuf>,
) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            let mut dialog = rfd::FileDialog::new()
                .set_title(title)
                .set_file_name(&default_filename)
                .add_filter("MP4 Video", &["mp4"]);
            if directory.is_dir() {
                dialog = dialog.set_directory(&directory);
            }
            let picked = dialog
                .save_file()
                .map(|path| path.to_string_lossy().into_owned());
            let _ = tx.send(picked);
        })
        .map_err(|error| error.to_string())?;
        let picked = rx.recv().map_err(|error| error.to_string())?;
        if let Some(ref path_str) = picked {
            let path = std::path::PathBuf::from(path_str);
            if crate::export::is_inside_bundle(&path, bundle_root.as_deref()) {
                return Err("Export destination cannot be inside the project bundle".into());
            }
        }
        Ok(picked)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn show_in_finder(path: String) -> Result<(), String> {
    commands::show_in_finder_impl(path)
}

#[cfg(feature = "tauri-app")]
pub fn run() {
    tauri::Builder::default()
        .manage(commands::AppState::default())
        .setup(|app| {
            playback::engine::start(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                tauri::WindowEvent::CloseRequested { api, .. }
                    if window.label() == hud::HUD_WINDOW_LABEL =>
                {
                    api.prevent_close();
                    let snapshot = commands::hud_set_visible_impl(&window.state::<AppState>(), false);
                    if let Ok(snapshot) = snapshot {
                        let _ = window.hide();
                        emit_hud(window.app_handle(), &snapshot);
                    }
                }
                tauri::WindowEvent::Destroyed if window.label() == hud::HUD_WINDOW_LABEL => {
                    let _ = commands::hud_close_impl(&window.state::<AppState>());
                }
                tauri::WindowEvent::Destroyed => {
                    let _ = commands::preview_detach_impl(
                        &window.state::<AppState>(),
                        window.label().to_string(),
                    );
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            open_project,
            close_project,
            project_rename,
            project_segments,
            project_waveform,
            project_zoom_suggestions,
            project_zoom_accept,
            project_zoom_dismiss,
            project_zoom_update,
            project_zoom_add,
            project_zoom_delete,
            project_layout_update,
            project_ripple_cuts,
            project_undo,
            project_redo,
            playback_status,
            playback_play,
            playback_pause,
            playback_seek,
            preview_attach,
            preview_layout,
            preview_present_fixed,
            preview_present_fixture,
            preview_status,
            preview_hit_test,
            preview_detach,
            window_identity,
            hud_snapshot,
            hud_update,
            hud_reconcile_cameras,
            hud_preview_attach,
            hud_preview_layout,
            hud_preview_status,
            hud_close,
            hud_set_visible,
            media_interop_status,
            media_run_parity,
            export_start,
            export_status,
            export_cancel,
            capture_preview_configure,
            list_capture_sources,
            list_devices,
            start_recording,
            pause_recording,
            resume_recording,
            stop_recording,
            get_session_status,
            get_permission_status,
            request_capture_permissions,
            mouse_telemetry_permission,
            open_system_privacy_settings,
            restart_app,
            compute_source_geometry,
            detect_silence,
            get_default_projects_dir,
            pick_project_folder,
            pick_save_directory,
            pick_export_destination,
            pick_wallpaper_source,
            set_window_title,
            show_in_finder
        ])
        .build(tauri::generate_context!())
        .expect("error while building aero shoot tauri application")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                app.state::<AppState>()
                    .playback_shutdown
                    .store(true, std::sync::atomic::Ordering::Release);
            }
        });
}
