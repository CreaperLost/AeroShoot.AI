//! Live preview capture before recording: the selected screen and webcam go
//! to the preview mailbox, and the mic and system audio feed level meters.
//! Recording stops this and feeds the same mailbox itself, as on macOS.
use super::record::audio::{self, AudioSource};
use super::record::camera;
use super::record::frames;
use super::record::gpu;
use super::record::screen_capture::{ScreenCapture, ScreenTarget};
use super::ComApartment;
use ::windows::Win32::Media::MediaFoundation::{
    MFShutdown, MFStartup, MFSTARTUP_NOSOCKET, MF_VERSION,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

const SCREEN_POLL: Duration = Duration::from_millis(16);

struct PreviewCapture {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    system_peak: Option<Arc<AtomicU64>>,
    mic_peak: Option<Arc<AtomicU64>>,
}

static PREVIEW: Mutex<Option<PreviewCapture>> = Mutex::new(None);

pub fn start(
    source_id: &str,
    capture_screen: bool,
    capture_system_audio: bool,
    camera_id: Option<&str>,
    mic_id: Option<&str>,
    mic_gain_db: f64,
) -> Result<(), String> {
    stop();
    frames::clear();
    frames::set_screen_shown(capture_screen);
    let mut preview = PreviewCapture {
        stop: Arc::new(AtomicBool::new(false)),
        threads: Vec::new(),
        system_peak: None,
        mic_peak: None,
    };
    let started = (|| {
        if capture_screen {
            // Not excluded from capture: that would hide AeroShoot from every
            // capture tool (Teams, OBS) whenever the Record screen is open. The
            // preview may show AeroShoot itself; recordings still exclude it.
            let target = ScreenTarget::from_source_id(source_id)?;
            preview
                .threads
                .push(spawn_screen(target, preview.stop.clone())?);
        }
        if let Some(camera_id) = camera_id {
            // As on macOS: the screen preview stays useful when the camera is
            // busy or denied.
            if let Ok(thread) = spawn_camera(camera_id, preview.stop.clone()) {
                preview.threads.push(thread);
            }
        }
        if let Some(mic_id) = mic_id {
            let peak = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
            let source = AudioSource::Microphone {
                endpoint_id: mic_id.to_string(),
                gain_db: mic_gain_db as f32,
            };
            if let Ok(thread) = audio::spawn_meter(source, preview.stop.clone(), peak.clone()) {
                preview.threads.push(thread);
                preview.mic_peak = Some(peak);
            }
        }
        if capture_system_audio {
            let peak = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
            if let Ok(thread) = audio::spawn_meter(
                AudioSource::SystemLoopback,
                preview.stop.clone(),
                peak.clone(),
            ) {
                preview.threads.push(thread);
                preview.system_peak = Some(peak);
            }
        }
        Ok(())
    })();
    if let Err(error) = started {
        shutdown(preview);
        return Err(error);
    }
    *PREVIEW.lock().unwrap_or_else(PoisonError::into_inner) = Some(preview);
    Ok(())
}

/// Stop capturing and release every device (a recording may need them).
pub fn stop() {
    let preview = PREVIEW
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(preview) = preview {
        shutdown(preview);
        frames::clear();
    }
}

/// (system audio, microphone) peaks in dBFS while the preview runs.
pub fn audio_levels() -> (Option<f64>, Option<f64>) {
    let guard = PREVIEW.lock().unwrap_or_else(PoisonError::into_inner);
    let read = |peak: &Option<Arc<AtomicU64>>| {
        peak.as_ref()
            .map(|p| f64::from_bits(p.load(Ordering::Relaxed)))
            .filter(|db| !db.is_nan())
    };
    match guard.as_ref() {
        Some(preview) => (read(&preview.system_peak), read(&preview.mic_peak)),
        None => (None, None),
    }
}

/// A stalled device thread is abandoned after this long rather than hanging
/// the app; running devices return within one frame.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

fn shutdown(preview: PreviewCapture) {
    // Each thread sees the flag after its current read and releases its own
    // device. Shutting the camera source down from here instead can deadlock
    // a blocked synchronous Source Reader.
    preview.stop.store(true, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + STOP_TIMEOUT;
    for thread in preview.threads {
        while !thread.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if thread.is_finished() {
            let _ = thread.join();
        }
    }
}

fn spawn_screen(target: ScreenTarget, stop: Arc<AtomicBool>) -> Result<JoinHandle<()>, String> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("aeroshoot-preview-screen".into())
        .spawn(move || {
            let _com = match ComApartment::enter() {
                Ok(com) => com,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            let gpu = match gpu::shared() {
                Ok(gpu) => gpu,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            let mut capture = match ScreenCapture::open(&gpu, target, true) {
                Ok(capture) => capture,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            while !stop.load(Ordering::SeqCst) && !capture.is_closed() {
                let drained =
                    capture.drain(|texture, content| frames::offer_screen(&gpu, texture, content));
                if drained.is_err() {
                    break;
                }
                std::thread::sleep(SCREEN_POLL);
            }
        })
        .map_err(|e| format!("Could not start the screen preview: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(thread),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => Err("The screen preview exited during startup".into()),
    }
}

fn spawn_camera(camera_id: &str, stop: Arc<AtomicBool>) -> Result<JoinHandle<()>, String> {
    let camera_id = camera_id.to_string();
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("aeroshoot-preview-camera".into())
        .spawn(move || {
            let _com = match ComApartment::enter() {
                Ok(com) => com,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            if let Err(error) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) } {
                let _ = ready_tx.send(Err(error.to_string()));
                return;
            }
            let (width, height) = (camera::CAMERA_WIDTH, camera::CAMERA_HEIGHT);
            match camera::open_reader(&camera_id, width, height, 30) {
                Ok((source, reader)) => {
                    let _ = ready_tx.send(Ok(()));
                    while !stop.load(Ordering::SeqCst) {
                        match camera::read_sample(&reader) {
                            Ok(Some((sample, _))) => {
                                if let Some(frame) = camera::preview_frame(&sample, width, height) {
                                    frames::offer_camera(frame);
                                }
                            }
                            Ok(None) => {}
                            Err(_) => break,
                        }
                    }
                    let _ = unsafe { source.Shutdown() };
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            }
            let _ = unsafe { MFShutdown() };
        })
        .map_err(|e| format!("Could not start the camera preview: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(thread),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => Err("The camera preview exited during startup".into()),
    }
}
