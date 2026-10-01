//! Pointer telemetry on Windows: a low-level mouse hook (`WH_MOUSE_LL`, no
//! permission needed) writing the shared version 2 telemetry stream through
//! `NativeMouseLogger`, with the macOS bridge's semantics: 100 ms geometry and
//! cursor polling, moves coalesced within a frame, explicit gaps.
//!
//! The hook callback only timestamps and queues: Windows silently removes a
//! low-level hook whose callback is slow, and that removal cannot be detected.
use super::clock::{now_hns, RecordingClock};
use super::cursor::{self, CursorSample};
use crate::telemetry::native::{
    NativeMouseLogger, COORDINATE_SPACE_WINDOWS, GAP_CURSOR_SHOWN_IN_VIDEO,
    GAP_EVENT_TAP_UNAVAILABLE, GAP_GEOMETRY_CHANGED, GAP_INITIAL_BUTTON_STATE_UNKNOWN,
    GAP_QUEUE_OVERFLOW, GAP_RECORDING_PAUSED, GAP_UNSUPPORTED_SOURCE_GEOMETRY,
    GEOMETRY_SAMPLING_INTERVAL_US,
};
use ::windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use ::windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use ::windows::Win32::Graphics::Gdi::{
    EnumDisplaySettingsW, GetMonitorInfoW, DEVMODEW, ENUM_CURRENT_SETTINGS, MONITORINFO,
    MONITORINFOEXW,
};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::System::Threading::GetCurrentThreadId;
use ::windows::Win32::UI::HiDpi::{
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, IsWindow, PostThreadMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, HHOOK, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Coalesce consecutive moves closer than one 60 fps frame, as on macOS.
const MOVE_SAMPLE_INTERVAL_US: u64 = 16_667;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const QUEUE_CAPACITY: usize = 4_096;
/// Mouse wheel units per detent.
const WHEEL_DELTA: f64 = 120.0;

/// One hook callback, copied out as-is.
#[derive(Clone, Copy, Debug)]
pub struct RawMouse {
    pub message: u32,
    pub x: i32,
    pub y: i32,
    pub data: u32,
    pub time_hns: u64,
}

/// Process-wide: Windows delivers low-level hook callbacks to a plain function.
static HOOK_QUEUE: Mutex<Option<SyncSender<RawMouse>>> = Mutex::new(None);
static HOOK_DROPPED: AtomicU64 = AtomicU64::new(0);

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let event = RawMouse {
            message: wparam.0 as u32,
            x: info.pt.x,
            y: info.pt.y,
            data: info.mouseData,
            time_hns: now_hns(),
        };
        // Never block input: a contended or full queue drops the event and
        // the worker records a gap.
        let sent = HOOK_QUEUE
            .try_lock()
            .ok()
            .and_then(|queue| queue.as_ref().map(|sender| sender.try_send(event).is_ok()))
            .unwrap_or(false);
        if !sent {
            HOOK_DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// What pointer coordinates are normalized against.
#[derive(Clone, Copy, Debug)]
pub enum MouseSource {
    Display(u32),
    Window(usize),
}

impl MouseSource {
    pub fn from_source_id(source_id: &str) -> Option<Self> {
        if let Some(number) = source_id.strip_prefix("display:") {
            return number.parse().ok().map(Self::Display);
        }
        source_id
            .strip_prefix("window:")
            .and_then(|handle| handle.parse().ok())
            .map(Self::Window)
    }
}

pub struct MouseConfig {
    pub source_id: String,
    pub source: MouseSource,
    pub output_width: u32,
    pub output_height: u32,
    pub cursor_mode: &'static str,
}

pub struct MouseTracker {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<(), String>>>,
    hook_thread: Option<(u32, JoinHandle<()>)>,
}

impl MouseTracker {
    /// Create the telemetry files and install the hook. `Ok(false)` inside
    /// means the hook could not be installed: the stream records why, and the
    /// caller must keep the cursor in the video.
    pub fn start(
        project: &Path,
        config: MouseConfig,
        clock: RecordingClock,
        paused: Arc<AtomicBool>,
    ) -> Result<(Self, bool), String> {
        let logger = NativeMouseLogger::create(project)?;
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        HOOK_DROPPED.store(0, Ordering::Relaxed);
        *HOOK_QUEUE.lock().unwrap_or_else(|e| e.into_inner()) = Some(sender);

        let hook_thread = install_hook();
        let hooked = hook_thread.is_ok();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = std::thread::Builder::new()
            .name("aeroshoot-mouse".into())
            .spawn(move || {
                // Monitor and window bounds in physical pixels, like hook points.
                unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
                let mut worker = Worker::new(logger, config, clock, paused);
                worker.run(&receiver, &worker_stop, hooked)
            })
            .map_err(|e| format!("Could not start mouse tracking: {e}"))?;
        Ok((
            Self {
                stop,
                worker: Some(worker),
                hook_thread: hook_thread.ok(),
            },
            hooked,
        ))
    }

    /// Remove the hook, write the final records, and close the files.
    pub fn stop(mut self) -> Result<(), String> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> Result<(), String> {
        if let Some((thread_id, thread)) = self.hook_thread.take() {
            let _ = unsafe { PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            let _ = thread.join();
        }
        *HOOK_QUEUE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.stop.store(true, Ordering::SeqCst);
        match self.worker.take() {
            Some(worker) => worker
                .join()
                .map_err(|_| "Mouse telemetry stopped unexpectedly".to_string())?,
            None => Ok(()),
        }
    }
}

impl Drop for MouseTracker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// The hook lives on its own thread with a message loop, as Windows requires.
fn install_hook() -> Result<(u32, JoinHandle<()>), String> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("aeroshoot-mouse-hook".into())
        .spawn(move || unsafe {
            let module = GetModuleHandleW(None).ok().map(|m| m.into());
            let hook: HHOOK = match SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), module, 0) {
                Ok(hook) => hook,
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(GetCurrentThreadId()));
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {}
            let _ = UnhookWindowsHookEx(hook);
        })
        .map_err(|e| e.to_string())?;
    match ready_rx.recv() {
        Ok(Ok(thread_id)) => Ok((thread_id, thread)),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => Err("The mouse hook thread exited".into()),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    rotation: f64,
}

struct Worker {
    logger: NativeMouseLogger,
    config: MouseConfig,
    clock: RecordingClock,
    paused: Arc<AtomicBool>,
    seq: u64,
    geometry: Option<Bounds>,
    geometry_id: Option<String>,
    geometry_revision: u32,
    pending_move: Option<Value>,
    pause_start: Option<u64>,
    last_cursor: Option<isize>,
    cursor_ids: HashSet<String>,
    started: bool,
    dropped_reported: u64,
}

impl Worker {
    fn new(
        logger: NativeMouseLogger,
        config: MouseConfig,
        clock: RecordingClock,
        paused: Arc<AtomicBool>,
    ) -> Self {
        Self {
            logger,
            config,
            clock,
            paused,
            seq: 0,
            geometry: None,
            geometry_id: None,
            geometry_revision: 0,
            pending_move: None,
            pause_start: None,
            last_cursor: None,
            cursor_ids: HashSet::new(),
            started: false,
            dropped_reported: 0,
        }
    }

    fn run(
        &mut self,
        events: &Receiver<RawMouse>,
        stop: &AtomicBool,
        hooked: bool,
    ) -> Result<(), String> {
        // Nothing is recorded during the countdown.
        while self.clock.started_ago_us().is_none() {
            if stop.load(Ordering::SeqCst) {
                return self.logger.finish();
            }
            while events.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(5));
        }
        self.begin(hooked)?;
        let mut next_poll = now_hns();
        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let wait_hns = next_poll.saturating_sub(now_hns());
            match events.recv_timeout(Duration::from_nanos(wait_hns * 100)) {
                Ok(event) => self.handle(event)?,
                Err(RecvTimeoutError::Timeout) => {
                    self.poll()?;
                    next_poll = now_hns() + POLL_INTERVAL.as_nanos() as u64 / 100;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        while let Ok(event) = events.try_recv() {
            self.handle(event)?;
        }
        self.end()
    }

    fn now_us(&self) -> u64 {
        self.clock.started_ago_us().unwrap_or(0)
    }

    fn begin(&mut self, hooked: bool) -> Result<(), String> {
        self.started = true;
        let now = self.now_us();
        self.refresh_geometry(now)?;
        self.gap(GAP_INITIAL_BUTTON_STATE_UNKNOWN, now, now, 0)?;
        if !hooked {
            self.gap(GAP_EVENT_TAP_UNAVAILABLE, now, now, 0)?;
            // The session keeps the cursor in the video; tell the editor.
            self.gap(GAP_CURSOR_SHOWN_IN_VIDEO, now, now, 0)?;
        } else if self.geometry_id.is_none() {
            self.gap(GAP_UNSUPPORTED_SOURCE_GEOMETRY, now, now, 0)?;
        }
        self.sample_cursor(now)?;
        self.flush()
    }

    fn poll(&mut self) -> Result<(), String> {
        let now = self.now_us();
        self.track_pause(now)?;
        self.report_dropped(now)?;
        self.refresh_geometry(now)?;
        if !self.paused.load(Ordering::SeqCst) {
            self.sample_cursor(now)?;
        }
        self.flush()
    }

    fn end(&mut self) -> Result<(), String> {
        let now = self.now_us();
        if self.started {
            self.report_dropped(now)?;
            if let Some(start) = self.pause_start.take() {
                self.gap(GAP_RECORDING_PAUSED, start, now, 0)?;
            }
            self.flush()?;
        }
        self.logger.finish()
    }

    fn track_pause(&mut self, now: u64) -> Result<(), String> {
        let paused = self.paused.load(Ordering::SeqCst);
        match (paused, self.pause_start) {
            (true, None) => {
                self.flush_move()?;
                self.pause_start = Some(now);
            }
            (false, Some(start)) => {
                self.pause_start = None;
                self.gap(GAP_RECORDING_PAUSED, start, now, 0)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn report_dropped(&mut self, now: u64) -> Result<(), String> {
        let dropped = HOOK_DROPPED.load(Ordering::Relaxed);
        if dropped > self.dropped_reported {
            let count = dropped - self.dropped_reported;
            self.dropped_reported = dropped;
            // Held-button state is unknown after lost events.
            let start = now.saturating_sub(GEOMETRY_SAMPLING_INTERVAL_US);
            self.gap(GAP_QUEUE_OVERFLOW, start, now, count)?;
        }
        Ok(())
    }

    fn handle(&mut self, event: RawMouse) -> Result<(), String> {
        if self.paused.load(Ordering::SeqCst) {
            return self.track_pause(self.now_us());
        }
        let Some(time) = self.clock.session_us(event.time_hns) else {
            return Ok(());
        };
        let (Some(bounds), Some(geometry_id)) = (self.geometry, self.geometry_id.clone()) else {
            return Ok(());
        };
        let Some(payload) = payload_for(&event) else {
            return Ok(());
        };
        let x = (f64::from(event.x) - bounds.x) / bounds.width;
        let y = (f64::from(event.y) - bounds.y) / bounds.height;
        let is_move = payload["kind"] == "move";
        let record = json!({
            "record": "event", "version": 2, "t_us": time,
            "geometry_id": geometry_id, "norm_x": x, "norm_y": y,
            "inside_source": (0.0..1.0).contains(&x) && (0.0..1.0).contains(&y),
            "payload": payload,
        });
        if is_move {
            // Keep only the latest of a burst of moves within one frame.
            if let Some(previous) = &self.pending_move {
                let previous_time = previous["t_us"].as_u64().unwrap_or(0);
                let same_geometry = previous["geometry_id"] == record["geometry_id"];
                if !(same_geometry
                    && time >= previous_time
                    && time - previous_time < MOVE_SAMPLE_INTERVAL_US)
                {
                    self.flush_move()?;
                }
            }
            self.pending_move = Some(record);
            Ok(())
        } else {
            self.flush_move()?;
            self.write_event(record)
        }
    }

    fn refresh_geometry(&mut self, now: u64) -> Result<(), String> {
        let bounds = source_bounds(self.config.source);
        if bounds == self.geometry {
            return Ok(());
        }
        self.flush_move()?;
        // A change is a discontinuity at polling resolution.
        self.gap(
            GAP_GEOMETRY_CHANGED,
            now.saturating_sub(GEOMETRY_SAMPLING_INTERVAL_US),
            now,
            0,
        )?;
        self.geometry = bounds;
        self.geometry_id = None;
        let Some(bounds) = bounds else {
            return Ok(());
        };
        self.geometry_revision += 1;
        let id = format!("mouse-g{}", self.geometry_revision);
        let record = json!({
            "record": "geometry", "version": 2, "geometry_id": id, "t_us": now,
            "coordinate_space": COORDINATE_SPACE_WINDOWS, "source_id": self.config.source_id,
            "bounds": { "x": bounds.x, "y": bounds.y, "width": bounds.width, "height": bounds.height },
            "output_width": self.config.output_width, "output_height": self.config.output_height,
            "sampling_interval_us": GEOMETRY_SAMPLING_INTERVAL_US,
            "cursor_mode": self.config.cursor_mode,
            // Bounds are already physical pixels.
            "physical_width": bounds.width, "physical_height": bounds.height,
            "rotation_degrees": bounds.rotation,
            "logical_to_physical_scale_x": 1.0, "logical_to_physical_scale_y": 1.0,
        });
        self.logger.append(&record.to_string())?;
        self.geometry_id = Some(id);
        Ok(())
    }

    fn sample_cursor(&mut self, now: u64) -> Result<(), String> {
        let Some(geometry_id) = self.geometry_id.clone() else {
            return Ok(());
        };
        let handle = cursor::current();
        let key = handle.map_or(0, |h| h.0 as isize);
        if Some(key) == self.last_cursor {
            return Ok(());
        }
        let sample: Option<CursorSample> = match handle {
            Some(handle) => cursor::describe(handle),
            None => Some(cursor::hidden()),
        };
        self.last_cursor = Some(key);
        let Some(sample) = sample else {
            return Ok(());
        };
        if let Some(png) = &sample.png {
            if self.cursor_ids.insert(sample.id.clone()) {
                let asset = json!({
                    "record": "cursor_asset", "version": 2, "cursor_id": sample.id,
                    "png_base64": base64(png), "width": sample.width, "height": sample.height,
                });
                self.logger.append(&asset.to_string())?;
            }
        }
        let mut payload = json!({
            "kind": "cursor_changed", "cursor_id": sample.id,
            "hotspot_x": sample.hotspot_x, "hotspot_y": sample.hotspot_y,
            "width": sample.width, "height": sample.height,
        });
        if let Some(name) = sample.name {
            payload["name"] = json!(name);
        }
        self.flush_move()?;
        self.write_event(json!({
            "record": "event", "version": 2, "t_us": now,
            "geometry_id": geometry_id, "payload": payload,
        }))
    }

    fn gap(&mut self, reason: &str, start: u64, end: u64, dropped: u64) -> Result<(), String> {
        self.flush_move()?;
        self.write_event(json!({
            "record": "event", "version": 2, "t_us": end,
            "payload": { "kind": "gap", "reason": reason, "start_us": start,
                "end_us": end, "dropped_events": dropped },
        }))
    }

    fn flush_move(&mut self) -> Result<(), String> {
        match self.pending_move.take() {
            Some(record) => self.write_event(record),
            None => Ok(()),
        }
    }

    fn write_event(&mut self, mut record: Value) -> Result<(), String> {
        record["seq"] = json!(self.seq);
        self.logger.append(&record.to_string())?;
        self.seq += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        self.flush_move()?;
        self.logger.append(r#"{"record":"flush"}"#)
    }
}

/// The telemetry payload for a hook message, if it is one we record.
fn payload_for(event: &RawMouse) -> Option<Value> {
    // HIWORD of mouseData: wheel delta (signed) or X button number.
    let high = (event.data >> 16) as u16;
    Some(match event.message {
        WM_MOUSEMOVE => json!({ "kind": "move" }),
        WM_LBUTTONDOWN => json!({ "kind": "button_down", "button": 0 }),
        WM_LBUTTONUP => json!({ "kind": "button_up", "button": 0 }),
        WM_RBUTTONDOWN => json!({ "kind": "button_down", "button": 1 }),
        WM_RBUTTONUP => json!({ "kind": "button_up", "button": 1 }),
        WM_MBUTTONDOWN => json!({ "kind": "button_down", "button": 2 }),
        WM_MBUTTONUP => json!({ "kind": "button_up", "button": 2 }),
        // XBUTTON1 / XBUTTON2 follow the macOS numbering: 3 and 4.
        WM_XBUTTONDOWN => json!({ "kind": "button_down", "button": 2 + u32::from(high) }),
        WM_XBUTTONUP => json!({ "kind": "button_up", "button": 2 + u32::from(high) }),
        // Wheel detents as lines; positive is away from the user (up) or right.
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = f64::from(high as i16) / WHEEL_DELTA;
            let (delta_x, delta_y) = if event.message == WM_MOUSEWHEEL {
                (0.0, delta)
            } else {
                (delta, 0.0)
            };
            json!({ "kind": "scroll", "delta_x": delta_x, "delta_y": delta_y,
                "units": "lines", "precise": false, "phase": 0, "momentum_phase": 0 })
        }
        _ => return None,
    })
}

/// Physical-pixel bounds of the source, or `None` when it is gone.
fn source_bounds(source: MouseSource) -> Option<Bounds> {
    match source {
        MouseSource::Display(number) => {
            let monitor = crate::capture::windows::sources::monitor_for_display(number)?;
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            unsafe {
                GetMonitorInfoW(
                    monitor,
                    &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
                )
            }
            .as_bool()
            .then_some(())?;
            let rect = info.monitorInfo.rcMonitor;
            let mut mode = DEVMODEW {
                dmSize: std::mem::size_of::<DEVMODEW>() as u16,
                ..Default::default()
            };
            let orientation = unsafe {
                EnumDisplaySettingsW(
                    ::windows::core::PCWSTR(info.szDevice.as_ptr()),
                    ENUM_CURRENT_SETTINGS,
                    &mut mode,
                )
            }
            .as_bool()
            .then(|| unsafe { mode.Anonymous1.Anonymous2.dmDisplayOrientation.0 })
            .unwrap_or(0);
            bounds_from(rect, f64::from(orientation) * 90.0)
        }
        MouseSource::Window(handle) => {
            let hwnd = HWND(handle as *mut _);
            if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                return None;
            }
            let mut rect = RECT::default();
            unsafe {
                DwmGetWindowAttribute(
                    hwnd,
                    DWMWA_EXTENDED_FRAME_BOUNDS,
                    &mut rect as *mut RECT as *mut _,
                    std::mem::size_of::<RECT>() as u32,
                )
            }
            .ok()?;
            bounds_from(rect, 0.0)
        }
    }
}

fn bounds_from(rect: RECT, rotation: f64) -> Option<Bounds> {
    let width = f64::from(rect.right - rect.left);
    let height = f64::from(rect.bottom - rect.top);
    (width > 0.0 && height > 0.0).then_some(Bounds {
        x: f64::from(rect.left),
        y: f64::from(rect.top),
        width,
        height,
        rotation,
    })
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn hook_messages_map_to_macos_payloads() {
        let event = |message, data| RawMouse {
            message,
            x: 0,
            y: 0,
            data,
            time_hns: 0,
        };
        assert_eq!(
            payload_for(&event(WM_MOUSEMOVE, 0)).unwrap()["kind"],
            "move"
        );
        assert_eq!(payload_for(&event(WM_RBUTTONDOWN, 0)).unwrap()["button"], 1);
        assert_eq!(
            payload_for(&event(WM_XBUTTONUP, 2 << 16)).unwrap()["button"],
            4
        );
        let wheel = payload_for(&event(WM_MOUSEWHEEL, (-240i16 as u16 as u32) << 16)).unwrap();
        assert_eq!(wheel["delta_y"], -2.0);
        assert_eq!(wheel["units"], "lines");
        assert!(payload_for(&event(0x0200 + 0x50, 0)).is_none());
    }

    #[test]
    fn source_ids_parse() {
        assert!(matches!(
            MouseSource::from_source_id("display:2"),
            Some(MouseSource::Display(2))
        ));
        assert!(matches!(
            MouseSource::from_source_id("window:123"),
            Some(MouseSource::Window(123))
        ));
        assert!(MouseSource::from_source_id("application:x").is_none());
    }
}
