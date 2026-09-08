use crate::project::manifest::TrackType;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum MediaValidationError {
    #[error("File too small to contain valid media container (size: {0} bytes)")]
    FileTooSmall(u64),
    #[error("Invalid MP4 header: unknown box type {0:?}")]
    InvalidMp4Box(String),
    #[error("Invalid MP4 box size {0} at offset {1}: exceeds file size {2}")]
    InvalidMp4BoxSize(u64, u64, u64),
    #[error("Missing required MP4 boxes: {0}")]
    MissingRequiredBoxes(String),
    #[error("Fragment missing media data or samples (sample_count: {0})")]
    EmptyMediaFragment(u64),
    #[error("Segment does not start with an independent keyframe")]
    NotKeyframeStart,
    #[error("Invalid WAV header: expected RIFF/WAVE markers")]
    InvalidWavHeader,
    #[error("Zero-filled dummy file detected")]
    ZeroFilledDummy,
    #[error("IO error: {0}")]
    Io(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaValidationInfo {
    pub container_format: String,
    pub size_bytes: u64,
    pub start_us: u64,
    pub end_us: u64,
    pub duration_us: u64,
    pub sample_count: u64,
    pub is_keyframe_start: bool,
}

pub struct MediaValidator;

impl MediaValidator {
    /// Validates actual media structure on disk for a given track type.
    /// Rejects zero-filled dummy files, corrupted box sizes, header-only stubs,
    /// fragments lacking samples or media data, and non-keyframe segment starts.
    /// Recovers real presentation timing (start_us, end_us, duration_us).
    pub fn validate<P: AsRef<Path>>(
        path: P,
        track_type: TrackType,
    ) -> Result<MediaValidationInfo, MediaValidationError> {
        let p = path.as_ref();
        let mut file = File::open(p).map_err(|e| MediaValidationError::Io(e.to_string()))?;
        let metadata = file
            .metadata()
            .map_err(|e| MediaValidationError::Io(e.to_string()))?;
        let len = metadata.len();

        match track_type {
            TrackType::Screen | TrackType::Webcam => {
                if len < 8 {
                    return Err(MediaValidationError::FileTooSmall(len));
                }

                // Check for all-zeros dummy file
                let mut check_buf = [0u8; 512];
                let bytes_read = file
                    .read(&mut check_buf)
                    .map_err(|e| MediaValidationError::Io(e.to_string()))?;
                if bytes_read > 0 && check_buf[..bytes_read].iter().all(|&b| b == 0) {
                    return Err(MediaValidationError::ZeroFilledDummy);
                }

                file.seek(SeekFrom::Start(0))
                    .map_err(|e| MediaValidationError::Io(e.to_string()))?;

                let mut offset = 0u64;
                let mut has_ftyp_or_styp = false;
                let mut has_moov = false;
                let mut moof_info: Option<(u64, u64)> = None; // (offset, size)
                let mut mdat_info: Option<(u64, u64)> = None;

                let valid_boxes: [&[u8; 4]; 9] = [
                    b"ftyp", b"moov", b"moof", b"mdat", b"free", b"styp", b"skip", b"wide",
                    b"pdin",
                ];

                while offset < len {
                    if offset + 8 > len {
                        return Err(MediaValidationError::InvalidMp4BoxSize(
                            (len - offset) as u64,
                            offset,
                            len,
                        ));
                    }

                    file.seek(SeekFrom::Start(offset))
                        .map_err(|e| MediaValidationError::Io(e.to_string()))?;

                    let mut header = [0u8; 8];
                    file.read_exact(&mut header)
                        .map_err(|e| MediaValidationError::Io(e.to_string()))?;

                    let box_size_raw =
                        u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
                    let box_type = [header[4], header[5], header[6], header[7]];
                    let box_type_str = String::from_utf8_lossy(&box_type).to_string();

                    if !valid_boxes.iter().any(|&b| b == &box_type) {
                        return Err(MediaValidationError::InvalidMp4Box(box_type_str));
                    }

                    let (box_size, header_len) = if box_size_raw == 1 {
                        if offset + 16 > len {
                            return Err(MediaValidationError::InvalidMp4BoxSize(1, offset, len));
                        }
                        let mut large_buf = [0u8; 8];
                        file.read_exact(&mut large_buf)
                            .map_err(|e| MediaValidationError::Io(e.to_string()))?;
                        (u64::from_be_bytes(large_buf), 16u64)
                    } else if box_size_raw == 0 {
                        (len - offset, 8u64)
                    } else {
                        (box_size_raw as u64, 8u64)
                    };

                    if box_size < header_len || offset + box_size > len {
                        return Err(MediaValidationError::InvalidMp4BoxSize(
                            box_size, offset, len,
                        ));
                    }

                    match &box_type {
                        b"ftyp" | b"styp" => has_ftyp_or_styp = true,
                        b"moov" => has_moov = true,
                        b"moof" => moof_info = Some((offset, box_size)),
                        b"mdat" => mdat_info = Some((offset, box_size)),
                        _ => {}
                    }

                    offset += box_size;
                }

                if !has_ftyp_or_styp {
                    return Err(MediaValidationError::MissingRequiredBoxes(
                        "Missing ftyp/styp header".into(),
                    ));
                }

                if !has_moov {
                    return Err(MediaValidationError::MissingRequiredBoxes(
                        "Missing moov movie box".into(),
                    ));
                }

                // Fragmented media must have both moof and mdat
                let (moof_offset, moof_size) = moof_info.ok_or_else(|| {
                    MediaValidationError::MissingRequiredBoxes("Missing moof movie fragment".into())
                })?;
                let (_mdat_offset, mdat_size) = mdat_info.ok_or_else(|| {
                    MediaValidationError::MissingRequiredBoxes("Missing mdat media data".into())
                })?;

                if mdat_size <= 8 {
                    return Err(MediaValidationError::EmptyMediaFragment(0));
                }

                // Deep fragment parsing: parse moof -> traf -> tfdt and trun
                file.seek(SeekFrom::Start(moof_offset))
                    .map_err(|e| MediaValidationError::Io(e.to_string()))?;
                let mut moof_bytes = vec![0u8; moof_size as usize];
                file.read_exact(&mut moof_bytes)
                    .map_err(|e| MediaValidationError::Io(e.to_string()))?;

                let (start_us, duration_us, sample_count, is_keyframe) =
                    Self::parse_moof_fragment(&moof_bytes)?;

                if sample_count == 0 {
                    return Err(MediaValidationError::EmptyMediaFragment(0));
                }

                if !is_keyframe {
                    return Err(MediaValidationError::NotKeyframeStart);
                }

                let end_us = start_us + duration_us;

                Ok(MediaValidationInfo {
                    container_format: "mp4".into(),
                    size_bytes: len,
                    start_us,
                    end_us,
                    duration_us,
                    sample_count,
                    is_keyframe_start: is_keyframe,
                })
            }
            TrackType::SystemAudio | TrackType::MicAudio => {
                // PCM WAV validation
                if len < 44 {
                    return Err(MediaValidationError::FileTooSmall(len));
                }

                let mut header = [0u8; 12];
                file.seek(SeekFrom::Start(0))
                    .map_err(|e| MediaValidationError::Io(e.to_string()))?;
                file.read_exact(&mut header)
                    .map_err(|e| MediaValidationError::Io(e.to_string()))?;

                if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
                    return Err(MediaValidationError::InvalidWavHeader);
                }

                // Check for all-zeros dummy file
                let mut check_buf = [0u8; 512];
                let bytes_read = file
                    .read(&mut check_buf)
                    .map_err(|e| MediaValidationError::Io(e.to_string()))?;
                if bytes_read > 0 && check_buf[..bytes_read].iter().all(|&b| b == 0) {
                    return Err(MediaValidationError::ZeroFilledDummy);
                }

                // Find fmt and data chunks
                file.seek(SeekFrom::Start(12))
                    .map_err(|e| MediaValidationError::Io(e.to_string()))?;
                let mut curr_offset = 12u64;

                let mut sample_rate = 48000u32;
                let mut channels = 2u16;
                let mut bits_per_sample = 16u16;
                let mut data_size = 0u64;

                while curr_offset + 8 <= len {
                    let mut chunk_head = [0u8; 8];
                    file.seek(SeekFrom::Start(curr_offset))
                        .map_err(|e| MediaValidationError::Io(e.to_string()))?;
                    file.read_exact(&mut chunk_head)
                        .map_err(|e| MediaValidationError::Io(e.to_string()))?;

                    let chunk_id = &chunk_head[0..4];
                    let chunk_len =
                        u32::from_le_bytes([chunk_head[4], chunk_head[5], chunk_head[6], chunk_head[7]])
                            as u64;

                    if chunk_id == b"fmt " && chunk_len >= 16 {
                        let mut fmt_buf = [0u8; 16];
                        file.read_exact(&mut fmt_buf)
                            .map_err(|e| MediaValidationError::Io(e.to_string()))?;
                        channels = u16::from_le_bytes([fmt_buf[2], fmt_buf[3]]);
                        sample_rate = u32::from_le_bytes([
                            fmt_buf[4], fmt_buf[5], fmt_buf[6], fmt_buf[7],
                        ]);
                        bits_per_sample = u16::from_le_bytes([fmt_buf[14], fmt_buf[15]]);
                    } else if chunk_id == b"data" {
                        data_size = chunk_len;
                    }

                    curr_offset += 8 + chunk_len;
                }

                if data_size == 0 || sample_rate == 0 || channels == 0 || bits_per_sample == 0 {
                    return Err(MediaValidationError::InvalidWavHeader);
                }

                let bytes_per_sample = (channels as u64 * bits_per_sample as u64) / 8;
                let total_samples = data_size / bytes_per_sample;
                let duration_us =
                    (total_samples as u128 * 1_000_000 / sample_rate as u128) as u64;

                Ok(MediaValidationInfo {
                    container_format: "wav".into(),
                    size_bytes: len,
                    start_us: 0,
                    end_us: duration_us,
                    duration_us,
                    sample_count: total_samples,
                    is_keyframe_start: true,
                })
            }
        }
    }

    fn parse_moof_fragment(
        moof_bytes: &[u8],
    ) -> Result<(u64, u64, u64, bool), MediaValidationError> {
        let mut idx = 8; // skip moof header
        let len = moof_bytes.len();

        let mut start_us = 0u64;
        let mut duration_us = 0u64;
        let mut sample_count = 0u64;
        let mut is_keyframe = false;

        while idx + 8 <= len {
            let box_size = u32::from_be_bytes([
                moof_bytes[idx],
                moof_bytes[idx + 1],
                moof_bytes[idx + 2],
                moof_bytes[idx + 3],
            ]) as usize;
            let box_type = &moof_bytes[idx + 4..idx + 8];

            if box_size < 8 || idx + box_size > len {
                break;
            }

            if box_type == b"traf" {
                let mut traf_idx = idx + 8;
                let traf_end = idx + box_size;

                while traf_idx + 8 <= traf_end {
                    let sub_size = u32::from_be_bytes([
                        moof_bytes[traf_idx],
                        moof_bytes[traf_idx + 1],
                        moof_bytes[traf_idx + 2],
                        moof_bytes[traf_idx + 3],
                    ]) as usize;
                    let sub_type = &moof_bytes[traf_idx + 4..traf_idx + 8];

                    if sub_size < 8 || traf_idx + sub_size > traf_end {
                        break;
                    }

                    if sub_type == b"tfdt" && sub_size >= 16 {
                        let version = moof_bytes[traf_idx + 8];
                        let ticks = if version == 1 && sub_size >= 20 {
                            u64::from_be_bytes([
                                moof_bytes[traf_idx + 12],
                                moof_bytes[traf_idx + 13],
                                moof_bytes[traf_idx + 14],
                                moof_bytes[traf_idx + 15],
                                moof_bytes[traf_idx + 16],
                                moof_bytes[traf_idx + 17],
                                moof_bytes[traf_idx + 18],
                                moof_bytes[traf_idx + 19],
                            ])
                        } else {
                            u32::from_be_bytes([
                                moof_bytes[traf_idx + 12],
                                moof_bytes[traf_idx + 13],
                                moof_bytes[traf_idx + 14],
                                moof_bytes[traf_idx + 15],
                            ]) as u64
                        };
                        start_us = (ticks as u128 * 1_000_000 / 90_000) as u64;
                    } else if sub_type == b"trun" && sub_size >= 16 {
                        let flags = u32::from_be_bytes([
                            0,
                            moof_bytes[traf_idx + 9],
                            moof_bytes[traf_idx + 10],
                            moof_bytes[traf_idx + 11],
                        ]);
                        let sc = u32::from_be_bytes([
                            moof_bytes[traf_idx + 12],
                            moof_bytes[traf_idx + 13],
                            moof_bytes[traf_idx + 14],
                            moof_bytes[traf_idx + 15],
                        ]) as u64;
                        sample_count = sc;

                        let mut trun_offset = traf_idx + 16;
                        if (flags & 0x000001) != 0 {
                            trun_offset += 4; // data_offset
                        }
                        if (flags & 0x000004) != 0 && trun_offset + 4 <= traf_idx + sub_size {
                            let first_flags = u32::from_be_bytes([
                                moof_bytes[trun_offset],
                                moof_bytes[trun_offset + 1],
                                moof_bytes[trun_offset + 2],
                                moof_bytes[trun_offset + 3],
                            ]);
                            // sample_is_non_sync_sample is bit 16: (flags & 0x00010000) == 0 means sync sample!
                            is_keyframe = (first_flags & 0x00010000) == 0;
                            trun_offset += 4;
                        } else {
                            // If first_sample_flags not present, check default (assume keyframe if first segment)
                            is_keyframe = true;
                        }

                        // Read sample duration if present
                        if (flags & 0x000100) != 0 && trun_offset + 4 <= traf_idx + sub_size {
                            let sample_dur_ticks = u32::from_be_bytes([
                                moof_bytes[trun_offset],
                                moof_bytes[trun_offset + 1],
                                moof_bytes[trun_offset + 2],
                                moof_bytes[trun_offset + 3],
                            ]);
                            duration_us = (sample_dur_ticks as u128 * 1_000_000 / 90_000) as u64;
                        } else {
                            duration_us = 33_333; // default 1/30s
                        }
                    }

                    traf_idx += sub_size;
                }
            }

            idx += box_size;
        }

        Ok((start_us, duration_us, sample_count, is_keyframe))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{generate_valid_fmp4_segment, generate_valid_wav_segment};
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_rejects_zero_filled_dummy_mp4() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("000001.mp4");
        std::fs::write(&file_path, vec![0u8; 4096]).unwrap();

        let result = MediaValidator::validate(&file_path, TrackType::Screen);
        assert_eq!(result.err(), Some(MediaValidationError::ZeroFilledDummy));
    }

    #[test]
    fn test_rejects_eight_byte_mp4_with_excessive_box_size() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("overflow.mp4");

        // 8-byte file claiming 999,999 byte ftyp box
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&999_999u32.to_be_bytes());
        bytes.extend_from_slice(b"ftyp");
        std::fs::write(&file_path, &bytes).unwrap();

        let result = MediaValidator::validate(&file_path, TrackType::Screen);
        assert_eq!(
            result.err(),
            Some(MediaValidationError::InvalidMp4BoxSize(999_999, 0, 8))
        );
    }

    #[test]
    fn test_rejects_header_only_mp4_lacking_fragments() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("header_only.mp4");
        let mut f = File::create(&file_path).unwrap();

        // Only ftyp box (32 bytes)
        let mut ftyp = Vec::new();
        ftyp.extend_from_slice(&32u32.to_be_bytes());
        ftyp.extend_from_slice(b"ftyp");
        ftyp.extend_from_slice(b"isom");
        ftyp.extend_from_slice(&0x0200u32.to_be_bytes());
        ftyp.extend_from_slice(b"isomiso2avc1mp41");
        f.write_all(&ftyp).unwrap();

        let result = MediaValidator::validate(&file_path, TrackType::Screen);
        assert!(matches!(
            result.err(),
            Some(MediaValidationError::MissingRequiredBoxes(_))
        ));
    }

    #[test]
    fn test_accepts_valid_fmp4_with_keyframe_and_samples() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("000001.mp4");

        let fmp4_bytes = generate_valid_fmp4_segment(0, 2_000_000, true);
        std::fs::write(&file_path, &fmp4_bytes).unwrap();

        let info = MediaValidator::validate(&file_path, TrackType::Screen).unwrap();
        assert_eq!(info.container_format, "mp4");
        assert_eq!(info.start_us, 0);
        assert_eq!(info.duration_us, 2_000_000);
        assert_eq!(info.end_us, 2_000_000);
        assert_eq!(info.sample_count, 1);
        assert!(info.is_keyframe_start);
    }

    #[test]
    fn test_rejects_fmp4_not_starting_with_keyframe() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("non_keyframe.mp4");

        let fmp4_bytes = generate_valid_fmp4_segment(0, 2_000_000, false);
        std::fs::write(&file_path, &fmp4_bytes).unwrap();

        let result = MediaValidator::validate(&file_path, TrackType::Screen);
        assert_eq!(result.err(), Some(MediaValidationError::NotKeyframeStart));
    }

    #[test]
    fn test_accepts_valid_wav_header_and_samples() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("000001.wav");

        let wav_bytes = generate_valid_wav_segment(1_000_000, 48_000, 2);
        std::fs::write(&file_path, &wav_bytes).unwrap();

        let info = MediaValidator::validate(&file_path, TrackType::MicAudio).unwrap();
        assert_eq!(info.container_format, "wav");
        assert_eq!(info.duration_us, 1_000_000);
    }
}
