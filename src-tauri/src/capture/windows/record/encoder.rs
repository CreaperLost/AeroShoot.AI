//! H.264 MP4 segment writer on the Media Foundation sink writer, using the
//! hardware encoder when one is available. Input is NV12, either GPU textures
//! (screen) or system-memory samples (camera).
use ::windows::core::{Interface, GUID, HSTRING};
use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use ::windows::Win32::Media::MediaFoundation::*;
use ::windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_UI4};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug)]
pub struct VideoFormat {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
}

impl VideoFormat {
    /// Same automatic rate as the macOS recorder: at least 4 Mbps.
    pub fn automatic_bitrate(width: u32, height: u32, fps: u32) -> u32 {
        let rate = u64::from(width) * u64::from(height) * u64::from(fps.max(1)) / 5;
        rate.clamp(4_000_000, u64::from(u32::MAX)) as u32
    }
}

pub struct H264Segment {
    writer: IMFSinkWriter,
    stream: u32,
    path: PathBuf,
    frames: u64,
    end_hns: u64,
}

// SAFETY: the Media Foundation sink writer is a free-threaded COM object
// created in the multithreaded apartment. A segment is used by one thread at
// a time and only moves to the publisher thread to be finished.
unsafe impl Send for H264Segment {}

/// The DXGI device manager is thread-safe by design; the publisher thread
/// uses it to prepare the next screen encoder.
#[derive(Clone)]
pub struct SharedDeviceManager(pub IMFDXGIDeviceManager);

// SAFETY: see above; IMFDXGIDeviceManager serializes device access itself.
unsafe impl Send for SharedDeviceManager {}

impl H264Segment {
    pub fn create(
        path: &Path,
        format: VideoFormat,
        device_manager: Option<&IMFDXGIDeviceManager>,
    ) -> ::windows::core::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        unsafe {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 4)?;
            let attributes = attributes.ok_or_else(::windows::core::Error::empty)?;
            // The `.mp4.tmp` name hides the container from extension sniffing.
            attributes.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
            attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            if let Some(manager) = device_manager {
                attributes.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, manager)?;
            }
            let writer =
                MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attributes)?;

            let output = video_type(format, &MFVideoFormat_H264)?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, format.bitrate_bps)?;
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
            let stream = writer.AddStream(&output)?;
            let input = video_type(format, &MFVideoFormat_NV12)?;
            writer.SetInputMediaType(stream, &input, None)?;
            set_keyframe_interval(&writer, stream, format.fps.max(1) * 2);
            writer.BeginWriting()?;
            Ok(Self {
                writer,
                stream,
                path: path.to_path_buf(),
                frames: 0,
                end_hns: 0,
            })
        }
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Media time covered so far, in microseconds.
    pub fn duration_us(&self) -> u64 {
        self.end_hns / 10
    }

    /// Encode an NV12 GPU texture at `time_hns` (relative to segment start).
    pub fn write_texture(
        &mut self,
        texture: &ID3D11Texture2D,
        time_hns: u64,
        duration_hns: u64,
    ) -> ::windows::core::Result<()> {
        unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false)?;
            let length = buffer.cast::<IMF2DBuffer>()?.GetContiguousLength()?;
            buffer.SetCurrentLength(length)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            self.write_sample(&sample, time_hns, duration_hns)
        }
    }

    /// Encode an NV12 sample at `time_hns` (relative to segment start).
    pub fn write_sample(
        &mut self,
        sample: &IMFSample,
        time_hns: u64,
        duration_hns: u64,
    ) -> ::windows::core::Result<()> {
        unsafe {
            sample.SetSampleTime(time_hns as i64)?;
            sample.SetSampleDuration(duration_hns as i64)?;
            self.writer.WriteSample(self.stream, sample)?;
        }
        self.frames += 1;
        self.end_hns = self.end_hns.max(time_hns + duration_hns);
        Ok(())
    }

    /// Flush the encoder and close the MP4 file.
    pub fn finish(self) -> ::windows::core::Result<PathBuf> {
        unsafe { self.writer.Finalize()? };
        drop(self.writer);
        // Flushing needs write access on Windows.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&self.path)
            .and_then(|file| file.sync_all())
            .map_err(|e| {
                ::windows::core::Error::new(::windows::core::HRESULT(-1), e.to_string())
            })?;
        Ok(self.path)
    }

    /// Abandon the segment without producing a file.
    pub fn discard(self) {
        let path = self.path.clone();
        drop(self);
        let _ = std::fs::remove_file(path);
    }
}

fn video_type(format: VideoFormat, subtype: &GUID) -> ::windows::core::Result<IMFMediaType> {
    unsafe {
        let media_type = MFCreateMediaType()?;
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        media_type.SetUINT64(&MF_MT_FRAME_SIZE, pack(format.width, format.height))?;
        media_type.SetUINT64(&MF_MT_FRAME_RATE, pack(format.fps.max(1), 1))?;
        media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        media_type.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
        media_type.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        Ok(media_type)
    }
}

fn pack(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

/// A keyframe every two seconds, like the macOS recorder. Best effort: an
/// encoder without the setting keeps its own default.
fn set_keyframe_interval(writer: &IMFSinkWriter, stream: u32, frames: u32) {
    unsafe {
        let mut codec: *mut std::ffi::c_void = std::ptr::null_mut();
        if writer
            .GetServiceForStream(stream, &GUID::zeroed(), &ICodecAPI::IID, &mut codec)
            .is_err()
            || codec.is_null()
        {
            return;
        }
        let codec = ICodecAPI::from_raw(codec);
        let value = VARIANT {
            Anonymous: VARIANT_0 {
                Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                    vt: VT_UI4,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: VARIANT_0_0_0 { ulVal: frames },
                }),
            },
        };
        let _ = codec.SetValue(&CODECAPI_AVEncMPVGOPSize, &value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_bitrate_matches_macos() {
        assert_eq!(VideoFormat::automatic_bitrate(1920, 1080, 30), 12_441_600);
        assert_eq!(VideoFormat::automatic_bitrate(640, 360, 30), 4_000_000);
    }
}

#[cfg(test)]
mod timing {
    use super::*;
    use crate::capture::windows::record::gpu::Converter;
    use std::time::Instant;

    #[test]
    #[ignore = "hardware timing probe"]
    fn encoder_step_timings() {
        let _com = crate::capture::windows::ComApartment::enter().unwrap();
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).unwrap() };
        let gpu = crate::capture::windows::record::gpu::shared().unwrap();
        let mut converter = Converter::new(&gpu, (1920, 1080), (1920, 1080), 60).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let format = VideoFormat {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_bps: 20_000_000,
        };
        for round in 0..3 {
            let t = Instant::now();
            let mut segment = H264Segment::create(
                &dir.path().join(format!("{round}.mp4.tmp")),
                format,
                Some(&gpu.manager),
            )
            .unwrap();
            let created = t.elapsed();
            let t = Instant::now();
            for frame in 0..600u64 {
                let texture = converter.convert().unwrap();
                segment
                    .write_texture(&texture, frame * 166_666, 166_666)
                    .unwrap();
            }
            let wrote = t.elapsed();
            let t = Instant::now();
            segment.finish().unwrap();
            println!(
                "round {round}: create {created:?}, 600 frames {wrote:?}, finish {:?}",
                t.elapsed()
            );
        }
    }
}
