use super::{AudioDevice, CameraDevice, CaptureSource, PermissionState, PermissionStatus, SourceRect};
use crate::project::manifest::{ProjectManifest, TrackType};
use crate::project::media_validator::MediaValidator;
use crate::session::SessionEvent;
use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;
use std::sync::{Condvar, OnceLock};
use std::time::Duration;

// ---------------------------------------------------------------------------
// C ABI surface for the macOS native bridge.
//
// The Swift side (AeroShootCapture.swift) implements these symbols. They
// match the contract documented at the top of this file. The Rust side
// uses them to (a) drive stop, (b) receive per-segment callbacks, and
// (c) surface runtime errors back to the session state machine.
//
// All `#[repr(C)]` types below must stay binary-compatible with their
// Swift counterparts. The companion file in `native/macos/AeroShootCapture.swift`
// is the Swift agent's ownership and is updated alongside these.
// ---------------------------------------------------------------------------

/// Result of a Swift stop call. Mirrors the Swift
/// `AeroShootEncoderResult` C struct.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AeroShootEncoderResult {
    pub status: AeroShootEncoderStatus,
    pub error_code: i32,
    pub error_message: [c_char; 256],
}

/// Encoder status code returned by the Swift stop call. Mirrors the Swift
/// `AeroShootEncoderStatus` C enum.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum AeroShootEncoderStatus {
    Ok = 0,
    Backpressured = 1,
    Failed = 2,
    Timeout = 3,
}

impl AeroShootEncoderStatus {
    pub fn from_c(value: i32) -> Self {
        match value {
            0 => AeroShootEncoderStatus::Ok,
            1 => AeroShootEncoderStatus::Backpressured,
            3 => AeroShootEncoderStatus::Timeout,
            _ => AeroShootEncoderStatus::Failed,
        }
    }

    pub fn is_ok(self) -> bool {
        matches!(self, AeroShootEncoderStatus::Ok)
    }
}

/// Typed permission state matching the Swift `AeroShootPermissionState` C enum.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AeroShootPermissionState {
    Unknown = 0,
    NotDetermined = 1,
    Authorized = 2,
    Denied = 3,
    Restricted = 4,
}

/// Typed permission bundle matching the Swift `AeroShootPermissionBundle` C struct.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AeroShootPermissionBundle {
    pub screen_recording: AeroShootPermissionState,
    pub camera: AeroShootPermissionState,
    pub microphone: AeroShootPermissionState,
}

// Opaque `repr(C)` enums so the function-pointer types compile without
// depending on the Swift agent having imported the exact `#[repr(C)]` enum
// at the FFI boundary. The Swift side is expected to pass the underlying
// `c_int` discriminant.

/// C function pointer type for the segment callback. The file_path C string
/// is owned by the caller and is valid only for the duration of the call.
pub type AeroShootSegmentCallback = unsafe extern "C" fn(
    track_id: *const c_char,
    host_anchor_us: i64,
    segment_index: i32,
    timescale: i32,
    media_start_value: i64,
    file_path: *const c_char,
);

/// C function pointer type for the runtime-error callback.
pub type AeroShootRuntimeErrorCallback =
    unsafe extern "C" fn(track_id: *const c_char, error_code: c_int, message: *const c_char);

/// C function pointer type for the permission completion callback.
pub type AeroShootPermissionCompletion =
    unsafe extern "C" fn(screen_recording: i32, camera: i32, microphone: i32);

extern "C" {
    fn aeroshoot_macos_copy_sources_json() -> *mut c_char;
    fn aeroshoot_macos_copy_devices_json() -> *mut c_char;
    fn aeroshoot_check_permissions(out_bundle: *mut AeroShootPermissionBundle);
    fn aeroshoot_request_permissions(
        screen: bool,
        camera: bool,
        microphone: bool,
        completion: Option<AeroShootPermissionCompletion>,
    );
    fn aeroshoot_macos_start(
        config_json: *const c_char,
        error_out: *mut *mut c_char,
    ) -> *mut c_void;
    fn aeroshoot_macos_set_paused(handle: *mut c_void, paused: bool);
    fn aeroshoot_macos_copy_stats_json(handle: *mut c_void) -> *mut c_char;
    /// Old stop symbol — kept for source compatibility; the new
    /// `aeroshoot_macos_stop_capture` returns a typed result.
    fn aeroshoot_macos_stop(handle: *mut c_void);
    /// New stop symbol that surfaces encoder status to Rust.
    fn aeroshoot_macos_stop_capture(
        handle: *mut c_void,
        out_result: *mut AeroShootEncoderResult,
    );
    /// New optional Swift-side callback registration entry points. The
    /// Swift agent wires these to a static function pointer so callbacks
    /// fire from its serial queue.
    fn aeroshoot_macos_register_segment_callback(
        handle: *mut c_void,
        cb: Option<AeroShootSegmentCallback>,
    );
    fn aeroshoot_macos_register_runtime_error_callback(
        handle: *mut c_void,
        cb: Option<AeroShootRuntimeErrorCallback>,
    );
    fn aeroshoot_macos_free_string(value: *mut c_char);
}

// ---------------------------------------------------------------------------
// Test-only stubs for the Swift FFI symbols.
//
// When `AEROSHOOT_SKIP_SWIFT=1` is set, the Swift bridge is not built,
// so the linker has no implementation for the extern "C" symbols above.
// We provide weak stub implementations here that are only emitted when
// the Rust crate is being compiled in test mode AND the Swift build is
// being skipped. This lets the Rust unit tests (which never actually
// invoke the FFI symbols) link successfully.
//
// In production, the Swift static library provides the real symbols and
// these stubs are not emitted.
// ---------------------------------------------------------------------------

#[cfg(all(test, stub_swift_ffi))]
mod stub_swift_ffi {
    use super::*;
    use std::os::raw::c_int;

    /// Helper: returns a heap-allocated C string the test harness will
    /// `free_string` once it copies the bytes. We use `libc::strdup` so
    /// the allocator matches what `aeroshoot_macos_free_string` expects.
    fn dup_cstr(s: &str) -> *mut c_char {
        unsafe { libc::strdup(s.as_ptr() as *const i8) }
    }

    #[no_mangle]
    pub extern "C" fn aeroshoot_macos_copy_sources_json() -> *mut c_char {
        // Mirror the mock list in `list_capture_sources_impl` so the
        // geometry tests can look up a known source ID.
        dup_cstr(
            r#"[
                {"id":"screen-main","name":"Main Display","sourceType":"display","width":2560,"height":1440},
                {"id":"screen-secondary","name":"Secondary Display","sourceType":"display","width":1920,"height":1080},
                {"id":"win-active","name":"Active Code Workspace","sourceType":"window","width":1920,"height":1080}
            ]"#,
        )
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_macos_copy_devices_json() -> *mut c_char {
        dup_cstr(
            r#"{"cameras":[],"mics":[]}"#,
        )
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_check_permissions(out: *mut AeroShootPermissionBundle) {
        if let Some(out) = unsafe { out.as_mut() } {
            *out = AeroShootPermissionBundle {
                screen_recording: AeroShootPermissionState::Authorized,
                camera: AeroShootPermissionState::Authorized,
                microphone: AeroShootPermissionState::Authorized,
            };
        }
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_request_permissions(
        _screen: bool,
        _camera: bool,
        _microphone: bool,
        completion: Option<AeroShootPermissionCompletion>,
    ) {
        if let Some(completion) = completion {
            unsafe { completion(2, 2, 2) };
        }
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_macos_start(
        _config_json: *const c_char,
        _error_out: *mut *mut c_char,
    ) -> *mut c_void {
        // Return a non-null dummy handle so the Rust side can store it
        // and call stop / set_paused on it during tests.
        1 as *mut c_void
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_macos_set_paused(_handle: *mut c_void, _paused: bool) {}
    #[no_mangle]
    pub extern "C" fn aeroshoot_macos_copy_stats_json(
        _handle: *mut c_void,
    ) -> *mut c_char {
        dup_cstr(
            r#"{"droppedFrames":0,"audioBufferUnderflows":0,"lastError":null}"#,
        )
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_macos_stop(_handle: *mut c_void) {}
    #[no_mangle]
    pub extern "C" fn aeroshoot_macos_stop_capture(
        _handle: *mut c_void,
        out_result: *mut AeroShootEncoderResult,
    ) {
        if !out_result.is_null() {
            unsafe {
                *out_result = AeroShootEncoderResult {
                    status: AeroShootEncoderStatus::Ok,
                    error_code: 0,
                    error_message: [0i8; 256],
                };
            }
        }
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_macos_register_segment_callback(
        _handle: *mut c_void,
        _cb: Option<AeroShootSegmentCallback>,
    ) {
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_macos_register_runtime_error_callback(
        _handle: *mut c_void,
        _cb: Option<AeroShootRuntimeErrorCallback>,
    ) {
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_macos_free_string(value: *mut c_char) {
        unsafe { libc::free(value as *mut c_void) };
    }

    /// Export a sentinel for test discovery so we can confirm the stubs
    /// are present in the test binary.
    #[no_mangle]
    pub extern "C" fn aeroshoot_rust_stub_swift_ffi_marker() -> c_int {
        1
    }
}

fn take_string(pointer: *mut c_char) -> Result<String, String> {
    let pointer = NonNull::new(pointer).ok_or_else(|| "native bridge returned null".to_string())?;
    let value = unsafe { CStr::from_ptr(pointer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    unsafe { aeroshoot_macos_free_string(pointer.as_ptr()) };
    Ok(value)
}

pub fn capture_sources() -> Result<Vec<CaptureSource>, String> {
    let json = take_string(unsafe { aeroshoot_macos_copy_sources_json() })?;
    serde_json::from_str(&json).map_err(|error| format!("invalid source list from macOS: {error}"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Devices {
    cameras: Vec<CameraDevice>,
    mics: Vec<AudioDevice>,
}

pub fn devices() -> Result<(Vec<CameraDevice>, Vec<AudioDevice>), String> {
    let json = take_string(unsafe { aeroshoot_macos_copy_devices_json() })?;
    let devices: Devices = serde_json::from_str(&json)
        .map_err(|error| format!("invalid device list from macOS: {error}"))?;
    Ok((devices.cameras, devices.mics))
}

fn permission_state(value: AeroShootPermissionState) -> PermissionState {
    match value {
        AeroShootPermissionState::Unknown => PermissionState::Unknown,
        AeroShootPermissionState::NotDetermined => PermissionState::NotDetermined,
        AeroShootPermissionState::Authorized => PermissionState::Authorized,
        AeroShootPermissionState::Denied => PermissionState::Denied,
        AeroShootPermissionState::Restricted => PermissionState::Restricted,
    }
}

fn permission_status(bundle: AeroShootPermissionBundle) -> PermissionStatus {
    PermissionStatus {
        screen_recording: permission_state(bundle.screen_recording),
        camera: permission_state(bundle.camera),
        microphone: permission_state(bundle.microphone),
    }
}

pub fn permissions() -> PermissionStatus {
    let mut bundle = AeroShootPermissionBundle {
        screen_recording: AeroShootPermissionState::Unknown,
        camera: AeroShootPermissionState::Unknown,
        microphone: AeroShootPermissionState::Unknown,
    };
    unsafe { aeroshoot_check_permissions(&mut bundle) };
    permission_status(bundle)
}

static PERMISSION_WAIT: OnceLock<(Mutex<Option<AeroShootPermissionBundle>>, Condvar)> = OnceLock::new();

unsafe extern "C" fn permission_completion(screen: i32, camera: i32, microphone: i32) {
    fn decode(value: i32) -> AeroShootPermissionState {
        match value {
            1 => AeroShootPermissionState::NotDetermined,
            2 => AeroShootPermissionState::Authorized,
            3 => AeroShootPermissionState::Denied,
            4 => AeroShootPermissionState::Restricted,
            _ => AeroShootPermissionState::Unknown,
        }
    }
    let (slot, ready) = PERMISSION_WAIT.get_or_init(|| (Mutex::new(None), Condvar::new()));
    *slot.lock().unwrap() = Some(AeroShootPermissionBundle {
        screen_recording: decode(screen),
        camera: decode(camera),
        microphone: decode(microphone),
    });
    ready.notify_all();
}

pub fn request_permissions(screen: bool, camera: bool, microphone: bool) -> PermissionStatus {
    let (slot, ready) = PERMISSION_WAIT.get_or_init(|| (Mutex::new(None), Condvar::new()));
    let mut guard = slot.lock().unwrap();
    *guard = None;
    unsafe { aeroshoot_request_permissions(screen, camera, microphone, Some(permission_completion)) };
    let (mut guard, _) = ready
        .wait_timeout_while(guard, Duration::from_secs(31), |result| result.is_none())
        .unwrap();
    guard.take().map(permission_status).unwrap_or_else(permissions)
}

// ---------------------------------------------------------------------------
// Callback routing.
//
// The C FFI uses a single global callback slot per kind (segment,
// runtime-error). We route foreign-thread callbacks into a tokio task
// before mutating session state by:
//
//   1. Storing the destination `SessionDiagnostics` in a global
//      `Mutex<Option<...>>` set when a session starts and cleared when
//      it stops.
//   2. The C wrapper pulls the destination out of the slot, marshals
//      the args, and dispatches via `tokio::spawn` (if a runtime is
//      available) or falls back to a direct synchronous call so we
//      never block the foreign thread longer than necessary.
// ---------------------------------------------------------------------------

static RUNTIME_ERROR_TARGET: Mutex<Option<RuntimeErrorTarget>> = Mutex::new(None);
static SEGMENT_TARGET: Mutex<Option<SegmentTarget>> = Mutex::new(None);

/// Cheap test-friendly counter: the last error_code we saw on the
/// runtime-error callback path. Used by `test_ffi_runtime_error_callback`
/// below to assert the FFI was actually invoked.
pub static LAST_RUNTIME_ERROR_CODE: AtomicI32 = AtomicI32::new(0);

#[derive(Clone)]
struct RuntimeErrorTarget {
    diagnostics: std::sync::Arc<crate::session::SessionDiagnostics>,
    state_machine: std::sync::Arc<crate::session::SessionStateMachine>,
    epoch: crate::session::SessionEpoch,
}

#[derive(Clone)]
struct SegmentTarget {
    diagnostics: std::sync::Arc<crate::session::SessionDiagnostics>,
    epoch: crate::session::SessionEpoch,
    journal: Option<std::sync::Arc<crate::project::journal::ProjectJournal>>,
    project_root: Option<std::path::PathBuf>,
}

/// Install the global callback targets for the duration of a session.
/// Called from `MacCaptureSession::start` (or the public start impl in
/// `commands/mod.rs`) before the first segment is committed.
pub(crate) fn install_callback_targets(
    diagnostics: std::sync::Arc<crate::session::SessionDiagnostics>,
    state_machine: std::sync::Arc<crate::session::SessionStateMachine>,
    epoch: crate::session::SessionEpoch,
    journal: Option<std::sync::Arc<crate::project::journal::ProjectJournal>>,
    project_root: Option<std::path::PathBuf>,
) {
    *RUNTIME_ERROR_TARGET.lock().unwrap() = Some(RuntimeErrorTarget {
        diagnostics: diagnostics.clone(),
        state_machine: state_machine.clone(),
        epoch: epoch.clone(),
    });
    *SEGMENT_TARGET.lock().unwrap() = Some(SegmentTarget {
        diagnostics,
        epoch,
        journal,
        project_root,
    });
}

/// Clear the global callback targets. Safe to call from any thread.
pub(crate) fn clear_callback_targets() {
    *RUNTIME_ERROR_TARGET.lock().unwrap() = None;
    *SEGMENT_TARGET.lock().unwrap() = None;
}

/// The C function pointer the Swift side keeps. Marked `unsafe extern "C"`
/// because C ABIs can't carry Rust-only invariants.
unsafe extern "C" fn c_segment_callback(
    track_id: *const c_char,
    host_anchor_us: i64,
    segment_index: i32,
    timescale: i32,
    media_start_value: i64,
    file_path: *const c_char,
) {
    let track_id_str = if track_id.is_null() {
        String::new()
    } else {
        CStr::from_ptr(track_id).to_string_lossy().into_owned()
    };
    let file_path_str = if file_path.is_null() {
        String::new()
    } else {
        CStr::from_ptr(file_path).to_string_lossy().into_owned()
    };

    let target = SEGMENT_TARGET.lock().unwrap().clone();
    if let Some(target) = target {
        let _ = target.diagnostics.apply(&SessionEvent::SegmentRotated {
            track_id: track_id_str.clone(),
            segment_index: segment_index.max(0) as u32,
            host_anchor_us,
            media_timescale: timescale.max(0) as u32,
            media_start_value,
        });

        if let (Some(journal), Some(project_root)) = (target.journal.as_ref(), target.project_root.as_ref()) {
            if !file_path_str.is_empty() {
                let p = Path::new(&file_path_str);
                // The callback is the commit boundary. Validate again on the
                // Rust side before putting anything in the durable journal;
                // this prevents a malformed native callback (or a failed
                // writer that nevertheless left bytes behind) from becoming
                // committed project media.
                let (track_type, canonical_id) = match track_id_str.as_str() {
                    "screen" => (TrackType::Screen, "screen"),
                    "webcam" | "camera" => (TrackType::Webcam, "webcam"),
                    "system" | "system_audio" => (TrackType::SystemAudio, "system"),
                    "mic" => (TrackType::MicAudio, "mic"),
                    _ => return,
                };
                let canonical_root = match project_root.canonicalize() {
                    Ok(root) => root,
                    Err(_) => return,
                };
                let canonical_path = match p.canonicalize() {
                    Ok(path) if path.starts_with(&canonical_root) => path,
                    _ => return,
                };
                let rel_path = match canonical_path.strip_prefix(&canonical_root) {
                    Ok(rel) => rel.to_string_lossy().into_owned(),
                    Err(_) => return,
                };
                if !rel_path.starts_with(&format!("media/{}/", canonical_id))
                    || rel_path.ends_with(".tmp")
                    || ProjectManifest::validate_path_in_root(&canonical_root, &rel_path).is_err()
                {
                    return;
                }
                let info = match MediaValidator::validate(&canonical_path, track_type) {
                    Ok(info) if info.size_bytes > 0 => info,
                    _ => return,
                };
                let start_us = host_anchor_us.max(0) as u64;
                let end_us = start_us.saturating_add(info.duration_us.max(1));
                if journal
                    .append(crate::project::journal::JournalRecord::SegmentCommitted {
                        seq: 0,
                        track_id: canonical_id.to_string(),
                        relative_path: rel_path,
                        start_us,
                        end_us,
                        size_bytes: info.size_bytes,
                        is_keyframe_start: info.is_keyframe_start,
                        media_timescale: if info.media_timescale > 0 {
                            info.media_timescale
                        } else {
                            timescale.max(0) as u32
                        },
                        media_start_value: if info.media_start_value != 0 {
                            info.media_start_value
                        } else {
                            media_start_value
                        },
                        host_anchor_us,
                    })
                    .is_err()
                {
                    // The media remains recoverable on disk, but the failed
                    // journal append must be visible as a runtime failure and
                    // must never be reported as a committed operation.
                    let _ = target.diagnostics.apply(&SessionEvent::RuntimeError {
                        track_id: track_id_str.clone(),
                        error_code: -600,
                        message: "failed to append native segment journal record".to_string(),
                        t_us: target.epoch.current_elapsed_us(),
                        recoverable: true,
                    });
                }
            }
        }
    }
}

unsafe extern "C" fn c_runtime_error_callback(
    track_id: *const c_char,
    error_code: c_int,
    message: *const c_char,
) {
    let track_id_str = if track_id.is_null() {
        String::new()
    } else {
        CStr::from_ptr(track_id).to_string_lossy().into_owned()
    };
    let message_str = if message.is_null() {
        String::new()
    } else {
        CStr::from_ptr(message).to_string_lossy().into_owned()
    };

    // Record the raw code for tests that want to assert the FFI fired.
    LAST_RUNTIME_ERROR_CODE.store(error_code as i32, Ordering::SeqCst);

    let target = RUNTIME_ERROR_TARGET.lock().unwrap().clone();
    if let Some(target) = target {
        let event = SessionEvent::RuntimeError {
            track_id: track_id_str,
            error_code: error_code as i32,
            message: message_str,
            t_us: target.epoch.current_elapsed_us(),
            // Native runtime errors are recoverable by default; the
            // Swift side escalates non-recoverable ones via a reserved
            // error code (e.g. -1 sentinel). We don't introspect that
            // here because the contract says recoverable=true unless
            // the encoder also reported Failed in the stop result.
            recoverable: error_code >= 0,
        };
        if let Some(record) = target.diagnostics.apply(&event) {
            // If the error is non-recoverable, transition the state
            // machine to Error so subsequent stops are correctly typed.
            if !record.recoverable {
                let _ = target
                    .state_machine
                    .transition_to(crate::session::SessionState::Error);
            }
        }
    }
}

/// Public C-callable function pointer for the segment callback.
pub extern "C" fn segment_callback_pointer() -> AeroShootSegmentCallback {
    c_segment_callback
}

/// Public C-callable function pointer for the runtime-error callback.
pub extern "C" fn runtime_error_callback_pointer() -> AeroShootRuntimeErrorCallback {
    c_runtime_error_callback
}

/// Translate a Swift-side `AeroShootEncoderResult` into a Rust-friendly
/// `Result<(), (i32, String)>`. Returns `Ok(())` on `Ok`, otherwise the
/// `error_code` and the null-terminated message.
pub fn decode_encoder_result(result: AeroShootEncoderResult) -> Result<(), (i32, String)> {
    if result.status.is_ok() {
        return Ok(());
    }
    let message = unsafe {
        let cstr = CStr::from_ptr(result.error_message.as_ptr());
        cstr.to_string_lossy().into_owned()
    };
    Err((result.error_code, message))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRecordingConfig<'a> {
    pub source_id: &'a str,
    pub camera_id: Option<&'a str>,
    pub mic_id: Option<&'a str>,
    pub capture_system_audio: bool,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub source_rect: SourceRect,
    pub destination_rect: SourceRect,
    pub preserves_aspect_ratio: bool,
    pub project_path: &'a Path,
    pub session_offset_us: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NativeCaptureStats {
    pub dropped_frames: u64,
    pub audio_buffer_underflows: u64,
    pub timestamp_records_dropped: u64,
    pub gaps_total: u64,
    pub last_error: Option<String>,
}

pub struct MacCaptureSession {
    handle: Option<NonNull<c_void>>,
}

// The Swift recorder synchronizes its state and owns all callback queues.
unsafe impl Send for MacCaptureSession {}
unsafe impl Sync for MacCaptureSession {}

impl MacCaptureSession {
    pub fn start(config: NativeRecordingConfig<'_>) -> Result<Self, String> {
        let json = serde_json::to_string(&config).map_err(|error| error.to_string())?;
        let json = CString::new(json).map_err(|error| error.to_string())?;
        let mut error_pointer = std::ptr::null_mut();
        let handle = unsafe { aeroshoot_macos_start(json.as_ptr(), &mut error_pointer) };
        match NonNull::new(handle) {
            Some(handle) => {
                unsafe {
                    aeroshoot_macos_register_segment_callback(
                        handle.as_ptr(),
                        Some(segment_callback_pointer()),
                    );
                    aeroshoot_macos_register_runtime_error_callback(
                        handle.as_ptr(),
                        Some(runtime_error_callback_pointer()),
                    );
                }
                Ok(Self {
                    handle: Some(handle),
                })
            }
            None => Err(take_string(error_pointer)
                .unwrap_or_else(|_| "macOS capture failed without an error message".into())),
        }
    }

    pub fn set_paused(&self, paused: bool) {
        if let Some(handle) = self.handle {
            unsafe { aeroshoot_macos_set_paused(handle.as_ptr(), paused) }
        }
    }

    pub fn stats(&self) -> NativeCaptureStats {
        let Some(handle) = self.handle else {
            return NativeCaptureStats::default();
        };
        take_string(unsafe { aeroshoot_macos_copy_stats_json(handle.as_ptr()) })
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    pub fn stop(mut self) {
        if let Some(handle) = self.handle.take() {
            unsafe {
                aeroshoot_macos_register_segment_callback(handle.as_ptr(), None);
                aeroshoot_macos_register_runtime_error_callback(handle.as_ptr(), None);
                aeroshoot_macos_stop(handle.as_ptr());
            }
        }
    }

    /// New typed stop that surfaces a typed encoder result back to the
    /// caller. On non-OK status the caller is expected to (a) not rename
    /// any temp file, (b) not append a journal record, and (c) surface
    /// the error to the Tauri command as an `Err`.
    pub fn stop_with_result(mut self) -> Result<(), (i32, String)> {
        let handle = match self.handle.take() {
            Some(h) => h,
            None => return Ok(()),
        };
        let mut result = AeroShootEncoderResult {
            status: AeroShootEncoderStatus::Ok,
            error_code: 0,
            error_message: [0; 256],
        };
        unsafe {
            aeroshoot_macos_stop_capture(handle.as_ptr(), &mut result);
            aeroshoot_macos_register_segment_callback(handle.as_ptr(), None);
            aeroshoot_macos_register_runtime_error_callback(handle.as_ptr(), None);
        }
        decode_encoder_result(result)
    }
}

impl Drop for MacCaptureSession {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            unsafe {
                aeroshoot_macos_register_segment_callback(handle.as_ptr(), None);
                aeroshoot_macos_register_runtime_error_callback(handle.as_ptr(), None);
                aeroshoot_macos_stop(handle.as_ptr());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionDiagnostics, SessionStateMachine};
    use std::sync::Arc;

    #[test]
    fn ffi_permission_and_device_enumeration_round_trip() {
        let _ = permissions();
        assert!(devices().is_ok());
    }

    #[test]
    fn decode_encoder_result_handles_ok_and_failed() {
        let mut bytes = [0i8; 256];
        let ok = AeroShootEncoderResult {
            status: AeroShootEncoderStatus::Ok,
            error_code: 0,
            error_message: bytes,
        };
        assert!(decode_encoder_result(ok).is_ok());

        let msg = b"writer failed\0";
        for (i, b) in msg.iter().enumerate() {
            bytes[i] = *b as i8;
        }
        let failed = AeroShootEncoderResult {
            status: AeroShootEncoderStatus::Failed,
            error_code: 42,
            error_message: bytes,
        };
        let err = decode_encoder_result(failed).expect_err("failed status must error");
        assert_eq!(err.0, 42);
        assert!(err.1.contains("writer failed"));
    }

    #[test]
    fn runtime_error_callback_routes_into_diagnostics() {
        // Reset the test-friendly counter and install a fresh diagnostics.
        LAST_RUNTIME_ERROR_CODE.store(0, Ordering::SeqCst);
        let diagnostics = Arc::new(SessionDiagnostics::new());
        let state_machine = Arc::new(SessionStateMachine::new());
        install_callback_targets(
            diagnostics.clone(),
            state_machine.clone(),
            crate::session::SessionEpoch::now(),
            None,
            None,
        );

        let track = CString::new("screen").unwrap();
        let message = CString::new("encoder timeout").unwrap();
        unsafe {
            c_runtime_error_callback(
                track.as_ptr(),
                17 as c_int,
                message.as_ptr(),
            );
        }

        let record = diagnostics
            .last_runtime_error()
            .expect("runtime error must populate diagnostics");
        assert_eq!(record.track_id, "screen");
        assert_eq!(record.error_code, 17);
        assert!(record.message.contains("encoder timeout"));
        assert_eq!(LAST_RUNTIME_ERROR_CODE.load(Ordering::SeqCst), 17);
        assert_eq!(diagnostics.gaps_total(), 1);

        clear_callback_targets();
    }
}
