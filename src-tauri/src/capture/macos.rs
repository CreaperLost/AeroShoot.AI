use super::backend::NativeCaptureSession;
use super::segments;
use super::{AudioDevice, CameraDevice, CaptureSource, PermissionState, PermissionStatus};
pub use super::{NativeCaptureStats, NativeRecordingConfig};
use serde::Deserialize;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Condvar, OnceLock};
use std::sync::{Mutex, PoisonError};
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
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AeroShootPermissionState {
    Unknown = 0,
    NotDetermined = 1,
    Authorized = 2,
    Denied = 3,
    Restricted = 4,
}

impl AeroShootPermissionState {
    fn from_c(value: i32) -> Self {
        match value {
            1 => Self::NotDetermined,
            2 => Self::Authorized,
            3 => Self::Denied,
            4 => Self::Restricted,
            _ => Self::Unknown,
        }
    }
}

/// Packed C layout: three `int32_t` fields. Swift writes these with
/// `storeBytes` rather than assigning a Swift struct, so keep this as raw
/// integers and decode — a `repr(C)` enum in the struct is UB on a bad value.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AeroShootPermissionBundle {
    pub screen_recording: i32,
    pub camera: i32,
    pub microphone: i32,
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
) -> c_int;

/// C function pointer type for the runtime-error callback.
pub type AeroShootRuntimeErrorCallback =
    unsafe extern "C" fn(track_id: *const c_char, error_code: c_int, message: *const c_char);

/// C function pointer type for the permission completion callback.
pub type AeroShootPermissionCompletion =
    unsafe extern "C" fn(screen_recording: i32, camera: i32, microphone: i32);

extern "C" {
    fn aeroshoot_macos_mouse_permission(request: bool) -> bool;
    fn aeroshoot_macos_register_mouse_sink(
        sink: Option<unsafe extern "C" fn(*const c_char) -> c_int>,
    );
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
    fn aeroshoot_macos_pause_and_finalize(
        handle: *mut c_void,
        out_result: *mut AeroShootEncoderResult,
    );
    fn aeroshoot_macos_copy_stats_json(handle: *mut c_void) -> *mut c_char;
    /// Old stop symbol — kept for source compatibility; the new
    /// `aeroshoot_macos_stop_capture` returns a typed result.
    fn aeroshoot_macos_stop(handle: *mut c_void);
    /// New stop symbol that surfaces encoder status to Rust.
    fn aeroshoot_macos_stop_capture(handle: *mut c_void, out_result: *mut AeroShootEncoderResult);
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
        dup_cstr(r#"{"cameras":[],"mics":[]}"#)
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_check_permissions(out: *mut AeroShootPermissionBundle) {
        if let Some(out) = unsafe { out.as_mut() } {
            *out = AeroShootPermissionBundle {
                screen_recording: AeroShootPermissionState::Authorized as i32,
                camera: AeroShootPermissionState::Authorized as i32,
                microphone: AeroShootPermissionState::Authorized as i32,
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
    pub extern "C" fn aeroshoot_macos_pause_and_finalize(
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
    pub extern "C" fn aeroshoot_macos_copy_stats_json(_handle: *mut c_void) -> *mut c_char {
        dup_cstr(r#"{"droppedFrames":0,"audioBufferUnderflows":0,"lastError":null}"#)
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_macos_stop(_handle: *mut c_void) {}
    #[no_mangle]
    pub extern "C" fn aeroshoot_macos_mouse_permission(_request: bool) -> bool {
        false
    }
    #[no_mangle]
    pub extern "C" fn aeroshoot_macos_register_mouse_sink(
        _sink: Option<unsafe extern "C" fn(*const c_char) -> c_int>,
    ) {
    }

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
        screen_recording: permission_state(AeroShootPermissionState::from_c(
            bundle.screen_recording,
        )),
        camera: permission_state(AeroShootPermissionState::from_c(bundle.camera)),
        microphone: permission_state(AeroShootPermissionState::from_c(bundle.microphone)),
    }
}

pub fn mouse_permission(request: bool) -> bool {
    unsafe { aeroshoot_macos_mouse_permission(request) }
}

pub fn permissions() -> PermissionStatus {
    let mut bundle = AeroShootPermissionBundle {
        screen_recording: 0,
        camera: 0,
        microphone: 0,
    };
    unsafe { aeroshoot_check_permissions(&mut bundle) };
    permission_status(bundle)
}

static PERMISSION_WAIT: OnceLock<(Mutex<Option<AeroShootPermissionBundle>>, Condvar)> =
    OnceLock::new();

unsafe extern "C" fn permission_completion(screen: i32, camera: i32, microphone: i32) {
    let (slot, ready) = PERMISSION_WAIT.get_or_init(|| (Mutex::new(None), Condvar::new()));
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(AeroShootPermissionBundle {
        screen_recording: screen,
        camera,
        microphone,
    });
    ready.notify_all();
}

pub fn request_permissions(screen: bool, camera: bool, microphone: bool) -> PermissionStatus {
    let (slot, ready) = PERMISSION_WAIT.get_or_init(|| (Mutex::new(None), Condvar::new()));
    let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
    *guard = None;
    unsafe {
        aeroshoot_request_permissions(screen, camera, microphone, Some(permission_completion))
    };
    let (mut guard, _) = ready
        .wait_timeout_while(guard, Duration::from_secs(125), |result| result.is_none())
        .unwrap();
    guard
        .take()
        .map(permission_status)
        .unwrap_or_else(permissions)
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

/// Reports a panic caught at the Swift boundary. The callback still returns an
/// error code to Swift; this keeps the panic message for diagnosis.
fn log_ffi_panic(callback: &str, payload: &(dyn std::any::Any + Send)) {
    let message = payload
        .downcast_ref::<&str>()
        .map(|message| message.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".into());
    eprintln!("[AeroShoot] panic in {callback}: {message}");
}

static MOUSE_LOGGER: Mutex<Option<crate::telemetry::native::NativeMouseLogger>> = Mutex::new(None);

unsafe extern "C" fn mouse_sink(json: *const c_char) -> c_int {
    std::panic::catch_unwind(|| {
        if json.is_null() { return -700; }
        let Ok(json) = CStr::from_ptr(json).to_str() else { return -700; };
        let mut logger = MOUSE_LOGGER.lock().unwrap_or_else(PoisonError::into_inner);
        match logger.as_mut().map(|logger| logger.append(json)) {
            Some(Ok(())) => {
                // Only gap records carry a reason; avoid re-parsing every move and click.
                let gap = json.contains("\"reason\"")
                    .then(|| serde_json::from_str::<serde_json::Value>(json).ok())
                    .flatten();
                if let Some(value) = gap {
                    if let Some(reason) = value.pointer("/payload/reason").and_then(|v| v.as_str()) {
                        // Missing Input Monitoring at start is shown by the recorder's
                        // mouse-tracking control before recording, so only mid-session
                        // failures are surfaced as runtime errors. The gap stays in the log.
                        if matches!(reason, "input_monitoring_revoked" | "event_tap_unavailable" | "unsupported_source_geometry") {
                            segments::report_recoverable_error(
                                "telemetry", 700,
                                format!("Mouse telemetry unavailable ({reason}); recording continues with the baked cursor."),
                            );
                        }
                    }
                }
                0
            },
            _ => -700,
        }
    })
    .unwrap_or_else(|panic| {
        log_ffi_panic("mouse telemetry sink", panic.as_ref());
        -700
    })
}

/// Cheap test-friendly counter: the last error_code we saw on the
/// runtime-error callback path. Used by `test_ffi_runtime_error_callback`
/// below to assert the FFI was actually invoked.
pub static LAST_RUNTIME_ERROR_CODE: AtomicI32 = AtomicI32::new(0);

pub(crate) use segments::{
    install_callback_targets, set_callback_epoch, take_native_segment_writers,
};

/// Clear the global callback targets. Safe to call from any thread.
pub(crate) fn clear_callback_targets() {
    segments::clear_callback_targets();
    *MOUSE_LOGGER.lock().unwrap_or_else(PoisonError::into_inner) = None;
    unsafe {
        aeroshoot_macos_register_mouse_sink(None);
    }
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
) -> c_int {
    // Swift owns AVAssetWriter on capture/rotation queues and calls this
    // synchronously after finishWriting. Rust reuses a per-track writer on
    // SegmentTarget so a journal failure after publish stays retryable.
    // Errors return a non-zero code so Swift latches terminalFailure instead
    // of treating publication as OK. Contain panics at the foreign ABI
    // boundary and report failure to Swift.
    std::panic::catch_unwind(|| {
        if track_id.is_null() || file_path.is_null() || segment_index < 0 || timescale <= 0 {
            return -600;
        }
        let id = CStr::from_ptr(track_id).to_string_lossy();
        let path = CStr::from_ptr(file_path).to_string_lossy();
        match segments::publish_segment(
            &id,
            segment_index as u32,
            host_anchor_us,
            timescale as u32,
            media_start_value,
            Path::new(path.as_ref()),
        ) {
            Ok(()) => 0,
            Err(()) => -600,
        }
    })
    .unwrap_or_else(|panic| {
        log_ffi_panic("segment callback", panic.as_ref());
        -600
    })
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
    LAST_RUNTIME_ERROR_CODE.store(error_code, Ordering::SeqCst);

    // Native runtime errors are recoverable by default; the Swift side
    // escalates non-recoverable ones with a negative code, which moves the
    // state machine to Error so subsequent stops are correctly typed.
    segments::report_runtime_error(track_id_str, error_code, message_str);
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

pub struct MacCaptureSession {
    handle: Option<NonNull<c_void>>,
}

// SAFETY: `MacCaptureSession` only holds an opaque handle to the Swift
// recorder. Every call through it enters Swift, which serializes access to the
// recorder on its own queues and locks, and the handle is released exactly once
// (taken in stop/Drop). Moving or sharing it between threads therefore cannot
// create unsynchronized access to Swift state.
unsafe impl Send for MacCaptureSession {}
unsafe impl Sync for MacCaptureSession {}

impl MacCaptureSession {
    pub fn start(config: NativeRecordingConfig<'_>) -> Result<Self, String> {
        if config.capture_screen && config.capture_mouse {
            *MOUSE_LOGGER.lock().unwrap_or_else(PoisonError::into_inner) = Some(
                crate::telemetry::native::NativeMouseLogger::create(config.project_path)?,
            );
            unsafe {
                aeroshoot_macos_register_mouse_sink(Some(mouse_sink));
            }
        } else {
            *MOUSE_LOGGER.lock().unwrap_or_else(PoisonError::into_inner) = None;
            unsafe {
                aeroshoot_macos_register_mouse_sink(None);
            }
        }
        let json = serde_json::to_string(&config).map_err(|error| error.to_string())?;
        let json = CString::new(json).map_err(|error| error.to_string())?;
        let mut error_pointer = std::ptr::null_mut();
        // Register before startup: capture can finish its first segment before
        // the asynchronous native start operation returns its handle.
        unsafe {
            aeroshoot_macos_register_segment_callback(
                std::ptr::null_mut(),
                Some(segment_callback_pointer()),
            );
            aeroshoot_macos_register_runtime_error_callback(
                std::ptr::null_mut(),
                Some(runtime_error_callback_pointer()),
            );
        }

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

    /// Stop sample admission, drain capture queues, and finalize the current
    /// native AVAssetWriter containers (submit to Rust) before the caller
    /// acknowledges Pause. Resume is `set_paused(false)` and opens a new writer
    /// on the next sample.
    pub fn pause_and_finalize(&self) -> Result<(), (i32, String)> {
        let Some(handle) = self.handle else {
            return Ok(());
        };
        let mut result = AeroShootEncoderResult {
            status: AeroShootEncoderStatus::Ok,
            error_code: 0,
            error_message: [0; 256],
        };
        unsafe {
            aeroshoot_macos_pause_and_finalize(handle.as_ptr(), &mut result);
        }
        decode_encoder_result(result)
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
        let native_result = decode_encoder_result(result);
        let telemetry_result = MOUSE_LOGGER
            .lock()
            .unwrap()
            .as_mut()
            .map(|logger| logger.finish())
            .unwrap_or(Ok(()))
            .map_err(|message| (-700, message));
        native_result.and(telemetry_result)
    }
}

impl NativeCaptureSession for MacCaptureSession {
    fn start(config: NativeRecordingConfig<'_>) -> Result<Self, String> {
        MacCaptureSession::start(config)
    }
    fn set_paused(&self, paused: bool) {
        MacCaptureSession::set_paused(self, paused)
    }
    fn pause_and_finalize(&self) -> Result<(), (i32, String)> {
        MacCaptureSession::pause_and_finalize(self)
    }
    fn stats(&self) -> NativeCaptureStats {
        MacCaptureSession::stats(self)
    }
    fn stop_with_result(self) -> Result<(), (i32, String)> {
        MacCaptureSession::stop_with_result(self)
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
    use crate::capture::SourceRect;
    use crate::session::{SessionDiagnostics, SessionStateMachine};
    use std::fs;
    use std::sync::Arc;

    #[test]
    fn ffi_permission_and_device_enumeration_round_trip() {
        // Do not call permissions() against the real ScreenCaptureKit bridge in
        // unit tests — the probe waits on WindowServer and can show a TCC prompt.
        assert!(devices().is_ok());
    }

    #[test]
    fn permission_bundle_is_three_packed_i32s() {
        assert_eq!(std::mem::size_of::<AeroShootPermissionBundle>(), 12);
        assert_eq!(std::mem::align_of::<AeroShootPermissionBundle>(), 4);
        assert_eq!(
            AeroShootPermissionState::from_c(2),
            AeroShootPermissionState::Authorized
        );
        assert_eq!(
            AeroShootPermissionState::from_c(3),
            AeroShootPermissionState::Denied
        );
        assert_eq!(
            AeroShootPermissionState::from_c(1),
            AeroShootPermissionState::NotDetermined
        );
        assert_eq!(
            AeroShootPermissionState::from_c(99),
            AeroShootPermissionState::Unknown
        );
        let status = permission_status(AeroShootPermissionBundle {
            screen_recording: 2,
            camera: 2,
            microphone: 3,
        });
        assert_eq!(status.screen_recording, PermissionState::Authorized);
        assert_eq!(status.camera, PermissionState::Authorized);
        assert_eq!(status.microphone, PermissionState::Denied);
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
    fn native_config_serializes_mic_gain_in_camel_case() {
        let rect = SourceRect::from_dimensions(1920, 1080);
        let project_path = Path::new("/tmp/example.aero");
        let cfg = NativeRecordingConfig {
            source_id: "src-1",
            capture_screen: true,
            camera_id: Some("cam-1"),
            mic_id: Some("mic-1"),
            capture_system_audio: true,
            fps: 30,
            width: 1920,
            height: 1080,
            source_rect: rect,
            destination_rect: rect,
            preserves_aspect_ratio: true,
            project_path,
            session_offset_us: 1_234,
            mic_gain_db: Some(6.0),
            video_bitrate_bps: None,
            capture_mouse: true,
            hide_cursor: false,
            start_delay_ms: 3_000,
            camera_width: None,
            camera_height: None,
            camera_fps: None,
            camera_bitrate_bps: None,
        };
        let json = serde_json::to_string(&cfg).expect("serialize config");
        // camelCase field that the Swift side decodes.
        assert!(json.contains("\"micGainDb\":6.0"), "json was: {json}");
        assert!(json.contains("\"startDelayMs\":3000"), "json was: {json}");
        // The Swift decoder treats `None` as "field omitted", so when the
        // Rust caller doesn't pass a gain we should omit it entirely.
        let cfg_none = NativeRecordingConfig {
            mic_gain_db: None,
            video_bitrate_bps: None,
            capture_mouse: true,
            hide_cursor: false,
            ..cfg
        };
        let json_none = serde_json::to_string(&cfg_none).expect("serialize config");
        assert!(!json_none.contains("micGainDb"), "json was: {json_none}");
    }

    /// The callback targets are process-wide statics. Tests that install them
    /// must not run concurrently, or one test's `clear_callback_targets` erases
    /// the other's diagnostics target mid-assertion.
    static CALLBACK_TARGETS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn runtime_error_callback_routes_into_diagnostics() {
        let _targets = CALLBACK_TARGETS_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
            c_runtime_error_callback(track.as_ptr(), 17 as c_int, message.as_ptr());
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

    #[test]
    fn native_callback_keeps_pending_publication_after_journal_failure() {
        let _targets = CALLBACK_TARGETS_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let journal = crate::project::ProjectJournal::open_or_create(dir.path()).unwrap();
        let journal = Arc::new(journal);
        fs::create_dir_all(dir.path().join("media/mic")).unwrap();
        let temp = dir.path().join("media/mic/000001.wav.tmp");
        let data = crate::fixtures::generate_valid_wav_segment(100_000, 48_000, 1);
        fs::write(&temp, &data).unwrap();
        journal.inject_fail_next_appends(1);

        let diagnostics = Arc::new(SessionDiagnostics::new());
        install_callback_targets(
            diagnostics.clone(),
            Arc::new(SessionStateMachine::new()),
            crate::session::SessionEpoch::now(),
            Some(journal.clone()),
            Some(dir.path().to_path_buf()),
        );

        let track = CString::new("mic").unwrap();
        let path = CString::new(temp.to_string_lossy().as_ref()).unwrap();
        let status = unsafe { c_segment_callback(track.as_ptr(), 0, 0, 48_000, 0, path.as_ptr()) };
        assert_eq!(status, -600);
        assert!(dir.path().join("media/mic/000001.wav").exists());
        assert!(journal.read_all().unwrap().is_empty());

        let mut writers = take_native_segment_writers();
        assert_eq!(writers.len(), 1);
        assert!(
            writers[0].has_pending_publication(),
            "throwaway callback writers must not drop pending publication"
        );

        let committed = writers[0]
            .finalize(100_000, journal.as_ref())
            .unwrap()
            .unwrap();
        assert_eq!(committed.relative_path, "media/mic/000001.wav");
        assert_eq!(journal.read_all().unwrap().len(), 1);
        assert!(!writers[0].has_pending_publication());
        clear_callback_targets();
    }
}
