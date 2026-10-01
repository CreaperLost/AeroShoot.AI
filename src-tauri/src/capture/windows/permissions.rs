//! Camera and microphone privacy state from the Windows capability consent
//! store. Screen capture needs no permission on Windows. Desktop apps get no
//! consent prompt either: the user changes the toggles in Settings.
use crate::capture::{PermissionState, PermissionStatus};
use ::windows::core::{w, HSTRING};
use ::windows::Win32::Foundation::ERROR_SUCCESS;
use ::windows::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
};

const CONSENT_STORE: &str =
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore";

pub fn permissions() -> PermissionStatus {
    PermissionStatus {
        screen_recording: PermissionState::Authorized,
        camera: capability_state("webcam"),
        microphone: capability_state("microphone"),
    }
}

/// Windows cannot prompt a desktop app; report the current state so the UI
/// can send the user to Settings.
pub fn request_permissions(_screen: bool, _camera: bool, _microphone: bool) -> PermissionStatus {
    permissions()
}

fn capability_state(capability: &str) -> PermissionState {
    let key = format!(r"{CONSENT_STORE}\{capability}");
    consent_state(
        consent_value(HKEY_LOCAL_MACHINE, &key),
        consent_value(HKEY_CURRENT_USER, &key),
        consent_value(HKEY_CURRENT_USER, &format!(r"{key}\NonPackaged")),
    )
}

/// Combine the three Settings toggles that gate an unpackaged desktop app:
/// device-wide access (machine-wide, needs an administrator), "Let apps
/// access" for this user, and "Let desktop apps access". A missing value
/// means the toggle was never turned off.
fn consent_state(
    device_wide: Option<String>,
    user: Option<String>,
    desktop_apps: Option<String>,
) -> PermissionState {
    let denied = |value: &Option<String>| {
        value
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("Deny"))
    };
    if denied(&device_wide) {
        PermissionState::Restricted
    } else if denied(&user) || denied(&desktop_apps) {
        PermissionState::Denied
    } else {
        PermissionState::Authorized
    }
}

fn consent_value(root: HKEY, subkey: &str) -> Option<String> {
    let subkey = HSTRING::from(subkey);
    let mut buffer = [0u16; 64];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    let status = unsafe {
        RegGetValueW(
            root,
            &subkey,
            w!("Value"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then(|| super::from_wide(&buffer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(text: &str) -> Option<String> {
        Some(text.into())
    }

    #[test]
    fn missing_or_allowed_toggles_authorize() {
        assert_eq!(consent_state(None, None, None), PermissionState::Authorized);
        assert_eq!(
            consent_state(value("Allow"), value("Allow"), value("Allow")),
            PermissionState::Authorized
        );
    }

    #[test]
    fn user_or_desktop_app_toggle_denies() {
        assert_eq!(
            consent_state(value("Allow"), value("Deny"), None),
            PermissionState::Denied
        );
        assert_eq!(
            consent_state(None, value("Allow"), value("deny")),
            PermissionState::Denied
        );
    }

    #[test]
    fn device_wide_toggle_restricts() {
        assert_eq!(
            consent_state(value("Deny"), value("Allow"), value("Allow")),
            PermissionState::Restricted
        );
    }

    #[test]
    fn reads_live_state_without_prompting() {
        let status = permissions();
        assert_eq!(status.screen_recording, PermissionState::Authorized);
        assert_eq!(request_permissions(true, true, true), status);
    }
}
