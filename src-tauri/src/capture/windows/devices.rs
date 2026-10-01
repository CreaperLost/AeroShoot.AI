//! Cameras (Media Foundation) and microphones (WASAPI endpoints).
//! IDs are the camera's symbolic link and the audio endpoint ID, the values
//! the capture APIs open devices by.
use super::ComApartment;
use crate::capture::{AudioDevice, CameraDevice, CameraFormat};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, PoisonError};
use ::windows::core::{GUID, PWSTR};
use ::windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use ::windows::Win32::Media::Audio::{
    eCapture, eConsole, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use ::windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFMediaSource, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MFCreateAttributes, MFEnumDeviceSources, MFShutdown, MFStartup,
    MFSTARTUP_NOSOCKET, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_VERSION,
};
use ::windows::Win32::System::Com::StructuredStorage::{
    PropVariantClear, PropVariantToStringAlloc,
};
use ::windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};

pub fn devices() -> Result<(Vec<CameraDevice>, Vec<AudioDevice>), String> {
    // Enumerate on a dedicated MTA thread: Tauri may call in from the UI
    // thread, whose apartment this module must not change.
    std::thread::spawn(|| {
        let _com = ComApartment::enter()?;
        let cameras = cameras().map_err(|e| format!("Camera enumeration failed: {e}"))?;
        let mics = microphones().map_err(|e| format!("Microphone enumeration failed: {e}"))?;
        Ok((cameras, mics))
    })
    .join()
    .map_err(|_| "Device enumeration panicked".to_string())?
}

fn microphones() -> ::windows::core::Result<Vec<AudioDevice>> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let default_id = enumerator
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .and_then(|device| endpoint_id(&device))
            .ok();
        let endpoints = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
        let mut mics = Vec::new();
        for index in 0..endpoints.GetCount()? {
            let device = endpoints.Item(index)?;
            let id = endpoint_id(&device)?;
            let name = friendly_name(&device).unwrap_or_else(|_| "Microphone".into());
            mics.push(AudioDevice {
                is_default: default_id.as_deref() == Some(id.as_str()),
                id,
                name,
            });
        }
        Ok(mics)
    }
}

unsafe fn endpoint_id(device: &IMMDevice) -> ::windows::core::Result<String> {
    unsafe { Ok(take_co_string(device.GetId()?)) }
}

unsafe fn friendly_name(device: &IMMDevice) -> ::windows::core::Result<String> {
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ)?;
        let mut value = store.GetValue(&PKEY_Device_FriendlyName)?;
        let text = PropVariantToStringAlloc(&value);
        let _ = PropVariantClear(&mut value);
        Ok(take_co_string(text?))
    }
}

fn cameras() -> ::windows::core::Result<Vec<CameraDevice>> {
    unsafe {
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
        let cameras = enumerate_cameras();
        let _ = MFShutdown();
        cameras
    }
}

unsafe fn enumerate_cameras() -> ::windows::core::Result<Vec<CameraDevice>> {
    unsafe {
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 1)?;
        let attributes = attributes.ok_or_else(::windows::core::Error::empty)?;
        attributes.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )?;

        let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attributes, &mut list, &mut count)?;
        if list.is_null() {
            return Ok(Vec::new());
        }
        let activates = std::slice::from_raw_parts_mut(list, count as usize);
        let mut cameras = Vec::new();
        for activate in activates.iter_mut() {
            // `take` moves each reference out so it is released exactly once.
            let Some(activate) = activate.take() else {
                continue;
            };
            let Ok(id) = allocated_string(
                &activate,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
            ) else {
                continue;
            };
            let name = allocated_string(&activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
                .unwrap_or_else(|_| "Camera".into());
            let formats = native_formats(&activate, &id);
            cameras.push(CameraDevice {
                // Windows has no default-camera setting; the first is preferred.
                is_default: cameras.is_empty(),
                id,
                name,
                formats,
            });
        }
        CoTaskMemFree(Some(list as *const _));
        Ok(cameras)
    }
}

/// A camera's native modes, read once per device and cached: reading them
/// activates the camera, which takes a moment and can flash its light. Empty
/// when the camera cannot be opened (for example, another app holds it).
unsafe fn native_formats(activate: &IMFActivate, id: &str) -> Vec<CameraFormat> {
    static CACHE: Mutex<Option<HashMap<String, Vec<CameraFormat>>>> = Mutex::new(None);
    if let Some(cached) = CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|cache| cache.get(id).cloned())
    {
        return cached;
    }
    let formats = unsafe { read_formats(activate) }.unwrap_or_default();
    if !formats.is_empty() {
        CACHE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(id.to_string(), formats.clone());
    }
    formats
}

unsafe fn read_formats(activate: &IMFActivate) -> ::windows::core::Result<Vec<CameraFormat>> {
    unsafe {
        let source: IMFMediaSource = activate.ActivateObject()?;
        let read = (|| -> ::windows::core::Result<Vec<CameraFormat>> {
            let presentation = source.CreatePresentationDescriptor()?;
            let mut selected = ::windows::core::BOOL(0);
            let mut stream = None;
            presentation.GetStreamDescriptorByIndex(0, &mut selected, &mut stream)?;
            let handler = stream
                .ok_or_else(::windows::core::Error::empty)?
                .GetMediaTypeHandler()?;
            let mut modes = BTreeSet::new();
            for index in 0..handler.GetMediaTypeCount()? {
                let media_type = handler.GetMediaTypeByIndex(index)?;
                let size = media_type.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                let rate = media_type.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0);
                let (width, height) = ((size >> 32) as u32, size as u32);
                let (numerator, denominator) = ((rate >> 32) as u32, (rate as u32).max(1));
                let fps = (f64::from(numerator) / f64::from(denominator)).round() as u32;
                if width > 0 && height > 0 && fps > 0 {
                    modes.insert(CameraFormat { width, height, fps });
                }
            }
            Ok(modes.into_iter().collect())
        })();
        let _ = source.Shutdown();
        let _ = activate.ShutdownObject();
        read
    }
}

unsafe fn allocated_string(activate: &IMFActivate, key: &GUID) -> ::windows::core::Result<String> {
    unsafe {
        let mut value = PWSTR::null();
        let mut len = 0u32;
        activate.GetAllocatedString(key, &mut value, &mut len)?;
        Ok(take_co_string(value))
    }
}

/// Copy and free a COM-allocated wide string.
unsafe fn take_co_string(value: PWSTR) -> String {
    if value.is_null() {
        return String::new();
    }
    unsafe {
        let text = String::from_utf16_lossy(value.as_wide());
        CoTaskMemFree(Some(value.0 as *const _));
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumeration_succeeds_and_marks_at_most_one_default_each() {
        // Hosted CI machines may have no camera or microphone; enumeration
        // itself must still succeed.
        let (cameras, mics) = devices().unwrap();
        assert!(cameras.iter().filter(|c| c.is_default).count() <= 1);
        assert!(mics.iter().filter(|m| m.is_default).count() <= 1);
        assert!(cameras
            .iter()
            .all(|c| !c.id.is_empty() && !c.name.is_empty()));
        assert!(mics.iter().all(|m| !m.id.is_empty() && !m.name.is_empty()));
        for camera in &cameras {
            assert!(camera.formats.iter().all(|f| f.width > 0 && f.height > 0 && f.fps > 0));
        }
    }
}
