//! 16-bit PCM WAV segment file, the same format the macOS recorder writes.
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const SAMPLE_RATE: u32 = 48_000;
const HEADER_BYTES: u64 = 44;

pub struct WavSegment {
    path: PathBuf,
    file: BufWriter<File>,
    channels: u16,
    frames: u64,
}

impl WavSegment {
    pub fn create(path: &Path, channels: u16) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let mut segment = Self {
            path: path.to_path_buf(),
            file: BufWriter::new(file),
            channels,
            frames: 0,
        };
        segment.write_header(0)?;
        Ok(segment)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn duration_us(&self) -> u64 {
        self.frames * 1_000_000 / u64::from(SAMPLE_RATE)
    }

    /// Append interleaved samples; the length must be a whole number of frames.
    pub fn write(&mut self, samples: &[i16]) -> std::io::Result<()> {
        debug_assert_eq!(samples.len() % usize::from(self.channels), 0);
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.file.write_all(&bytes)?;
        self.frames += (samples.len() / usize::from(self.channels)) as u64;
        Ok(())
    }

    pub fn write_silence(&mut self, frames: u64) -> std::io::Result<()> {
        const CHUNK: u64 = 4_800;
        let mut remaining = frames;
        let zeros = vec![0i16; (CHUNK * u64::from(self.channels)) as usize];
        while remaining > 0 {
            let count = remaining.min(CHUNK);
            self.write(&zeros[..(count * u64::from(self.channels)) as usize])?;
            remaining -= count;
        }
        Ok(())
    }

    /// Patch the sizes into the header and make the file durable.
    pub fn finish(mut self) -> std::io::Result<PathBuf> {
        let data_bytes = self.frames * u64::from(self.channels) * 2;
        let data_bytes = u32::try_from(data_bytes)
            .map_err(|_| std::io::Error::other("WAV segment exceeds 4 GiB"))?;
        self.file.seek(SeekFrom::Start(0))?;
        self.write_header(data_bytes)?;
        let file = self.file.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        Ok(self.path)
    }

    fn write_header(&mut self, data_bytes: u32) -> std::io::Result<()> {
        let channels = self.channels;
        let block_align = channels * 2;
        let byte_rate = SAMPLE_RATE * u32::from(block_align);
        let mut header = Vec::with_capacity(HEADER_BYTES as usize);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes()); // PCM
        header.extend_from_slice(&channels.to_le_bytes());
        header.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        header.extend_from_slice(&byte_rate.to_le_bytes());
        header.extend_from_slice(&block_align.to_le_bytes());
        header.extend_from_slice(&16u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&data_bytes.to_le_bytes());
        self.file.write_all(&header)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::manifest::TrackType;
    use crate::project::media_validator::MediaValidator;

    #[test]
    fn finished_segment_passes_the_project_validator() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("media/mic/000001.wav.tmp");
        let mut wav = WavSegment::create(&path, 2).unwrap();
        wav.write(&[100, -100, 200, -200]).unwrap();
        wav.write_silence(47_998).unwrap();
        assert_eq!(wav.frames(), 48_000);
        let path = wav.finish().unwrap();

        let info = MediaValidator::validate(&path, TrackType::MicAudio).unwrap();
        assert_eq!(info.duration_us, 1_000_000);
        assert_eq!(info.media_timescale, 48_000);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 44 + 48_000 * 4);
    }

    #[test]
    fn never_overwrites_an_existing_segment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("000001.wav.tmp");
        std::fs::write(&path, b"existing").unwrap();
        assert!(WavSegment::create(&path, 1).is_err());
    }
}
