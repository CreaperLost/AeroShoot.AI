pub mod capture;
pub mod commands;
pub mod dsp;
pub mod export;
pub mod fixtures;
pub mod media;
pub mod playback;
pub mod project;
pub mod render;
pub mod session;
pub mod telemetry;
pub mod timeline;

#[cfg(feature = "tauri-app")]
use commands::*;
#[cfg(feature = "tauri-app")]
use tauri::{Manager, State};

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
fn start_recording(
    state: State<'_, AppState>,
    options: commands::StartRecordingOptions,
) -> Result<commands::StartRecordingResult, String> {
    commands::start_recording_impl(&state, options)
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
fn stop_recording(state: State<'_, AppState>) -> Result<commands::StopRecordingResult, String> {
    commands::stop_recording_impl(&state)
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
fn mouse_telemetry_permission(request: bool) -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        serde_json::json!({ "supported": true, "authorized": capture::macos::mouse_permission(request) })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = request;
        serde_json::json!({ "supported": false, "authorized": false })
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
fn detect_silence(config: dsp::SilenceConfig) -> Vec<dsp::SilenceCutInterval> {
    let sample_rate = 48000;
    let mut mock_samples = vec![0.5f32; sample_rate as usize]; // 1s speech
    mock_samples.extend(vec![0.0f32; sample_rate as usize * 2]); // 2s silence
    mock_samples.extend(vec![0.5f32; sample_rate as usize]); // 1s speech
    commands::detect_silence_impl(&mock_samples, sample_rate, &config)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn open_project(
    app: tauri::AppHandle,
    path: String,
) -> Result<project::OpenedProject, String> {
    tauri::async_runtime::spawn_blocking(move || {
        commands::open_project_impl(&app.state::<AppState>(), path)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn close_project(state: State<'_, AppState>, project_handle: String) -> Result<(), String> {
    commands::close_project_impl(&state, project_handle)
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
    if generation.is_some_and(|g| g != state.preview.lock().status().generation) { return Err("Stale preview generation".into()); }
    commands::preview_detach_impl(&state, window_label)
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
    let directory = commands::default_projects_dir();
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            let mut dialog = rfd::FileDialog::new().set_title("Open AeroShoot Project");
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
pub fn run() {
    tauri::Builder::default()
        .manage(commands::AppState::default())
        .setup(|app| { playback::engine::start(app.handle().clone()); Ok(()) })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                let _ = commands::preview_detach_impl(
                    &window.state::<AppState>(),
                    window.label().to_string(),
                );
            }
        })
        .invoke_handler(tauri::generate_handler![
            open_project,
            close_project,
            project_segments,
            project_waveform,
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
            media_interop_status,
            media_run_parity,
            export_start,
            export_status,
            export_cancel,
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
            pick_project_folder
        ])
        .build(tauri::generate_context!())
        .expect("error while building aero shoot tauri application")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) { app.state::<AppState>().playback_shutdown.store(true, std::sync::atomic::Ordering::Release); }
        });
}
