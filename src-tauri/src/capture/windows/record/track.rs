//! Per-track plumbing shared by the audio and video recorders: live counters,
//! the pause/finalize/stop control channel, segment paths, and publication.
use crate::capture::segments;
use crate::project::manifest::TrackType;
use crate::project::media_validator::MediaValidator;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use super::clock::now_hns;

/// Rotate with headroom under the 60-second segment limit, as on macOS.
pub const SEGMENT_TARGET_US: u64 = 55_000_000;

/// Error code reported for Windows capture failures.
pub const CAPTURE_ERROR: i32 = -620;

/// Live counters read by `stats()` from any thread.
#[derive(Default)]
pub struct TrackStats {
    /// Samples (audio packets or video frames) delivered by the device,
    /// including countdown warm-up; readiness waits for the first one.
    pub samples: AtomicU64,
    last_sample_hns: AtomicU64,
    peak_db_bits: AtomicU64,
    pub gaps: AtomicU64,
    pub dropped: AtomicU64,
    error: Mutex<Option<String>>,
}

impl TrackStats {
    pub fn new() -> Arc<Self> {
        let stats = Self::default();
        stats
            .peak_db_bits
            .store(f64::NAN.to_bits(), Ordering::Relaxed);
        Arc::new(stats)
    }

    pub fn record_sample(&self) {
        self.samples.fetch_add(1, Ordering::Relaxed);
        self.last_sample_hns.store(now_hns(), Ordering::Relaxed);
    }

    pub fn set_peak_db(&self, value: f64) {
        self.peak_db_bits.store(value.to_bits(), Ordering::Relaxed);
    }

    pub fn peak_db(&self) -> Option<f64> {
        let value = f64::from_bits(self.peak_db_bits.load(Ordering::Relaxed));
        (!value.is_nan()).then_some(value)
    }

    pub fn last_sample_age_ms(&self) -> Option<u64> {
        match self.last_sample_hns.load(Ordering::Relaxed) {
            0 => None,
            last => Some(now_hns().saturating_sub(last) / 10_000),
        }
    }

    /// Keep the first error: it is the cause, later ones are consequences.
    pub fn fail(&self, message: String) {
        let mut error = self.error.lock().unwrap_or_else(PoisonError::into_inner);
        if error.is_none() {
            *error = Some(message);
        }
    }

    pub fn error(&self) -> Option<String> {
        self.error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Requests from the session to a track thread.
pub enum TrackCommand {
    /// Close and publish the open segment, then reply.
    Finalize(Sender<Result<(), String>>),
    /// Close and publish the open segment, then exit the thread.
    Stop,
}

/// What every track thread shares with the session.
#[derive(Clone)]
pub struct TrackContext {
    pub track_id: &'static str,
    pub track_type: TrackType,
    pub project_path: PathBuf,
    pub stats: Arc<TrackStats>,
    pub paused: Arc<AtomicBool>,
}

impl TrackContext {
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// `media/<track>/<index + 1>.<ext>.tmp`, the path the publisher expects.
    pub fn segment_path(&self, index: u32) -> PathBuf {
        let extension = match self.track_type {
            TrackType::Screen | TrackType::Webcam => "mp4",
            TrackType::SystemAudio | TrackType::MicAudio => "wav",
        };
        self.project_path
            .join("media")
            .join(self.track_id)
            .join(format!("{:06}.{extension}.tmp", u64::from(index) + 1))
    }

    /// Validate and hand a closed segment to the project journal.
    pub fn publish(&self, index: u32, host_anchor_us: u64, path: &Path) -> Result<(), String> {
        let timescale = MediaValidator::validate(path, self.track_type)
            .map(|info| info.media_timescale)
            .map_err(|e| format!("{} segment {} is invalid: {e}", self.track_id, index + 1))?;
        segments::publish_segment(
            self.track_id,
            index,
            i64::try_from(host_anchor_us).unwrap_or(i64::MAX),
            timescale.max(1),
            0,
            path,
        )
        .map_err(|()| {
            format!(
                "{} segment {} could not be committed",
                self.track_id,
                index + 1
            )
        })
    }

    /// Latch a fatal track error and surface it to the session.
    pub fn fail(&self, message: String) {
        self.stats.fail(message.clone());
        segments::report_runtime_error(self.track_id.into(), CAPTURE_ERROR, message);
    }
}

/// Drain pending commands. `Stop` wins over any `Finalize` still queued.
pub fn next_command(commands: &Receiver<TrackCommand>) -> Option<TrackCommand> {
    commands.try_recv().ok()
}

pub type Reply = Sender<Result<(), String>>;
pub type Job = Box<dyn FnOnce() -> Result<(), String> + Send>;

/// Per-track background thread that finishes, validates, and journals closed
/// segments (and prepares encoders) in submission order, so the capture
/// thread never waits for a container to be flushed to disk.
pub struct Publisher {
    context: TrackContext,
    jobs: Option<Sender<(Job, Option<Reply>)>>,
    thread: Option<JoinHandle<()>>,
}

impl Publisher {
    pub fn start(context: &TrackContext) -> Result<Self, String> {
        let (jobs, receiver) = mpsc::channel::<(Job, Option<Reply>)>();
        let worker_context = context.clone();
        let thread = std::thread::Builder::new()
            .name(format!("aeroshoot-{}-publish", context.track_id))
            .spawn(move || {
                let _com = crate::capture::windows::ComApartment::enter();
                for (job, reply) in receiver {
                    let result = job();
                    if let Err(error) = &result {
                        worker_context.fail(error.clone());
                    }
                    if let Some(reply) = reply {
                        let _ = reply.send(result);
                    }
                }
            })
            .map_err(|e| format!("Could not start the {} publisher: {e}", context.track_id))?;
        Ok(Self {
            context: context.clone(),
            jobs: Some(jobs),
            thread: Some(thread),
        })
    }

    pub fn submit(
        &self,
        job: impl FnOnce() -> Result<(), String> + Send + 'static,
        reply: Option<Reply>,
    ) {
        if let Some(jobs) = &self.jobs {
            if let Err(mpsc::SendError((_, reply))) = jobs.send((Box::new(job), reply)) {
                if let Some(reply) = reply {
                    let _ = reply.send(Err("The segment publisher has stopped".into()));
                }
            }
        }
    }

    /// Reply once everything submitted so far is published, with the track's
    /// first error if any segment failed.
    pub fn barrier(&self, reply: Reply) {
        let stats = self.context.stats.clone();
        self.submit(move || stats.error().map_or(Ok(()), Err), Some(reply));
    }

    /// Hand off a closed segment (or nothing), replying when it is published.
    /// `closed` is the result of closing the segment on the capture thread.
    pub fn close(
        &self,
        closed: Result<Option<Job>, String>,
        reply: Option<Reply>,
    ) -> Result<(), String> {
        match closed {
            Ok(Some(job)) => {
                self.submit(job, reply);
                Ok(())
            }
            Ok(None) => {
                if let Some(reply) = reply {
                    self.barrier(reply);
                }
                Ok(())
            }
            Err(error) => {
                if let Some(reply) = reply {
                    let _ = reply.send(Err(error.clone()));
                }
                Err(error)
            }
        }
    }

    /// Publish everything outstanding and stop the thread.
    pub fn finish(&mut self) {
        self.jobs.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.finish();
    }
}

/// A boxed publish job, for recorders building one.
pub fn job(work: impl FnOnce() -> Result<(), String> + Send + 'static) -> Job {
    Box::new(work)
}
