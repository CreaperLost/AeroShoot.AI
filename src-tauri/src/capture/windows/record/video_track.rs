//! Segment lifecycle shared by the screen and camera recorders: the next
//! encoder is always prepared ahead on the publisher thread, and closed
//! segments are finished there too. Starting or finishing a hardware encoder
//! takes long enough (0.15–1.3 s measured) to drop frames if the capture
//! thread did it.
use super::encoder::{H264Segment, SharedDeviceManager, VideoFormat};
use super::track::{job, Publisher, Reply, TrackContext, SEGMENT_TARGET_US};
use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use ::windows::Win32::Media::MediaFoundation::IMFSample;
use std::sync::mpsc::{self, Receiver};

struct OpenVideo {
    encoder: H264Segment,
    index: u32,
    anchor_us: u64,
    /// Source time (hns) that maps to media time zero in this segment.
    origin_hns: u64,
}

pub struct VideoSegments {
    context: TrackContext,
    format: VideoFormat,
    device_manager: Option<SharedDeviceManager>,
    publisher: Publisher,
    segment: Option<OpenVideo>,
    prepared: Option<H264Segment>,
    preparing: Option<Receiver<Result<H264Segment, String>>>,
    next_index: u32,
}

impl VideoSegments {
    /// Starts the publisher and prepares the first encoder before returning,
    /// so recording can begin without delay.
    pub fn new(
        context: &TrackContext,
        format: VideoFormat,
        device_manager: Option<SharedDeviceManager>,
    ) -> Result<Self, String> {
        let mut segments = Self {
            context: context.clone(),
            format,
            publisher: Publisher::start(context)?,
            device_manager,
            segment: None,
            prepared: None,
            preparing: None,
            next_index: 0,
        };
        segments.prepared = Some(create_encoder(
            &segments.context,
            0,
            format,
            segments.device_manager.clone(),
        )?);
        Ok(segments)
    }

    pub fn is_open(&self) -> bool {
        self.segment.is_some()
    }

    /// Encode an NV12 texture captured at source time `time_hns`.
    pub fn write_texture(
        &mut self,
        texture: &ID3D11Texture2D,
        anchor_us: u64,
        time_hns: u64,
        duration_hns: u64,
    ) -> Result<(), String> {
        let track = self.context.track_id;
        let segment = self.open(anchor_us, time_hns)?;
        let media_time = media_time(segment, time_hns);
        segment
            .encoder
            .write_texture(texture, media_time, duration_hns)
            .map_err(|e| format!("{track} encoding failed: {e}"))?;
        self.rotate_if_full()
    }

    /// Encode an NV12 sample captured at source time `time_hns`.
    pub fn write_sample(
        &mut self,
        sample: &IMFSample,
        anchor_us: u64,
        time_hns: u64,
        duration_hns: u64,
    ) -> Result<(), String> {
        let track = self.context.track_id;
        let segment = self.open(anchor_us, time_hns)?;
        let media_time = media_time(segment, time_hns);
        segment
            .encoder
            .write_sample(sample, media_time, duration_hns)
            .map_err(|e| format!("{track} encoding failed: {e}"))?;
        self.rotate_if_full()
    }

    /// Close the open segment and hand it to the publisher; `reply` hears
    /// once it is committed.
    pub fn close(&mut self, reply: Option<Reply>) -> Result<(), String> {
        let closed = match self.segment.take() {
            None => None,
            Some(segment) if segment.encoder.frames() == 0 => {
                segment.encoder.discard();
                None
            }
            Some(segment) => {
                let context = self.context.clone();
                Some(job(move || {
                    let track = context.track_id;
                    let path = segment
                        .encoder
                        .finish()
                        .map_err(|e| format!("Could not finish the {track} segment: {e}"))?;
                    context.publish(segment.index, segment.anchor_us, &path)
                }))
            }
        };
        self.publisher.close(Ok(closed), reply)
    }

    /// Publish everything outstanding and remove the unused prepared encoder.
    pub fn finish(&mut self) {
        self.publisher.finish();
        if let Some(Ok(prepared)) = self.preparing.take().and_then(|r| r.recv().ok()) {
            prepared.discard();
        }
        if let Some(prepared) = self.prepared.take() {
            prepared.discard();
        }
    }

    fn open(&mut self, anchor_us: u64, origin_hns: u64) -> Result<&mut OpenVideo, String> {
        if self.segment.is_none() {
            let encoder = self.take_prepared()?;
            let index = self.next_index;
            self.next_index += 1;
            self.segment = Some(OpenVideo {
                encoder,
                index,
                anchor_us,
                origin_hns,
            });
            self.prepare(self.next_index);
        }
        Ok(self.segment.as_mut().expect("segment is open"))
    }

    fn rotate_if_full(&mut self) -> Result<(), String> {
        if self
            .segment
            .as_ref()
            .is_some_and(|s| s.encoder.duration_us() >= SEGMENT_TARGET_US)
        {
            self.close(None)?;
        }
        Ok(())
    }

    /// Queue creation of the encoder for segment `index` behind any pending
    /// publications.
    fn prepare(&mut self, index: u32) {
        let (sender, receiver) = mpsc::channel();
        let context = self.context.clone();
        let format = self.format;
        let manager = self.device_manager.clone();
        self.publisher.submit(
            move || {
                let _ = sender.send(create_encoder(&context, index, format, manager));
                Ok(())
            },
            None,
        );
        self.preparing = Some(receiver);
    }

    fn take_prepared(&mut self) -> Result<H264Segment, String> {
        if let Some(prepared) = self.prepared.take() {
            return Ok(prepared);
        }
        match self.preparing.take() {
            // Normally ready long before the 55 s rotation.
            Some(receiver) => receiver
                .recv()
                .map_err(|_| "The encoder could not be prepared".to_string())?,
            None => create_encoder(
                &self.context,
                self.next_index,
                self.format,
                self.device_manager.clone(),
            ),
        }
    }
}

/// Media time for a sample, never before the end of the previous one (a
/// camera timestamp can step back across a driver glitch).
fn media_time(segment: &OpenVideo, time_hns: u64) -> u64 {
    time_hns
        .saturating_sub(segment.origin_hns)
        .max(segment.encoder.duration_us() * 10)
}

fn create_encoder(
    context: &TrackContext,
    index: u32,
    format: VideoFormat,
    manager: Option<SharedDeviceManager>,
) -> Result<H264Segment, String> {
    H264Segment::create(
        &context.segment_path(index),
        format,
        manager.as_ref().map(|m| &m.0),
    )
    .map_err(|e| format!("Could not start the {} encoder: {e}", context.track_id))
}
