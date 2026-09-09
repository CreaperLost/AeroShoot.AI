//! One media worker per desktop app. Decode/mix/GPU work never runs on the UI thread.
use super::{audio::AudioOutput, PlaybackState};
use crate::{
    commands::AppState,
    export::SceneEvaluator,
    media::audio::{AudioMixer, CHUNK_FRAMES, SAMPLE_RATE},
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};
use tauri::Manager;
struct Runtime {
    generation: u64,
    evaluator: SceneEvaluator,
    mixer: AudioMixer,
    _lease: std::fs::File,
}

pub fn start(app: tauri::AppHandle) {
    thread::spawn(move || {
        let mut runtime: Option<Runtime> = None;
        let pending = Arc::new(AtomicBool::new(false));
        let mut last_frame: Option<(u64, u64, u64)> = None;
        while !app
            .state::<AppState>()
            .playback_shutdown
            .load(Ordering::Acquire)
        {
            let state = app.state::<AppState>();
            let result = tick(&app, &state, &mut runtime, &pending, &mut last_frame);
            if let Err((generation, error)) = result {
                state.playback.lock().fail(generation, error);
                let app_copy = app.clone();
                let _ = app.run_on_main_thread(move || {
                    let state = app_copy.state::<AppState>();
                    if state
                        .playback
                        .lock()
                        .status()
                        .is_ok_and(|s| s.generation == generation)
                    {
                        let mut preview = state.preview.lock();
                        let snapshot = preview.status();
                        if snapshot.attached {
                            let _ = preview.present_fixed(0.0, 0.0, 0.0, snapshot.generation);
                        }
                    }
                });
            }
            thread::sleep(Duration::from_millis(15));
        }
        app.state::<AppState>().playback.lock().close();
    });
}
fn tick(
    app: &tauri::AppHandle,
    state: &AppState,
    runtime: &mut Option<Runtime>,
    pending: &Arc<AtomicBool>,
    last_frame: &mut Option<(u64, u64, u64)>,
) -> Result<(), (u64, String)> {
    let status = state.playback.lock().status().map_err(|e| (0, e))?;
    if matches!(status.state, PlaybackState::Closed | PlaybackState::Error) {
        *runtime = None;
        return Ok(());
    }
    let generation = status.generation;
    let error = |e| (generation, e);
    if runtime.as_ref().map(|r| r.generation) != Some(generation) {
        let (root, document, tracks) = {
            let opened = state.opened_project.lock();
            let Some(reader) = opened.as_ref() else {
                return Ok(());
            };
            if reader.summary.project_handle != status.project_handle {
                return Ok(());
            }
            (
                reader.root().to_path_buf(),
                reader.history().current.clone(),
                super::tracks_from_reader(reader),
            )
        };
        let lease = crate::project::reader::acquire_read_lease(&root).map_err(error)?;
        let mixer = AudioMixer::new(&root, &document, &tracks).map_err(error)?;
        let evaluator = SceneEvaluator::new(root, document, tracks, 1920, 1080).map_err(error)?;
        *runtime = Some(Runtime {
            generation,
            evaluator,
            mixer,
            _lease: lease,
        });
        *last_frame = None;
    }
    let runtime = runtime.as_mut().unwrap();
    if status.state == PlaybackState::Playing && runtime.mixer.has_audio() {
        let initialize = {
            let owner = state.playback.lock();
            owner.audio.is_none()
        };
        if initialize {
            let base = (status.position_us as u128 * SAMPLE_RATE as u128 / 1_000_000) as u64;
            let mut output = AudioOutput::new().map_err(error)?;
            let mut queued = base;
            while queued < (base + CHUNK_FRAMES as u64 * 3).min(runtime.mixer.total_frames) {
                let chunk = runtime
                    .mixer
                    .read_frames(queued, CHUNK_FRAMES)
                    .map_err(error)?;
                if chunk.is_empty() {
                    break;
                }
                queued += (chunk.len() / 2) as u64;
                output.queue(&chunk).map_err(error)?;
            }
            let mut owner = state.playback.lock();
            let current = owner.status().map_err(error)?;
            if current.generation != generation || current.state != PlaybackState::Playing {
                return Ok(());
            }
            owner.audio_start_frame = base;
            owner.audio_queued_frame = queued;
            output.play();
            owner.audio = Some(output);
        }
        loop {
            let (queued, position) = {
                let mut owner = state.playback.lock();
                let current = owner.status().map_err(error)?;
                if current.generation != generation || current.state != PlaybackState::Playing {
                    break;
                }
                (
                    owner.audio_queued_frame,
                    (current.position_us as u128 * SAMPLE_RATE as u128 / 1_000_000) as u64,
                )
            };
            if queued >= runtime.mixer.total_frames || queued >= position + 3 * CHUNK_FRAMES as u64
            {
                break;
            }
            let chunk = runtime
                .mixer
                .read_frames(queued, CHUNK_FRAMES)
                .map_err(error)?;
            if chunk.is_empty() {
                break;
            }
            let mut owner = state.playback.lock();
            let current = owner.status().map_err(error)?;
            if current.generation != generation || current.state != PlaybackState::Playing {
                return Ok(());
            }
            if let Some(output) = owner.audio.as_mut() {
                output.queue(&chunk).map_err(error)?;
                owner.audio_queued_frame += (chunk.len() / 2) as u64;
            } else {
                break;
            }
        }
    }
    let status = state.playback.lock().status().map_err(error)?;
    if status.generation != generation {
        return Ok(());
    }
    let preview = state.preview.lock().status();
    if !preview.attached
        || !preview.visible
        || status.duration_us == 0
        || status.position_us >= status.duration_us
        || pending.load(Ordering::Acquire)
    {
        return Ok(());
    }
    let key = (generation, status.position_us, preview.generation);
    if *last_frame == Some(key) {
        return Ok(());
    }
    let frame = runtime
        .evaluator
        .preview_at(status.position_us)
        .map_err(error)?;
    pending.store(true, Ordering::Release);
    let pending_done = Arc::clone(pending);
    let app_copy = app.clone();
    let surface_generation = preview.generation;
    app.run_on_main_thread(move || {
        let state = app_copy.state::<AppState>();
        let mut owner = state.playback.lock();
        if owner.status().is_ok_and(|s| {
            s.generation == generation
                && !matches!(s.state, PlaybackState::Closed | PlaybackState::Error)
        }) {
            let mut surface = state.preview.lock();
            let current = surface.status();
            if current.attached && current.generation == surface_generation && current.visible {
                match surface.present_frame(&frame, surface_generation) {
                    Ok(()) => {
                        owner.mark_presented(generation);
                    }
                    Err(e) => owner.fail(generation, e),
                }
            }
        }
        pending_done.store(false, Ordering::Release);
    })
    .map_err(|e| {
        pending.store(false, Ordering::Release);
        error(e.to_string())
    })?;
    *last_frame = Some(key);
    Ok(())
}
