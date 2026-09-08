pub mod capture;
pub mod commands;
pub mod dsp;
pub mod fixtures;
pub mod project;
pub mod session;
pub mod telemetry;
pub mod timeline;

#[cfg(feature = "tauri-app")]
use commands::*;
#[cfg(feature = "tauri-app")]
use tauri::State;

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
fn get_permission_status(state: State<'_, AppState>) -> capture::PermissionStatus {
    commands::get_permission_status_impl(&state)
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
pub fn run() {
    tauri::Builder::default()
        .manage(commands::AppState::default())
        .invoke_handler(tauri::generate_handler![
            list_capture_sources,
            list_devices,
            start_recording,
            pause_recording,
            resume_recording,
            stop_recording,
            get_session_status,
            get_permission_status,
            detect_silence
        ])
        .run(tauri::generate_context!())
        .expect("error while running aero shoot tauri application");
}
