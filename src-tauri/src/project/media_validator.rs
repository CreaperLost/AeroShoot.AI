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
    #[error("Invalid mdhd box (size {0}, version {1}): {2}")]
    InvalidMdhdBox(u64, u8, String),
    #[error("Invalid elst box (size {0}): {1}")]
    InvalidElstBox(u64, String),
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
    /// Native media-clock timescale parsed from the `mdhd` box (e.g. 90_000
    /// for AVFoundation H.264, 48_000 for AAC, 44_100 for a 44.1 kHz track).
    /// Zero when the container is not fMP4 or the `mdhd` box is absent.
    pub media_timescale: u32,
    /// Media-clock value at which the segment's first sample begins. Signed
    /// because `elst` edit lists can shift the presentation start. Combined
    /// with `media_timescale` this is what downstream rendering needs to
    /// re-anchor the segment onto a host timeline.
    pub media_start_value: i64,
    /// Host-clock anchor (epoch microseconds) at which the segment was
    /// committed. The Swift capture bridge stamps this via the segment
    /// callback; the Rust side persists it next to the segment's media
    /// start_value so the editor can re-derive the same mapping.
    pub host_anchor_us: i64,
}

/// Result of parsing a track's `mdhd` box (full box per ISO/IEC 14496-12 §8.4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MdhdInfo {
    pub version: u8,
    pub timescale: u32,
    pub duration_ticks: i64,
    /// ISO 639-2/T language code packed into a 15-bit value, or 0.
    pub language: u16,
}

impl MdhdInfo {
    /// Convert a media-clock value (in `timescale` ticks) to microseconds.
    /// Saturates to `u64::MAX` on overflow rather than panicking, since
    /// media durations can exceed what we want to render.
    pub fn ticks_to_us(&self, ticks: i64) -> u64 {
        if self.timescale == 0 {
            return 0;
        }
        // Cast to i128 first to avoid overflow on the multiply.
        let us = (ticks as i128).saturating_mul(1_000_000) / (self.timescale as i128);
        if us < 0 {
            0
        } else if us > u64::MAX as i128 {
            u64::MAX
        } else {
            us as u64
        }
    }
}

/// Result of parsing a track's `elst` edit list (full box per ISO/IEC 14496-12 §8.6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElstInfo {
    pub version: u8,
    /// Media time of the first non-empty edit entry, in media-clock ticks.
    /// `None` when the first entry is an empty edit (`media_time == -1`).
    pub first_media_time: Option<i64>,
    /// Segment duration of the first non-empty edit entry, in **movie**
    /// timescale ticks (ISO/IEC 14496-12). Must not be converted with `mdhd`.
    pub first_segment_duration: i64,
}

pub struct MediaValidator;

impl MediaValidator {
    /// Validates actual media structure on disk for a given track type.
    /// Rejects zero-filled dummy files, corrupted box sizes, header-only stubs,
    /// fragments lacking samples or media data, and non-keyframe segment starts.
    /// Recovers real presentation timing (start_us, end_us, duration_us) and
    /// the native media-clock timescale so the editor can re-establish the
    /// host-clock mapping on replay.
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
            TrackType::Screen | TrackType::Webcam => Self::validate_fmp4(&mut file, len),
            TrackType::SystemAudio | TrackType::MicAudio => Self::validate_wav(&mut file, len),
        }
    }

    fn validate_fmp4(
        file: &mut File,
        len: u64,
    ) -> Result<MediaValidationInfo, MediaValidationError> {
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
        let mut moofs: Vec<(u64, u64)> = Vec::new();
        let mut mdat_info: Option<(u64, u64)> = None;

        // AVAssetWriter also emits mfra/sidx after finishWriting. Rejecting them
        // used to fail a valid 2s rotation and leave only the first fragment.
        const VALID_BOXES: &[&[u8; 4]] = &[
            b"ftyp", b"moov", b"moof", b"mdat", b"free", b"styp", b"skip", b"wide", b"pdin",
            b"uuid", b"mfra", b"sidx", b"ssix", b"meta",
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

            let box_size_raw = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
            let box_type = [header[4], header[5], header[6], header[7]];

            if !VALID_BOXES.iter().any(|&b| b == &box_type) {
                return Err(MediaValidationError::InvalidMp4Box(
                    String::from_utf8_lossy(&box_type).into_owned(),
                ));
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
                b"moof" => moofs.push((offset, box_size)),
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
        let (_mdat_offset, mdat_size) = mdat_info.ok_or_else(|| {
            MediaValidationError::MissingRequiredBoxes("Missing mdat media data".into())
        })?;

        if mdat_size <= 8 {
            return Err(MediaValidationError::EmptyMediaFragment(0));
        }

        // Read the moov box so we can extract mdhd timescale + elst offset.
        // We need the moov bytes specifically, not moof. Find the moov location.
        // Reset and walk to the moov box.
        file.seek(SeekFrom::Start(0))
            .map_err(|e| MediaValidationError::Io(e.to_string()))?;
        let mut moov_offset: Option<u64> = None;
        let mut moov_size_v: u64 = 0;
        let mut off = 0u64;
        while off < len {
            if off + 8 > len {
                break;
            }
            let mut head = [0u8; 8];
            file.seek(SeekFrom::Start(off))
                .map_err(|e| MediaValidationError::Io(e.to_string()))?;
            file.read_exact(&mut head)
                .map_err(|e| MediaValidationError::Io(e.to_string()))?;
            let raw = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
            let typ = [head[4], head[5], head[6], head[7]];
            let (sz, hlen) = if raw == 1 {
                let mut lb = [0u8; 8];
                file.read_exact(&mut lb)
                    .map_err(|e| MediaValidationError::Io(e.to_string()))?;
                (u64::from_be_bytes(lb), 16u64)
            } else if raw == 0 {
                (len - off, 8u64)
            } else {
                (raw as u64, 8u64)
            };
            if typ == *b"moov" {
                moov_offset = Some(off);
                moov_size_v = sz;
                break;
            }
            off += sz.max(hlen);
        }
        let moov_offset = moov_offset
            .ok_or_else(|| MediaValidationError::MissingRequiredBoxes("Missing moov".into()))?;
        file.seek(SeekFrom::Start(moov_offset))
            .map_err(|e| MediaValidationError::Io(e.to_string()))?;
        let mut moov_bytes = vec![0u8; moov_size_v as usize];
        file.read_exact(&mut moov_bytes)
            .map_err(|e| MediaValidationError::Io(e.to_string()))?;

        let (mdhd, elst) = Self::parse_moov(&moov_bytes)
            .map_err(|e| MediaValidationError::InvalidMdhdBox(moov_bytes.len() as u64, 0, e))?;

        // AVAssetWriter finalizes recordings as ordinary MP4 (stbl) even when
        // movieFragmentInterval left leftover moof boxes. Prefer the sample
        // table whenever it actually contains samples.
        if let Some(mdat) = mdat_info {
            if let Ok((ticks, count)) = regular_sample_table(&moov_bytes, mdat) {
                let duration_ticks = if mdhd.duration_ticks > 0 {
                    (mdhd.duration_ticks as u64).max(ticks)
                } else {
                    ticks
                };
                let duration_us = mdhd.ticks_to_us(duration_ticks as i64);
                return Ok(MediaValidationInfo {
                    container_format: "mp4".into(),
                    size_bytes: len,
                    start_us: 0,
                    end_us: duration_us,
                    duration_us,
                    sample_count: count,
                    is_keyframe_start: true,
                    media_timescale: mdhd.timescale,
                    media_start_value: 0,
                    host_anchor_us: 0,
                });
            }
        }

        if moofs.is_empty() {
            return Err(MediaValidationError::MissingRequiredBoxes(
                "No sample table or media fragment".into(),
            ));
        }

        let mut total_duration_ticks: i64 = 0;
        let mut total_samples: u64 = 0;
        let mut media_start_ticks: i64 = 0;
        let mut is_keyframe = false;
        for (index, &(moof_offset, moof_size)) in moofs.iter().enumerate() {
            file.seek(SeekFrom::Start(moof_offset))
                .map_err(|e| MediaValidationError::Io(e.to_string()))?;
            let mut moof_bytes = vec![0u8; moof_size as usize];
            file.read_exact(&mut moof_bytes)
                .map_err(|e| MediaValidationError::Io(e.to_string()))?;
            let (start, sample_duration_ticks, sample_count, keyframe) =
                Self::parse_moof_fragment(&moof_bytes)?;
            if sample_count == 0 {
                return Err(MediaValidationError::EmptyMediaFragment(0));
            }
            if index == 0 {
                media_start_ticks = start;
                is_keyframe = keyframe;
            }
            total_duration_ticks = total_duration_ticks
                .saturating_add((sample_duration_ticks as i64).saturating_mul(sample_count as i64));
            total_samples = total_samples.saturating_add(sample_count);
        }

        if !is_keyframe {
            return Err(MediaValidationError::NotKeyframeStart);
        }

        // elst.media_time is in the media timescale; elst.segment_duration is
        // in the *movie* timescale and must not be converted with mdhd.
        let effective_start_ticks: i64 = match elst.and_then(|e| e.first_media_time) {
            Some(media_time) if media_time >= 0 => media_time,
            _ => media_start_ticks,
        };
        let duration_ticks = if mdhd.duration_ticks > total_duration_ticks {
            mdhd.duration_ticks
        } else {
            total_duration_ticks
        };
        let start_us = mdhd.ticks_to_us(effective_start_ticks);
        let duration_us = mdhd.ticks_to_us(duration_ticks);
        let end_us = start_us.saturating_add(duration_us);

        Ok(MediaValidationInfo {
            container_format: "mp4".into(),
            size_bytes: len,
            start_us,
            end_us,
            duration_us,
            sample_count: total_samples,
            is_keyframe_start: is_keyframe,
            media_timescale: mdhd.timescale,
            media_start_value: effective_start_ticks,
            host_anchor_us: 0, // Populated by the segment-callback handler.
        })
    }

    /// Parse the `moov` box, walking `trak → mdia → mdhd` to extract the
    /// track timescale + duration, and `trak → edts → elst` for the edit
    /// list offset. Returns `(MdhdInfo, Option<ElstInfo>)`.
    pub fn parse_moov(moov_bytes: &[u8]) -> Result<(MdhdInfo, Option<ElstInfo>), String> {
        let len = moov_bytes.len();
        if len < 8 {
            return Err("moov too small".into());
        }
        let mut idx = 8; // skip moof header bytes
        let mut mdhd_result: Option<MdhdInfo> = None;
        let mut elst_result: Option<ElstInfo> = None;

        // Walk moov children
        while idx + 8 <= len {
            let (child_size, child_type, child_payload_start) =
                read_box_header(moov_bytes, idx, len)?;
            if child_type == *b"trak" {
                // Recurse into trak to find mdia (for mdhd) and edts (for elst).
                let trak_end = idx + child_size;
                let mut trak_idx = child_payload_start;
                while trak_idx + 8 <= trak_end {
                    let (sub_size, sub_type, sub_start) =
                        read_box_header(moov_bytes, trak_idx, trak_end)?;
                    if sub_type == *b"mdia" {
                        let mdia_end = trak_idx + sub_size;
                        let mut mdia_idx = sub_start;
                        while mdia_idx + 8 <= mdia_end {
                            let (mdhd_box_size, mdhd_box_type, mdhd_box_start) =
                                read_box_header(moov_bytes, mdia_idx, mdia_end)?;
                            if mdhd_box_type == *b"mdhd" && mdhd_result.is_none() {
                                mdhd_result = Some(Self::parse_mdhd(
                                    &moov_bytes[mdhd_box_start..mdhd_box_start + mdhd_box_size],
                                )?);
                            }
                            mdia_idx += mdhd_box_size;
                        }
                    } else if sub_type == *b"edts" && elst_result.is_none() {
                        let edts_end = trak_idx + sub_size;
                        let mut edts_idx = sub_start;
                        while edts_idx + 8 <= edts_end {
                            let (elst_box_size, elst_box_type, elst_box_start) =
                                read_box_header(moov_bytes, edts_idx, edts_end)?;
                            if elst_box_type == *b"elst" {
                                elst_result = Some(Self::parse_elst(
                                    &moov_bytes[elst_box_start..elst_box_start + elst_box_size],
                                )?);
                            }
                            edts_idx += elst_box_size;
                        }
                    }
                    trak_idx += sub_size;
                }
            }
            idx += child_size;
        }

        let mdhd = mdhd_result.ok_or_else(|| "missing mdhd".to_string())?;
        Ok((mdhd, elst_result))
    }

    /// Parse a `mdhd` full box (version 0 or 1) per ISO/IEC 14496-12 §8.4.2.
    pub fn parse_mdhd(payload: &[u8]) -> Result<MdhdInfo, String> {
        if payload.len() < 4 {
            return Err("mdhd payload too small".into());
        }
        let version = payload[0];
        // `language` is a 15-bit unsigned int packed into 2 bytes
        // (top bit is a pad bit reserved by the spec and must be
        // ignored). Use a `u16` to read both bytes at once and mask.
        let read_language =
            |lo: usize| -> u16 { u16::from_be_bytes([payload[lo], payload[lo + 1]]) & 0x7FFF };
        if version == 0 {
            // v0: 4 (header) + 4 (creation) + 4 (modification)
            //   + 4 (timescale) + 4 (duration) + 2 (language)
            //   + 2 (pre_defined) = 24 bytes.
            if payload.len() < 24 {
                return Err("mdhd v0 too small".into());
            }
            let _creation = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let _modification =
                u32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]);
            let timescale =
                u32::from_be_bytes([payload[12], payload[13], payload[14], payload[15]]);
            let duration = u32::from_be_bytes([payload[16], payload[17], payload[18], payload[19]]);
            let language = read_language(20);
            Ok(MdhdInfo {
                version,
                timescale,
                duration_ticks: duration as i64,
                language,
            })
        } else if version == 1 {
            // v1: 4 (header) + 8 (creation) + 8 (modification)
            //   + 4 (timescale) + 8 (duration) + 2 (language)
            //   + 2 (pre_defined) = 36 bytes.
            if payload.len() < 36 {
                return Err("mdhd v1 too small".into());
            }
            let _creation = u64::from_be_bytes([
                payload[4],
                payload[5],
                payload[6],
                payload[7],
                payload[8],
                payload[9],
                payload[10],
                payload[11],
            ]);
            let _modification = u64::from_be_bytes([
                payload[12],
                payload[13],
                payload[14],
                payload[15],
                payload[16],
                payload[17],
                payload[18],
                payload[19],
            ]);
            let timescale =
                u32::from_be_bytes([payload[20], payload[21], payload[22], payload[23]]);
            let duration = u64::from_be_bytes([
                payload[24],
                payload[25],
                payload[26],
                payload[27],
                payload[28],
                payload[29],
                payload[30],
                payload[31],
            ]);
            let language = read_language(32);
            Ok(MdhdInfo {
                version,
                timescale,
                duration_ticks: duration as i64,
                language,
            })
        } else {
            Err(format!("unsupported mdhd version {}", version))
        }
    }

    /// Parse an `elst` full box (version 0 or 1) per ISO/IEC 14496-12 §8.6.6.
    /// Returns the first non-empty entry's `media_time` and `segment_duration`.
    pub fn parse_elst(payload: &[u8]) -> Result<ElstInfo, String> {
        if payload.len() < 4 {
            return Err("elst payload too small".into());
        }
        let version = payload[0];
        let entry_count = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
        if entry_count == 0 {
            return Ok(ElstInfo {
                version,
                first_media_time: None,
                first_segment_duration: 0,
            });
        }
        let entry_size = if version == 1 { 20 } else { 12 };
        let first_entry_offset = 8usize;
        let first_entry_end = first_entry_offset + entry_size;
        if payload.len() < first_entry_end {
            return Err("elst entry truncated".into());
        }
        let entry = &payload[first_entry_offset..first_entry_end];
        let (segment_duration, media_time) = if version == 1 {
            let dur = u64::from_be_bytes([
                entry[0], entry[1], entry[2], entry[3], entry[4], entry[5], entry[6], entry[7],
            ]) as i64;
            // ISO/IEC 14496-12 uses a 64-bit signed integer for media_time.
            let mt_raw = u64::from_be_bytes([
                entry[8], entry[9], entry[10], entry[11], entry[12], entry[13], entry[14],
                entry[15],
            ]);
            let media_time = i64::from_be_bytes(mt_raw.to_be_bytes());
            (dur, media_time)
        } else {
            let dur = u32::from_be_bytes([entry[0], entry[1], entry[2], entry[3]]) as i64;
            let mt_raw = u32::from_be_bytes([entry[4], entry[5], entry[6], entry[7]]);
            // media_time in v0 is signed 32-bit: -1 means empty edit.
            let media_time = i32::from_be_bytes(mt_raw.to_be_bytes()) as i64;
            (dur, media_time)
        };
        let first_media_time = if media_time == -1 {
            None
        } else {
            Some(media_time)
        };
        Ok(ElstInfo {
            version,
            first_media_time,
            first_segment_duration: segment_duration,
        })
    }

    fn parse_moof_fragment(
        moof_bytes: &[u8],
    ) -> Result<(i64, u32, u64, bool), MediaValidationError> {
        let mut idx = 8; // skip moof header
        let len = moof_bytes.len();

        let mut media_start_ticks: i64 = 0;
        let mut sample_duration_ticks: u32 = 0;
        let mut sample_count: u64 = 0;
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
                        media_start_ticks = if version == 1 && sub_size >= 20 {
                            let raw = u64::from_be_bytes([
                                moof_bytes[traf_idx + 12],
                                moof_bytes[traf_idx + 13],
                                moof_bytes[traf_idx + 14],
                                moof_bytes[traf_idx + 15],
                                moof_bytes[traf_idx + 16],
                                moof_bytes[traf_idx + 17],
                                moof_bytes[traf_idx + 18],
                                moof_bytes[traf_idx + 19],
                            ]);
                            i64::from_be_bytes(raw.to_be_bytes())
                        } else {
                            let raw = u32::from_be_bytes([
                                moof_bytes[traf_idx + 12],
                                moof_bytes[traf_idx + 13],
                                moof_bytes[traf_idx + 14],
                                moof_bytes[traf_idx + 15],
                            ]);
                            i32::from_be_bytes(raw.to_be_bytes()) as i64
                        };
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
                            sample_duration_ticks = u32::from_be_bytes([
                                moof_bytes[trun_offset],
                                moof_bytes[trun_offset + 1],
                                moof_bytes[trun_offset + 2],
                                moof_bytes[trun_offset + 3],
                            ]);
                        } else {
                            sample_duration_ticks = 3_000; // default ~1/30s at 90kHz
                        }
                    }

                    traf_idx += sub_size;
                }
            }

            idx += box_size;
        }

        Ok((
            media_start_ticks,
            sample_duration_ticks,
            sample_count,
            is_keyframe,
        ))
    }

    fn validate_wav(
        file: &mut File,
        len: u64,
    ) -> Result<MediaValidationInfo, MediaValidationError> {
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
                sample_rate = u32::from_le_bytes([fmt_buf[4], fmt_buf[5], fmt_buf[6], fmt_buf[7]]);
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
        let duration_us = (total_samples as u128 * 1_000_000 / sample_rate as u128) as u64;

        Ok(MediaValidationInfo {
            container_format: "wav".into(),
            size_bytes: len,
            start_us: 0,
            end_us: duration_us,
            duration_us,
            sample_count: total_samples,
            is_keyframe_start: true,
            media_timescale: sample_rate,
            media_start_value: 0,
            host_anchor_us: 0,
        })
    }
}

/// Read a top-level MP4 box header at `offset` and return `(size, type, payload_start)`.
/// `end` is the upper bound on the parent's payload so a malformed child
/// cannot cause an out-of-bounds slice.
fn read_box_header(
    bytes: &[u8],
    offset: usize,
    end: usize,
) -> Result<(usize, [u8; 4], usize), String> {
    if offset + 8 > end {
        return Err("box header out of bounds".into());
    }
    let raw = u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]);
    let box_type = [
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ];
    let (box_size, header_len) = if raw == 1 {
        if offset + 16 > end {
            return Err("extended box header out of bounds".into());
        }
        let mut large_buf = [0u8; 8];
        large_buf.copy_from_slice(&bytes[offset + 8..offset + 16]);
        (u64::from_be_bytes(large_buf) as usize, 16usize)
    } else if raw == 0 {
        (end - offset, 8usize)
    } else {
        (raw as usize, 8usize)
    };
    if box_size < header_len || offset + box_size > end {
        return Err(format!(
            "invalid box size {} at offset {}",
            box_size, offset
        ));
    }
    Ok((box_size, box_type, offset + header_len))
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
        // The fixture's mdhd timescale is 90_000; the validator must
        // respect that instead of the old hard-coded 90_000.
        assert_eq!(info.media_timescale, 90_000);
        assert_eq!(info.media_start_value, 0);
    }

    #[test]
    fn test_accepts_mfra_and_sums_fragment_sample_durations() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("000001.mp4");
        let mut bytes = generate_valid_fmp4_segment(0, 2_000_000, true);
        let moof_at = bytes
            .windows(4)
            .position(|w| w == b"moof")
            .map(|i| i - 4)
            .expect("fixture moof");
        let fragments = bytes[moof_at..].to_vec();
        bytes.extend_from_slice(&fragments);
        bytes.extend_from_slice(&8u32.to_be_bytes());
        bytes.extend_from_slice(b"mfra");
        std::fs::write(&file_path, &bytes).unwrap();

        let info = MediaValidator::validate(&file_path, TrackType::Screen).unwrap();
        assert_eq!(info.sample_count, 2);
        assert_eq!(info.duration_us, 4_000_000);
        assert_eq!(info.end_us, 4_000_000);
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
        assert_eq!(info.media_timescale, 48_000);
    }

    #[test]
    fn test_parse_mdhd_v0_timescale() {
        // Build a hand-crafted mdhd v0 payload (24 bytes total):
        //   version(1) + flags(3) + creation(4) + modification(4)
        //   + timescale(4) + duration(4) + language(2) + pre_defined(2)
        // The language is a 15-bit unsigned int packed into 2 bytes
        // (top bit is a pad bit and must be ignored).
        let mut payload = Vec::new();
        payload.push(0); // version
        payload.extend_from_slice(&[0, 0, 0]); // flags
        payload.extend_from_slice(&0u32.to_be_bytes()); // creation
        payload.extend_from_slice(&0u32.to_be_bytes()); // modification
        payload.extend_from_slice(&48_000u32.to_be_bytes()); // timescale
        payload.extend_from_slice(&96_000u32.to_be_bytes()); // duration
                                                             // language = 0x55C4 (English) packed as 2 big-endian bytes.
        payload.extend_from_slice(&0x55C4u16.to_be_bytes());
        payload.extend_from_slice(&0u16.to_be_bytes()); // pre_defined

        let info = MediaValidator::parse_mdhd(&payload).unwrap();
        assert_eq!(info.version, 0);
        assert_eq!(info.timescale, 48_000);
        assert_eq!(info.duration_ticks, 96_000);
        // language bits: 0x55C4 & 0x7FFF = 0x55C4 (top bit was already 0).
        assert_eq!(info.language, 0x55C4);

        // ticks_to_us: 96_000 ticks at 48_000 Hz = 2_000_000 us.
        assert_eq!(info.ticks_to_us(96_000), 2_000_000);
    }

    #[test]
    fn test_parse_mdhd_v1_timescale() {
        // Hand-crafted mdhd v1 payload (36 bytes total):
        //   version(1) + flags(3) + creation(8) + modification(8)
        //   + timescale(4) + duration(8) + language(2) + pre_defined(2)
        let mut payload = Vec::new();
        payload.push(1); // version = 1
        payload.extend_from_slice(&[0, 0, 0]); // flags
        payload.extend_from_slice(&0u64.to_be_bytes()); // creation
        payload.extend_from_slice(&0u64.to_be_bytes()); // modification
        payload.extend_from_slice(&90_000u32.to_be_bytes()); // timescale
        payload.extend_from_slice(&180_000u64.to_be_bytes()); // duration
        payload.extend_from_slice(&0x55C4u16.to_be_bytes()); // language
        payload.extend_from_slice(&0u16.to_be_bytes()); // pre_defined

        let info = MediaValidator::parse_mdhd(&payload).unwrap();
        assert_eq!(info.version, 1);
        assert_eq!(info.timescale, 90_000);
        assert_eq!(info.duration_ticks, 180_000);
        assert_eq!(info.language, 0x55C4);

        // ticks_to_us: 180_000 at 90_000 Hz = 2_000_000 us.
        assert_eq!(info.ticks_to_us(180_000), 2_000_000);
    }

    #[test]
    fn test_parse_elst_v0_with_empty_edit() {
        // elst v0 with one entry whose media_time is -1 (empty edit).
        // Entry layout: segment_duration(4) + media_time(4) + media_rate(4) = 12 bytes.
        let mut payload = Vec::new();
        payload.push(0); // version
        payload.extend_from_slice(&[0, 0, 0]); // flags
        payload.extend_from_slice(&1u32.to_be_bytes()); // entry_count = 1
        payload.extend_from_slice(&2_000u32.to_be_bytes()); // segment_duration
        payload.extend_from_slice(&(-1i32).to_be_bytes()); // media_time = -1 (empty edit)
        payload.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // media_rate = 1.0 (16.16)

        let info = MediaValidator::parse_elst(&payload).unwrap();
        assert_eq!(info.version, 0);
        assert_eq!(info.first_media_time, None);
        assert_eq!(info.first_segment_duration, 2_000);
    }

    #[test]
    fn test_parse_elst_v1_with_real_media_time() {
        // elst v1 entry layout: segment_duration(8) + media_time(8) + media_rate(4) = 20 bytes.
        let mut payload = Vec::new();
        payload.push(1); // version = 1
        payload.extend_from_slice(&[0, 0, 0]); // flags
        payload.extend_from_slice(&1u32.to_be_bytes()); // entry_count = 1
        payload.extend_from_slice(&5_000u64.to_be_bytes()); // segment_duration
        payload.extend_from_slice(&900i64.to_be_bytes()); // media_time
        payload.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // media_rate = 1.0

        let info = MediaValidator::parse_elst(&payload).unwrap();
        assert_eq!(info.version, 1);
        assert_eq!(info.first_media_time, Some(900));
        assert_eq!(info.first_segment_duration, 5_000);
    }

    #[test]
    fn test_mdhd_ticks_to_us_handles_overflow_and_zero() {
        let info = MdhdInfo {
            version: 0,
            timescale: 0,
            duration_ticks: 0,
            language: 0,
        };
        assert_eq!(info.ticks_to_us(1_000), 0, "zero timescale must not divide");

        let info = MdhdInfo {
            version: 0,
            timescale: 48_000,
            duration_ticks: 0,
            language: 0,
        };
        assert_eq!(info.ticks_to_us(48_000), 1_000_000);
        assert_eq!(info.ticks_to_us(-48_000), 0, "negative ticks clamp to 0");
    }
}

/// Validate sample counts, durations, sync start and every chunk's byte extent.
fn regular_sample_table(moov: &[u8], mdat: (u64, u64)) -> Result<(u64, u64), String> {
    fn word(bytes: &[u8], at: usize) -> Result<u32, String> {
        let value = bytes.get(at..at + 4).ok_or("Truncated sample table")?;
        Ok(u32::from_be_bytes(value.try_into().unwrap()))
    }
    fn children(bytes: &[u8]) -> Result<Vec<(&[u8], &[u8])>, String> {
        let mut result = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let size = word(bytes, at)? as usize;
            if size < 8 || size > bytes.len() - at {
                return Err("Invalid sample-table box".into());
            }
            result.push((&bytes[at + 4..at + 8], &bytes[at + 8..at + size]));
            at += size;
        }
        Ok(result)
    }
    let mut level = moov.get(8..).ok_or("Truncated moov")?;
    for name in [b"trak", b"mdia", b"minf", b"stbl"] {
        level = children(level)?
            .into_iter()
            .find(|(kind, _)| *kind == name)
            .ok_or("Missing video sample table")?
            .1;
    }
    let tables = children(level)?;
    let table = |name: &[u8]| -> Result<&[u8], String> {
        tables
            .iter()
            .find(|(kind, _)| *kind == name)
            .map(|(_, data)| *data)
            .ok_or("Missing sample table".into())
    };
    let sizes = table(b"stsz")?;
    let count = word(sizes, 8)? as usize;
    if count == 0 || count > 1_000_000 {
        return Err("Invalid sample count".into());
    }
    let fixed = word(sizes, 4)? as u64;
    let mut sample_sizes = Vec::with_capacity(count);
    for i in 0..count {
        let size = if fixed > 0 {
            fixed
        } else {
            word(sizes, 12 + i * 4)? as u64
        };
        if size == 0 {
            return Err("Empty sample".into());
        }
        sample_sizes.push(size);
    }
    let timing = table(b"stts")?;
    let entries = word(timing, 4)? as usize;
    if entries > count {
        return Err("Invalid timing table".into());
    }
    let mut timed = 0u64;
    let mut ticks = 0u64;
    for i in 0..entries {
        let n = word(timing, 8 + i * 8)? as u64;
        let duration = word(timing, 12 + i * 8)? as u64;
        if duration == 0 {
            return Err("Zero sample duration".into());
        }
        timed += n;
        ticks = ticks.checked_add(n * duration).ok_or("Duration overflow")?;
    }
    if timed != count as u64 || ticks > i64::MAX as u64 {
        return Err("Inconsistent sample timing".into());
    }
    if let Ok(sync) = table(b"stss") {
        if word(sync, 4)? == 0 || word(sync, 8)? != 1 {
            return Err("First sample is not a keyframe".into());
        }
    }
    let (offsets, wide) = match table(b"stco") {
        Ok(v) => (v, false),
        Err(_) => (table(b"co64")?, true),
    };
    let chunks = word(offsets, 4)? as usize;
    if chunks == 0 || chunks > count {
        return Err("Invalid chunk count".into());
    }
    let mapping = table(b"stsc")?;
    let runs = word(mapping, 4)? as usize;
    if runs == 0 || runs > chunks || word(mapping, 8)? != 1 {
        return Err("Invalid sample-to-chunk mapping".into());
    }
    let mut rows = Vec::with_capacity(runs);
    for i in 0..runs {
        let first = word(mapping, 8 + i * 12)? as usize;
        let per_chunk = word(mapping, 12 + i * 12)? as usize;
        if first == 0
            || first > chunks
            || per_chunk == 0
            || word(mapping, 16 + i * 12)? == 0
            || rows.last().is_some_and(|&(prev, _)| first <= prev)
        {
            return Err("Invalid chunk run".into());
        }
        rows.push((first, per_chunk));
    }
    let mut consumed = 0usize;
    let mut run = 0;
    for chunk in 1..=chunks {
        while run + 1 < rows.len() && rows[run + 1].0 <= chunk {
            run += 1;
        }
        let end = consumed.checked_add(rows[run].1).ok_or("Sample overflow")?;
        let bytes: u64 = sample_sizes
            .get(consumed..end)
            .ok_or("Too many chunk samples")?
            .iter()
            .sum();
        let at = 8 + (chunk - 1) * if wide { 8 } else { 4 };
        let offset = if wide {
            ((word(offsets, at)? as u64) << 32) | word(offsets, at + 4)? as u64
        } else {
            word(offsets, at)? as u64
        };
        if offset < mdat.0 + 8
            || offset
                .checked_add(bytes)
                .is_none_or(|end| end > mdat.0 + mdat.1)
        {
            return Err("Chunk exceeds media payload".into());
        }
        consumed = end;
    }
    if consumed != count {
        return Err("Missing chunk samples".into());
    }
    Ok((ticks, count as u64))
}
