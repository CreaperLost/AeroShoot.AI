//! Displays and top-level windows, with IDs shaped like the macOS bridge's:
//! `display:<n>` for `\\.\DISPLAY<n>` and `window:<HWND>`.
use super::from_wide;
use crate::capture::{CaptureSource, CaptureSourceType};
use ::windows::core::{BOOL, PCWSTR, PWSTR};
use ::windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use ::windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use ::windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, DEVMODEW, ENUM_CURRENT_SETTINGS,
    HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use ::windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
};
use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;

const MONITORINFOF_PRIMARY: u32 = 1;

/// Desktop shell surfaces that are visible top-level windows but not
/// something a user means to record.
const SHELL_WINDOW_CLASSES: &[&str] = &["Progman", "WorkerW", "Shell_TrayWnd"];

pub fn capture_sources() -> Result<Vec<CaptureSource>, String> {
    let mut sources = displays()?;
    sources.extend(windows());
    Ok(sources)
}

fn displays() -> Result<Vec<CaptureSource>, String> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let monitors = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        monitors.push(monitor);
        true.into()
    }

    let mut monitors: Vec<HMONITOR> = Vec::new();
    let listed = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut monitors as *mut Vec<HMONITOR> as isize),
        )
    };
    if !listed.as_bool() {
        return Err("Windows could not list the connected displays".into());
    }

    let mut displays: Vec<(u32, bool, u32, u32)> = Vec::new();
    for (index, monitor) in monitors.into_iter().enumerate() {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        let found = unsafe {
            GetMonitorInfoW(
                monitor,
                &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
            )
        };
        if !found.as_bool() {
            continue;
        }
        // The current display mode is in physical pixels regardless of this
        // process's DPI awareness; the monitor rect is not.
        let mut mode = DEVMODEW {
            dmSize: size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        let has_mode = unsafe {
            EnumDisplaySettingsW(
                PCWSTR(info.szDevice.as_ptr()),
                ENUM_CURRENT_SETTINGS,
                &mut mode,
            )
        };
        let (width, height) = if has_mode.as_bool() {
            (mode.dmPelsWidth, mode.dmPelsHeight)
        } else {
            let rect = info.monitorInfo.rcMonitor;
            (
                (rect.right - rect.left).max(0) as u32,
                (rect.bottom - rect.top).max(0) as u32,
            )
        };
        let number = display_number(&from_wide(&info.szDevice)).unwrap_or(index as u32 + 1);
        let primary = info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;
        displays.push((number, primary, width, height));
    }
    // Primary first so a fresh install defaults to the main screen.
    displays.sort_by_key(|&(number, primary, _, _)| (!primary, number));

    Ok(displays
        .into_iter()
        .map(|(number, primary, width, height)| CaptureSource {
            id: format!("display:{number}"),
            name: display_name(number, primary),
            source_type: CaptureSourceType::Display,
            width,
            height,
        })
        .collect())
}

/// The monitor behind a `display:<n>` source ID.
pub(crate) fn monitor_for_display(number: u32) -> Option<HMONITOR> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let monitors = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        monitors.push(monitor);
        true.into()
    }
    let mut monitors: Vec<HMONITOR> = Vec::new();
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut monitors as *mut Vec<HMONITOR> as isize),
        )
    };
    monitors.into_iter().find(|&monitor| {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        unsafe {
            GetMonitorInfoW(
                monitor,
                &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
            )
        }
        .as_bool()
            && display_number(&from_wide(&info.szDevice)) == Some(number)
    })
}

/// `\\.\DISPLAY2` → `2`.
fn display_number(device: &str) -> Option<u32> {
    device.strip_prefix(r"\\.\DISPLAY")?.parse().ok()
}

fn display_name(number: u32, primary: bool) -> String {
    if primary {
        format!("Display {number} (Primary)")
    } else {
        format!("Display {number}")
    }
}

fn windows() -> Vec<CaptureSource> {
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        let handles = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
        handles.push(hwnd);
        true.into()
    }

    let mut handles: Vec<HWND> = Vec::new();
    // A failed enumeration leaves the display list usable on its own.
    let _ = unsafe {
        EnumWindows(
            Some(collect),
            LPARAM(&mut handles as *mut Vec<HWND> as isize),
        )
    };
    let own_pid = std::process::id();
    handles
        .into_iter()
        .filter_map(|hwnd| window_source(hwnd, own_pid))
        .collect()
}

/// A visible, uncloaked, unowned app window of another process. Minimized
/// windows are skipped: capture yields no frames for them.
fn window_source(hwnd: HWND, own_pid: u32) -> Option<CaptureSource> {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        if matches!(GetWindow(hwnd, GW_OWNER), Ok(owner) if !owner.is_invalid()) {
            return None;
        }
        if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return None;
        }
        let mut cloaked: u32 = 0;
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
        {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == own_pid {
            return None;
        }

        let mut buffer = [0u16; 512];
        let title_len = GetWindowTextW(hwnd, &mut buffer).max(0) as usize;
        let title = String::from_utf16_lossy(&buffer[..title_len]);
        if title.trim().is_empty() {
            return None;
        }
        let class_len = GetClassNameW(hwnd, &mut buffer).max(0) as usize;
        let class = String::from_utf16_lossy(&buffer[..class_len]);
        if SHELL_WINDOW_CLASSES.contains(&class.as_str()) {
            return None;
        }

        let mut bounds = RECT::default();
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut bounds as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )
        .ok()?;
        let width = (bounds.right - bounds.left).max(0) as u32;
        let height = (bounds.bottom - bounds.top).max(0) as u32;
        if width < 2 || height < 2 {
            return None;
        }

        let app = process_name(pid).unwrap_or_else(|| "Application".into());
        Some(CaptureSource {
            id: format!("window:{}", hwnd.0 as usize),
            name: format!("{app} — {title}"),
            source_type: CaptureSourceType::Window,
            width,
            height,
        })
    }
}

/// Executable name without extension, e.g. `Code` for `Code.exe`.
fn process_name(pid: u32) -> Option<String> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    let queried = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    queried.ok()?;
    let path = String::from_utf16_lossy(&buffer[..len as usize]);
    Path::new(&path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_ids_follow_gdi_device_numbers() {
        assert_eq!(display_number(r"\\.\DISPLAY1"), Some(1));
        assert_eq!(display_number(r"\\.\DISPLAY12"), Some(12));
        assert_eq!(display_number(r"\\.\DISPLAYX"), None);
        assert_eq!(display_number("DISPLAY1"), None);
        assert_eq!(display_name(2, true), "Display 2 (Primary)");
        assert_eq!(display_name(3, false), "Display 3");
    }

    #[test]
    fn enumerates_at_least_one_display_with_pixels() {
        let sources = capture_sources().unwrap();
        let display = sources
            .iter()
            .find(|s| s.source_type == CaptureSourceType::Display)
            .expect("a Windows session has a display");
        assert!(display.id.starts_with("display:"));
        assert!(display.width > 0 && display.height > 0);
        assert!(sources
            .iter()
            .filter(|s| s.source_type == CaptureSourceType::Window)
            .all(|s| s.id.starts_with("window:") && s.name.contains(" — ")));
    }
}
