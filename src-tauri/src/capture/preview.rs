//! Native Record-scene preview. Configuration shares the recording lifecycle lock.
//!
//! `start` / `stop` bring the SCStream + AVCaptureSession up and tear them down.
//! Frames never reach Rust: the Swift `LivePreviewRenderer` draws the newest
//! capture frames straight into the native preview view on the GPU.
#[cfg(target_os = "macos")]
extern "C" {
    fn aeroshoot_live_preview_start(
        source: *const std::ffi::c_char,
        capture_screen: bool,
        capture_system_audio: bool,
        camera: *const std::ffi::c_char,
        mic: *const std::ffi::c_char,
        mic_gain_db: f64,
    ) -> *mut std::ffi::c_char;
    fn aeroshoot_live_preview_stop();
    fn aeroshoot_live_preview_copy_levels_json() -> *mut std::ffi::c_char;
    fn aeroshoot_macos_free_string(value: *mut std::ffi::c_char);
}

pub fn stop() {
    #[cfg(target_os = "macos")]
    unsafe {
        aeroshoot_live_preview_stop();
    }
}

pub fn start(
    source: &str,
    capture_screen: bool,
    capture_system_audio: bool,
    camera: Option<&str>,
    mic: Option<&str>,
    mic_gain_db: f64,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    unsafe {
        let source = std::ffi::CString::new(source).map_err(|_| "Invalid source ID")?;
        let camera = camera
            .map(std::ffi::CString::new)
            .transpose()
            .map_err(|_| "Invalid camera ID")?;
        let mic = mic
            .map(std::ffi::CString::new)
            .transpose()
            .map_err(|_| "Invalid microphone ID")?;
        let error = aeroshoot_live_preview_start(
            source.as_ptr(),
            capture_screen,
            capture_system_audio,
            camera.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
            mic.as_ref().map_or(std::ptr::null(), |m| m.as_ptr()),
            mic_gain_db.clamp(-24.0, 24.0),
        );
        if !error.is_null() {
            let message = std::ffi::CStr::from_ptr(error)
                .to_string_lossy()
                .into_owned();
            libc::free(error.cast());
            return Err(message);
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (
            source,
            capture_screen,
            capture_system_audio,
            camera,
            mic,
            mic_gain_db,
        );
        Err("Native capture preview is unavailable on this platform".into())
    }
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PreviewAudioLevels {
    pub system_audio_peak_db: Option<f64>,
    pub mic_peak_db: Option<f64>,
}

pub fn audio_levels() -> PreviewAudioLevels {
    #[cfg(target_os = "macos")]
    unsafe {
        let pointer = aeroshoot_live_preview_copy_levels_json();
        if pointer.is_null() {
            return PreviewAudioLevels::default();
        }
        let json = std::ffi::CStr::from_ptr(pointer)
            .to_string_lossy()
            .into_owned();
        aeroshoot_macos_free_string(pointer);
        serde_json::from_str(&json).unwrap_or_default()
    }
    #[cfg(not(target_os = "macos"))]
    PreviewAudioLevels::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_levels_accept_null_peaks_from_swift() {
        let levels: PreviewAudioLevels =
            serde_json::from_str(r#"{"systemAudioPeakDb":null,"micPeakDb":-12.5}"#).unwrap();
        assert_eq!(levels.system_audio_peak_db, None);
        assert_eq!(levels.mic_peak_db, Some(-12.5));
    }
}
