pub mod capture;
pub mod commands;
pub mod fixtures;
pub mod hud;
pub mod project;
pub mod session;
pub mod telemetry;

#[cfg(feature = "tauri-app")]
use commands::*;
#[cfg(feature = "tauri-app")]
use tauri::{Manager, State};

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
    #[cfg(target_os = "windows")]
    {
        window
            .hwnd()
            .map(|hwnd| hwnd.0 as *mut std::ffi::c_void)
            .map_err(|e| format!("Failed to get the window handle: {e}"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = window;
        Err("Native preview is not implemented on this platform".into())
    }
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn capture_preview_configure(
    app: tauri::AppHandle,
    enabled: bool,
    source_id: Option<String>,
    camera_id: Option<String>,
    mic_id: Option<String>,
    mic_gain_db: Option<f64>,
    capture_screen: Option<bool>,
    capture_system_audio: Option<bool>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _guard = state.command_lock.lock();
        // An active recording owns capture and feeds the same preview mailbox.
        if state.active_session.read().is_some() {
            return Ok(());
        }
        if !enabled {
            capture::preview::stop();
            return Ok(());
        }
        capture::preview::start(
            source_id.as_deref().ok_or("Select a screen source")?,
            capture_screen.unwrap_or(true),
            capture_system_audio.unwrap_or(false),
            camera_id.as_deref(),
            mic_id.as_deref(),
            mic_gain_db.unwrap_or(0.0),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn capture_preview_audio_levels() -> capture::preview::PreviewAudioLevels {
    capture::preview::audio_levels()
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
    let app_handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        commands::start_recording_impl(&app_handle.state::<AppState>(), options)
    })
    .await
    .map_err(|e| e.to_string())??;
    if let Some(session) = app.state::<AppState>().active_session.read().as_ref() {
        if let Some(window) = app.get_webview_window("main") {
            let title = commands::window_title_for_recording(Some(&session.project_name));
            let _ = window.set_title(&title);
        }
    }
    Ok(result)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn pause_recording(app: tauri::AppHandle) -> Result<commands::SessionStateResult, String> {
    // Sync commands run on the main thread; the lifecycle lock can be held for
    // seconds by start/stop, so wait for it on a worker instead.
    tauri::async_runtime::spawn_blocking(move || {
        commands::pause_recording_impl(&app.state::<AppState>())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn resume_recording(app: tauri::AppHandle) -> Result<commands::SessionStateResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        commands::resume_recording_impl(&app.state::<AppState>())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn stop_recording(app: tauri::AppHandle) -> Result<commands::StopRecordingResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        commands::stop_recording_impl(&app.state::<AppState>())
    })
    .await
    .map_err(|e| e.to_string())?
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
    if capture::backend::MOUSE_TELEMETRY {
        crate::telemetry::native::MouseTelemetryPermission::macos(
            capture::backend::mouse_permission(request),
        )
    } else {
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
fn studio_preview_attach(
    app: tauri::AppHandle,
    window_label: String,
    hit_mode: hud::PreviewHitMode,
) -> Result<hud::PreviewStatus, String> {
    let ns_window = Some(preview_ns_window(&app, &window_label)?);
    commands::studio_preview_attach_impl(
        &app.state::<AppState>(),
        window_label,
        hit_mode,
        ns_window,
    )
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn studio_preview_layout(
    state: State<'_, AppState>,
    viewport: hud::PreviewViewport,
) -> Result<hud::PreviewStatus, String> {
    commands::studio_preview_layout_impl(&state, viewport)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn studio_preview_status(state: State<'_, AppState>) -> hud::PreviewStatus {
    commands::studio_preview_status_impl(&state)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn studio_preview_detach(state: State<'_, AppState>) -> Result<hud::PreviewStatus, String> {
    commands::studio_preview_detach_impl(&state)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_default_projects_dir() -> String {
    commands::get_default_projects_dir_impl()
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
fn show_in_finder(path: String) -> Result<(), String> {
    commands::show_in_finder_impl(path)
}

#[cfg(feature = "tauri-app")]
pub fn run() {
    tauri::Builder::default()
        .manage(commands::AppState::default())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == hud::STUDIO_WINDOW_LABEL {
                    // Closing the recorder quits the app. Finalize an active
                    // recording first so its project is complete.
                    api.prevent_close();
                    // Ignore further close requests while the first one is
                    // still stopping the recording.
                    static QUIT_IN_PROGRESS: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    if QUIT_IN_PROGRESS.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    let app = window.app_handle().clone();
                    std::thread::spawn(move || {
                        let state = app.state::<AppState>();
                        // A start still in its countdown creates the session
                        // when it ends; wait for it so that recording is
                        // stopped and saved instead of left half-created.
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(20);
                        while state.state_machine.current() == session::SessionState::Preparing
                            && std::time::Instant::now() < deadline
                        {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        if state.active_session.read().is_some() {
                            if let Err(error) = commands::stop_recording_impl(&state) {
                                eprintln!(
                                    "[AeroShoot] Stopping the recording before quit failed: {error}"
                                );
                            }
                        }
                        app.exit(0);
                    });
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_capture_sources,
            list_devices,
            compute_source_geometry,
            capture_preview_configure,
            capture_preview_audio_levels,
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
            studio_preview_attach,
            studio_preview_layout,
            studio_preview_status,
            studio_preview_detach,
            get_default_projects_dir,
            pick_save_directory,
            set_window_title,
            show_in_finder
        ])
        .build(tauri::generate_context!())
        .expect("error while building aero shoot tauri application")
        .run(|app, event| {
            // Clicking the Dock icon with no visible window brings the recorder back.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } = event
            {
                if let Some(window) = app.get_webview_window(hud::STUDIO_WINDOW_LABEL) {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}
