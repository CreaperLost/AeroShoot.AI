//! Keeps AeroShoot's own windows out of its screen captures, like the macOS
//! bridge's capture filter that excludes the app.
//!
//! Windows has no per-capture filter for a display capture, so this uses
//! display affinity (`WDA_EXCLUDEFROMCAPTURE`), which hides the windows from
//! every capture tool. It is therefore only on while AeroShoot records a
//! display, and removed afterwards; the live preview does not use it, so other
//! apps can still capture AeroShoot while it is being set up.
use ::windows::core::BOOL;
use ::windows::Win32::Foundation::{HWND, LPARAM};
use ::windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    WDA_NONE,
};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

/// New windows (dialogs) are picked up this often while excluded.
const REFRESH: Duration = Duration::from_millis(250);

struct Exclusion {
    holders: usize,
    stop: Arc<AtomicBool>,
    refresher: Option<JoinHandle<HashSet<isize>>>,
}

static EXCLUSION: Mutex<Option<Exclusion>> = Mutex::new(None);

/// Excludes AeroShoot's windows from capture until every guard is dropped.
pub struct ExclusionGuard(());

pub fn exclude_own_windows() -> ExclusionGuard {
    let mut guard = EXCLUSION.lock().unwrap_or_else(PoisonError::into_inner);
    match guard.as_mut() {
        Some(exclusion) => exclusion.holders += 1,
        None => {
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            // Apply once before returning so the first captured frame is clean.
            let mut excluded = HashSet::new();
            apply(&mut excluded);
            let refresher = std::thread::Builder::new()
                .name("aeroshoot-capture-exclusion".into())
                .spawn(move || {
                    while !worker_stop.load(Ordering::SeqCst) {
                        std::thread::sleep(REFRESH);
                        apply(&mut excluded);
                    }
                    excluded
                })
                .ok();
            *guard = Some(Exclusion {
                holders: 1,
                stop,
                refresher,
            });
        }
    }
    ExclusionGuard(())
}

impl Drop for ExclusionGuard {
    fn drop(&mut self) {
        let finished = {
            let mut guard = EXCLUSION.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(exclusion) = guard.as_mut() else {
                return;
            };
            exclusion.holders -= 1;
            if exclusion.holders > 0 {
                return;
            }
            guard.take()
        };
        if let Some(exclusion) = finished {
            exclusion.stop.store(true, Ordering::SeqCst);
            if let Some(excluded) = exclusion.refresher.and_then(|r| r.join().ok()) {
                for hwnd in excluded {
                    let _ = unsafe { SetWindowDisplayAffinity(HWND(hwnd as *mut _), WDA_NONE) };
                }
            }
        }
    }
}

/// Exclude every top-level window of this process not yet excluded.
fn apply(excluded: &mut HashSet<isize>) {
    for hwnd in own_windows() {
        let key = hwnd.0 as isize;
        if !excluded.contains(&key)
            && unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }.is_ok()
        {
            excluded.insert(key);
        }
    }
}

fn own_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        let windows = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == std::process::id() {
            windows.push(hwnd);
        }
        true.into()
    }
    let mut windows = Vec::new();
    let _ = unsafe {
        EnumWindows(
            Some(collect),
            LPARAM(&mut windows as *mut Vec<HWND> as isize),
        )
    };
    windows
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::windows::core::w;
    use ::windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowDisplayAffinity, WINDOW_EX_STYLE,
        WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    fn affinity(hwnd: HWND) -> u32 {
        let mut value = 0;
        unsafe { GetWindowDisplayAffinity(hwnd, &mut value) }.unwrap();
        value
    }

    #[test]
    #[ignore = "opens a window"]
    fn excludes_own_windows_from_another_thread_and_restores_them() {
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("AeroShoot exclusion test"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                200,
                200,
                400,
                300,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        assert_eq!(affinity(hwnd), WDA_NONE.0);
        // Acquire on a thread that does not own the window, as the capture
        // threads do.
        let guard = std::thread::spawn(exclude_own_windows).join().unwrap();
        assert_eq!(affinity(hwnd), WDA_EXCLUDEFROMCAPTURE.0);
        let second = exclude_own_windows();
        drop(guard);
        assert_eq!(affinity(hwnd), WDA_EXCLUDEFROMCAPTURE.0, "still held");
        drop(second);
        assert_eq!(affinity(hwnd), WDA_NONE.0, "restored after the last holder");
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    #[ignore = "opens a window and records the display briefly"]
    fn only_a_recording_hides_the_app_not_the_preview() {
        use crate::commands::{
            list_capture_sources_impl, start_recording_impl, stop_recording_impl, AppState,
            StartRecordingOptions,
        };
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("AeroShoot exclusion scope test"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                200,
                200,
                400,
                300,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        let display = list_capture_sources_impl()
            .into_iter()
            .find(|s| s.id.starts_with("display:"))
            .unwrap()
            .id;

        crate::capture::windows::preview::start(&display, true, false, None, None, 0.0).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            affinity(hwnd),
            WDA_NONE.0,
            "the preview leaves the app capturable"
        );

        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        let options = StartRecordingOptions {
            source_id: display,
            capture_screen: true,
            camera_id: None,
            mic_id: None,
            capture_system_audio: false,
            fps: 30,
            resolution: "720p".into(),
            layout: None,
            project_name: Some("Exclusion Scope".into()),
            project_dir: None,
            mic_gain_db: None,
            video_bitrate_bps: None,
            capture_mouse: false,
            start_delay_ms: 0,
            camera: Default::default(),
        };
        start_recording_impl(&state, options).unwrap();
        assert_eq!(
            affinity(hwnd),
            WDA_EXCLUDEFROMCAPTURE.0,
            "a recording hides the app"
        );
        std::thread::sleep(Duration::from_secs(1));
        stop_recording_impl(&state).unwrap();
        assert_eq!(
            affinity(hwnd),
            WDA_NONE.0,
            "capturable again after recording"
        );
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}
