//! Bounded WAV/PCM reader. Times are counted in sample frames, not interleaved scalars.
use super::reader::open_regular;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const MAX_WAV_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_CHANNELS: u16 = 8;
pub const MIN_SAMPLE_RATE: u32 = 8_000;
pub const MAX_SAMPLE_RATE: u32 = 192_000;
pub const READ_FRAME_CHUNK: usize = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmEncoding {
    Int16,
    Int24,
    Int32,
    Float32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WavInfo {
    pub encoding: PcmEncoding,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub block_align: u16,
    pub data_offset: u64,
    pub data_bytes: u64,
    pub frame_count: u64,
}

impl WavInfo {
    pub fn frame_us(&self, frame_index: u64) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        (frame_index as u128 * 1_000_000 / self.sample_rate as u128) as u64
    }

    pub fn frames_for_us(&self, duration_us: u64) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        (duration_us as u128 * self.sample_rate as u128 / 1_000_000) as u64
    }
}

/// Peak and RMS of one channel over N sample frames. Never mix channels first.
pub fn channel_peak_rms(samples: &[f32]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f32;
    for &x in samples {
        let a = x.abs();
        if a > peak {
            peak = a;
        }
        sum_sq += x * x;
    }
    (peak, (sum_sq / samples.len() as f32).sqrt())
}

/// After per-channel measurement, take the loudest channel. Opposite-polarity
/// stereo stays energetic because samples are never averaged together.
pub fn max_energy(per_channel: &[(f32, f32)]) -> (f32, f32) {
    let mut peak = 0.0f32;
    let mut rms = 0.0f32;
    for &(p, r) in per_channel {
        if p > peak {
            peak = p;
        }
        if r > rms {
            rms = r;
        }
    }
    (peak, rms)
}

pub struct PcmReader {
    file: File,
    info: WavInfo,
    next_frame: u64,
}

impl PcmReader {
    pub fn open(path: &Path) -> Result<Self, String> {
        let info = parse_wav(path)?;
        let file = open_regular(path)?;
        Ok(Self {
            file,
            info,
            next_frame: 0,
        })
    }

    pub fn info(&self) -> &WavInfo {
        &self.info
    }

    /// Reads up to `max_frames` interleaved f32 frames into `buf`.
    /// `buf.len()` must be at least `max_frames * channels`.
    pub fn read_frames(&mut self, buf: &mut [f32], max_frames: usize) -> Result<usize, String> {
        let channels = self.info.channels as usize;
        if max_frames == 0 {
            return Ok(0);
        }
        if buf.len() < max_frames * channels {
            return Err("PCM read buffer is too small".into());
        }
        let remaining = self.info.frame_count.saturating_sub(self.next_frame) as usize;
        let frames = max_frames.min(remaining);
        if frames == 0 {
            return Ok(0);
        }
        let byte_len = frames * self.info.block_align as usize;
        let offset = self.info.data_offset + self.next_frame * self.info.block_align as u64;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut bytes = vec![0u8; byte_len];
        self.file
            .read_exact(&mut bytes)
            .map_err(|e| e.to_string())?;
        decode_frames(&bytes, &self.info, &mut buf[..frames * channels])?;
        self.next_frame += frames as u64;
        Ok(frames)
    }

    pub fn seek_to_frame(&mut self, frame: u64) -> Result<(), String> {
        if frame > self.info.frame_count {
            return Err("PCM seek is past the end of the file".into());
        }
        self.next_frame = frame;
        Ok(())
    }
}

pub fn parse_wav(path: &Path) -> Result<WavInfo, String> {
    let mut file = open_regular(path)?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if len > MAX_WAV_BYTES {
        return Err("WAV exceeds size limit".into());
    }
    if len < 12 {
        return Err("WAV header is too small".into());
    }
    let mut header = [0u8; 12];
    file.read_exact(&mut header).map_err(|e| e.to_string())?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("Not a RIFF/WAVE file".into());
    }

    let mut offset = 12u64;
    let mut fmt: Option<(PcmEncoding, u16, u32, u16, u16)> = None;
    let mut data_offset = None;
    let mut data_bytes = 0u64;
    for _ in 0..1_024 {
        if offset + 8 > len {
            break;
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut chunk_head = [0u8; 8];
        file.read_exact(&mut chunk_head)
            .map_err(|e| e.to_string())?;
        let chunk_id = &chunk_head[0..4];
        let chunk_len =
            u32::from_le_bytes([chunk_head[4], chunk_head[5], chunk_head[6], chunk_head[7]]) as u64;
        if offset.saturating_add(8).saturating_add(chunk_len) > len {
            return Err("WAV chunk exceeds file size".into());
        }
        if chunk_id == b"fmt " {
            fmt = Some(parse_fmt(&mut file, chunk_len)?);
        } else if chunk_id == b"data" && data_offset.is_none() {
            data_offset = Some(offset + 8);
            data_bytes = chunk_len;
        }
        offset = offset.saturating_add(8).saturating_add(chunk_len);
        if chunk_len % 2 == 1 {
            offset = offset.saturating_add(1);
        }
    }

    let (encoding, channels, sample_rate, bits_per_sample, block_align) =
        fmt.ok_or("WAV is missing a fmt chunk")?;
    let data_offset = data_offset.ok_or("WAV is missing a data chunk")?;
    if channels == 0 || channels > MAX_CHANNELS {
        return Err("Unsupported WAV channel count".into());
    }
    if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
        return Err("Unsupported WAV sample rate".into());
    }
    let expected_align = channels * ((bits_per_sample + 7) / 8);
    if block_align == 0 || block_align != expected_align {
        return Err("WAV block align does not match sample format".into());
    }
    let usable = data_bytes - (data_bytes % block_align as u64);
    if usable == 0 {
        return Err("WAV data chunk has no complete sample frames".into());
    }
    Ok(WavInfo {
        encoding,
        channels,
        sample_rate,
        bits_per_sample,
        block_align,
        data_offset,
        data_bytes: usable,
        frame_count: usable / block_align as u64,
    })
}

fn parse_fmt(file: &mut File, chunk_len: u64) -> Result<(PcmEncoding, u16, u32, u16, u16), String> {
    if chunk_len < 16 {
        return Err("WAV fmt chunk is too small".into());
    }
    let mut buf = vec![0u8; chunk_len.min(64) as usize];
    file.read_exact(&mut buf).map_err(|e| e.to_string())?;
    let mut format = u16::from_le_bytes([buf[0], buf[1]]);
    let channels = u16::from_le_bytes([buf[2], buf[3]]);
    let sample_rate = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let block_align = u16::from_le_bytes([buf[12], buf[13]]);
    let bits = u16::from_le_bytes([buf[14], buf[15]]);
    if format == 0xFFFE {
        if buf.len() < 40 {
            return Err("WAV extensible fmt chunk is too small".into());
        }
        let guid = &buf[24..40];
        format = match guid {
            [1, 0, 0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113] => 1,
            [3, 0, 0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113] => 3,
            _ => {
                return Err("Unsupported WAV extensible subformat".into());
            }
        };
    }
    let encoding = match (format, bits) {
        (1, 16) => PcmEncoding::Int16,
        (1, 24) => PcmEncoding::Int24,
        (1, 32) => PcmEncoding::Int32,
        (3, 32) => PcmEncoding::Float32,
        (1, _) | (3, _) => {
            return Err(format!("Unsupported PCM bit depth: {bits}"));
        }
        (other, _) => {
            return Err(format!("Unsupported WAV encoding: {other}"));
        }
    };
    Ok((encoding, channels, sample_rate, bits, block_align))
}

fn decode_frames(bytes: &[u8], info: &WavInfo, dest: &mut [f32]) -> Result<(), String> {
    let channels = info.channels as usize;
    let frames = dest.len() / channels;
    match info.encoding {
        PcmEncoding::Int16 => {
            for i in 0..frames * channels {
                let o = i * 2;
                let s = i16::from_le_bytes([bytes[o], bytes[o + 1]]);
                dest[i] = s as f32 / 32768.0;
            }
        }
        PcmEncoding::Int24 => {
            for i in 0..frames * channels {
                let o = i * 3;
                let mut v = i32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], 0]);
                if v & 0x800000 != 0 {
                    v |= !0xFFFFFF;
                }
                dest[i] = v as f32 / 8_388_608.0;
            }
        }
        PcmEncoding::Int32 => {
            for i in 0..frames * channels {
                let o = i * 4;
                let s = i32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
                dest[i] = s as f32 / 2_147_483_648.0;
            }
        }
        PcmEncoding::Float32 => {
            for i in 0..frames * channels {
                let o = i * 4;
                dest[i] = f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::generate_pcm16_wav;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn zero_and_constant_and_opposite_stereo_energy() {
        assert_eq!(channel_peak_rms(&[]), (0.0, 0.0));
        assert_eq!(channel_peak_rms(&[0.0, 0.0, 0.0]), (0.0, 0.0));
        let (p, r) = channel_peak_rms(&[0.5, 0.5, 0.5]);
        assert!((p - 0.5).abs() < 1e-6);
        assert!((r - 0.5).abs() < 1e-6);
        let left = channel_peak_rms(&[0.5, 0.5]);
        let right = channel_peak_rms(&[-0.5, -0.5]);
        let (p, r) = max_energy(&[left, right]);
        assert!((p - 0.5).abs() < 1e-6);
        assert!((r - 0.5).abs() < 1e-6);
        let mixed: Vec<f32> = (0..4)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let avg = mixed.iter().sum::<f32>() / mixed.len() as f32;
        assert!(avg.abs() < 1e-6, "signed mix would cancel; do not use it");
    }

    #[test]
    fn forty_eight_k_frame_index_is_one_second_for_any_channel_count() {
        for channels in [1u16, 2] {
            let info = WavInfo {
                encoding: PcmEncoding::Int16,
                channels,
                sample_rate: 48_000,
                bits_per_sample: 16,
                block_align: channels * 2,
                data_offset: 44,
                data_bytes: 48_000 * channels as u64 * 2,
                frame_count: 48_000,
            };
            assert_eq!(info.frame_us(48_000), 1_000_000);
        }
    }

    #[test]
    fn reads_constant_pcm_and_rejects_mulaw() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("c.wav");
        let samples = vec![16384i16; 480];
        std::fs::write(&path, generate_pcm16_wav(48_000, 1, &samples)).unwrap();
        let mut reader = PcmReader::open(&path).unwrap();
        assert_eq!(reader.info().frame_count, 480);
        assert_eq!(reader.info().sample_rate, 48_000);
        let mut buf = vec![0.0; 480];
        let n = reader.read_frames(&mut buf, 480).unwrap();
        assert_eq!(n, 480);
        assert!(buf.iter().all(|s| (s - 0.5).abs() < 1e-4));
        reader.seek_to_frame(10).unwrap();
        let mut one = vec![0.0; 1];
        assert_eq!(reader.read_frames(&mut one, 1).unwrap(), 1);
        assert!((one[0] - 0.5).abs() < 1e-4);

        let mut bad = generate_pcm16_wav(48_000, 1, &[0; 16]);
        bad[20] = 7;
        bad[21] = 0;
        let mulaw = dir.path().join("mu.wav");
        std::fs::File::create(&mulaw)
            .unwrap()
            .write_all(&bad)
            .unwrap();
        let err = parse_wav(&mulaw).unwrap_err();
        assert!(err.contains("Unsupported WAV encoding"));
    }
}
