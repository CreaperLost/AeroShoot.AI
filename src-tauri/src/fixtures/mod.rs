use crate::session::SessionEpoch;

/// Generates synthetic video and audio timestamps with customizable skew and clock drift
/// to test multi-track synchronization and recovery logic without hardware capture devices.
pub struct SyntheticCaptureStream {
    epoch: SessionEpoch,
    fps: u32,
    audio_sample_rate: u32,
    video_frame_count: u64,
    audio_sample_count: u64,
    video_clock_drift_ppm: f64, // Parts per million drift (e.g. 50 ppm)
}

impl SyntheticCaptureStream {
    pub fn new(fps: u32, audio_sample_rate: u32, video_clock_drift_ppm: f64) -> Self {
        Self {
            epoch: SessionEpoch::now(),
            fps,
            audio_sample_rate,
            video_frame_count: 0,
            audio_sample_count: 0,
            video_clock_drift_ppm,
        }
    }

    /// Generates next video frame presentation timestamp in microseconds (`pts_us`).
    pub fn next_video_frame_pts_us(&mut self) -> u64 {
        let nominal_us = (self.video_frame_count as u128 * 1_000_000 / self.fps as u128) as f64;
        let drift_factor = 1.0 + (self.video_clock_drift_ppm / 1_000_000.0);
        let actual_us = (nominal_us * drift_factor) as u64;

        self.video_frame_count += 1;
        actual_us
    }

    /// Generates next audio buffer presentation timestamp and sample count in microseconds.
    pub fn next_audio_chunk_us(&mut self, chunk_samples: u64) -> (u64, u64) {
        let pts_us =
            (self.audio_sample_count as u128 * 1_000_000 / self.audio_sample_rate as u128) as u64;
        let dur_us = (chunk_samples as u128 * 1_000_000 / self.audio_sample_rate as u128) as u64;

        self.audio_sample_count += chunk_samples;
        (pts_us, dur_us)
    }

    pub fn epoch(&self) -> &SessionEpoch {
        &self.epoch
    }

    pub fn video_frame_count(&self) -> u64 {
        self.video_frame_count
    }

    pub fn audio_sample_count(&self) -> u64 {
        self.audio_sample_count
    }
}

pub fn make_mp4_box(box_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let size = (payload.len() + 8) as u32;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(box_type);
    out.extend_from_slice(payload);
    out
}

/// Generates a fully standard-compliant fragmented MP4 (fMP4) segment containing
/// `ftyp`, `moov` (with `mvhd`, `trak`, and `mvex`), `moof` (with `mfhd`, `traf`, `tfhd`, `tfdt`, `trun`),
/// and `mdat` with real sample payloads and keyframe/sync flags.
pub fn generate_valid_fmp4_segment(start_us: u64, duration_us: u64, is_keyframe: bool) -> Vec<u8> {
    let mut out = Vec::new();

    // 1. ftyp box (32 bytes)
    let mut ftyp_payload = Vec::new();
    ftyp_payload.extend_from_slice(b"isom");
    ftyp_payload.extend_from_slice(&0x00000200u32.to_be_bytes());
    ftyp_payload.extend_from_slice(b"isomiso2avc1mp41");
    out.extend_from_slice(&make_mp4_box(b"ftyp", &ftyp_payload));

    // 2. moov box: mvhd, trak (tkhd, mdia(mdhd, hdlr, minf(vmhd, dinf, stbl))), mvex (trex)
    // mvhd
    let mut mvhd_payload = Vec::new();
    mvhd_payload.push(0); // version
    mvhd_payload.extend_from_slice(&[0, 0, 0]); // flags
    mvhd_payload.extend_from_slice(&0u32.to_be_bytes()); // creation
    mvhd_payload.extend_from_slice(&0u32.to_be_bytes()); // mod
    mvhd_payload.extend_from_slice(&1000u32.to_be_bytes()); // timescale
    mvhd_payload.extend_from_slice(&0u32.to_be_bytes()); // duration
    mvhd_payload.extend_from_slice(&0x00010000u32.to_be_bytes()); // rate 1.0
    mvhd_payload.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
    mvhd_payload.extend_from_slice(&[0u8; 10]); // reserved
                                                // matrix 36 bytes (unity)
    mvhd_payload.extend_from_slice(&0x00010000u32.to_be_bytes());
    mvhd_payload.extend_from_slice(&[0u8; 12]);
    mvhd_payload.extend_from_slice(&0x00010000u32.to_be_bytes());
    mvhd_payload.extend_from_slice(&[0u8; 12]);
    mvhd_payload.extend_from_slice(&0x40000000u32.to_be_bytes());
    mvhd_payload.extend_from_slice(&[0u8; 24]); // pre-defined
    mvhd_payload.extend_from_slice(&2u32.to_be_bytes()); // next track ID
    let mvhd = make_mp4_box(b"mvhd", &mvhd_payload);

    // tkhd
    let mut tkhd_payload = Vec::new();
    tkhd_payload.push(0);
    tkhd_payload.extend_from_slice(&[0, 0, 3]); // flags: enabled | in_movie
    tkhd_payload.extend_from_slice(&0u32.to_be_bytes());
    tkhd_payload.extend_from_slice(&0u32.to_be_bytes());
    tkhd_payload.extend_from_slice(&1u32.to_be_bytes()); // track_id = 1
    tkhd_payload.extend_from_slice(&0u32.to_be_bytes());
    tkhd_payload.extend_from_slice(&0u32.to_be_bytes());
    tkhd_payload.extend_from_slice(&[0u8; 8]);
    tkhd_payload.extend_from_slice(&0u16.to_be_bytes());
    tkhd_payload.extend_from_slice(&0u16.to_be_bytes());
    tkhd_payload.extend_from_slice(&0u16.to_be_bytes());
    tkhd_payload.extend_from_slice(&0u16.to_be_bytes());
    // matrix
    tkhd_payload.extend_from_slice(&0x00010000u32.to_be_bytes());
    tkhd_payload.extend_from_slice(&[0u8; 12]);
    tkhd_payload.extend_from_slice(&0x00010000u32.to_be_bytes());
    tkhd_payload.extend_from_slice(&[0u8; 12]);
    tkhd_payload.extend_from_slice(&0x40000000u32.to_be_bytes());
    tkhd_payload.extend_from_slice(&(1920u32 << 16).to_be_bytes()); // width 1920
    tkhd_payload.extend_from_slice(&(1080u32 << 16).to_be_bytes()); // height 1080
    let tkhd = make_mp4_box(b"tkhd", &tkhd_payload);

    // mdhd
    let mut mdhd_payload = Vec::new();
    mdhd_payload.push(0);
    mdhd_payload.extend_from_slice(&[0, 0, 0]);
    mdhd_payload.extend_from_slice(&0u32.to_be_bytes());
    mdhd_payload.extend_from_slice(&0u32.to_be_bytes());
    mdhd_payload.extend_from_slice(&90_000u32.to_be_bytes()); // timescale 90kHz
    mdhd_payload.extend_from_slice(&0u32.to_be_bytes());
    mdhd_payload.extend_from_slice(&0u16.to_be_bytes());
    mdhd_payload.extend_from_slice(&0u16.to_be_bytes());
    let mdhd = make_mp4_box(b"mdhd", &mdhd_payload);

    // hdlr
    let mut hdlr_payload = Vec::new();
    hdlr_payload.extend_from_slice(&[0, 0, 0, 0]);
    hdlr_payload.extend_from_slice(&0u32.to_be_bytes());
    hdlr_payload.extend_from_slice(b"vide");
    hdlr_payload.extend_from_slice(&[0u8; 12]);
    hdlr_payload.extend_from_slice(b"VideoHandler\0");
    let hdlr = make_mp4_box(b"hdlr", &hdlr_payload);

    // vmhd
    let vmhd = make_mp4_box(b"vmhd", &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]);

    // dinf -> dref -> url
    let url_box = make_mp4_box(b"url ", &[0, 0, 0, 1]); // self-contained
    let mut dref_payload = Vec::new();
    dref_payload.extend_from_slice(&[0, 0, 0, 0]);
    dref_payload.extend_from_slice(&1u32.to_be_bytes()); // entry count 1
    dref_payload.extend_from_slice(&url_box);
    let dinf = make_mp4_box(b"dinf", &make_mp4_box(b"dref", &dref_payload));

    // stbl: stsd (with avc1), stts, stsc, stsz, stco
    let mut avc1_payload = Vec::new();
    avc1_payload.extend_from_slice(&[0u8; 6]); // reserved
    avc1_payload.extend_from_slice(&1u16.to_be_bytes()); // data reference index
    avc1_payload.extend_from_slice(&0u16.to_be_bytes());
    avc1_payload.extend_from_slice(&0u16.to_be_bytes());
    avc1_payload.extend_from_slice(&[0u8; 12]);
    avc1_payload.extend_from_slice(&1920u16.to_be_bytes());
    avc1_payload.extend_from_slice(&1080u16.to_be_bytes());
    avc1_payload.extend_from_slice(&0x00480000u32.to_be_bytes()); // 72 dpi
    avc1_payload.extend_from_slice(&0x00480000u32.to_be_bytes()); // 72 dpi
    avc1_payload.extend_from_slice(&0u32.to_be_bytes());
    avc1_payload.extend_from_slice(&1u16.to_be_bytes()); // frame count
    avc1_payload.extend_from_slice(&[0u8; 32]); // compressor name
    avc1_payload.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
    avc1_payload.extend_from_slice(&(-1i16).to_be_bytes());
    let avc1_box = make_mp4_box(b"avc1", &avc1_payload);

    let mut stsd_payload = Vec::new();
    stsd_payload.extend_from_slice(&[0, 0, 0, 0]);
    stsd_payload.extend_from_slice(&1u32.to_be_bytes());
    stsd_payload.extend_from_slice(&avc1_box);
    let stsd = make_mp4_box(b"stsd", &stsd_payload);

    let stts = make_mp4_box(b"stts", &[0, 0, 0, 0, 0, 0, 0, 0]);
    let stsc = make_mp4_box(b"stsc", &[0, 0, 0, 0, 0, 0, 0, 0]);
    let stsz = make_mp4_box(b"stsz", &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let stco = make_mp4_box(b"stco", &[0, 0, 0, 0, 0, 0, 0, 0]);

    let mut stbl_payload = Vec::new();
    stbl_payload.extend_from_slice(&stsd);
    stbl_payload.extend_from_slice(&stts);
    stbl_payload.extend_from_slice(&stsc);
    stbl_payload.extend_from_slice(&stsz);
    stbl_payload.extend_from_slice(&stco);
    let stbl = make_mp4_box(b"stbl", &stbl_payload);

    let mut minf_payload = Vec::new();
    minf_payload.extend_from_slice(&vmhd);
    minf_payload.extend_from_slice(&dinf);
    minf_payload.extend_from_slice(&stbl);
    let minf = make_mp4_box(b"minf", &minf_payload);

    let mut mdia_payload = Vec::new();
    mdia_payload.extend_from_slice(&mdhd);
    mdia_payload.extend_from_slice(&hdlr);
    mdia_payload.extend_from_slice(&minf);
    let mdia = make_mp4_box(b"mdia", &mdia_payload);

    let mut trak_payload = Vec::new();
    trak_payload.extend_from_slice(&tkhd);
    trak_payload.extend_from_slice(&mdia);
    let trak = make_mp4_box(b"trak", &trak_payload);

    // mvex -> trex
    let mut trex_payload = Vec::new();
    trex_payload.extend_from_slice(&[0, 0, 0, 0]);
    trex_payload.extend_from_slice(&1u32.to_be_bytes()); // track_id
    trex_payload.extend_from_slice(&1u32.to_be_bytes()); // default desc
    trex_payload.extend_from_slice(&3000u32.to_be_bytes()); // default duration
    trex_payload.extend_from_slice(&64u32.to_be_bytes()); // default size
    trex_payload.extend_from_slice(&0u32.to_be_bytes()); // default flags
    let trex = make_mp4_box(b"trex", &trex_payload);
    let mvex = make_mp4_box(b"mvex", &trex);

    let mut moov_payload = Vec::new();
    moov_payload.extend_from_slice(&mvhd);
    moov_payload.extend_from_slice(&trak);
    moov_payload.extend_from_slice(&mvex);
    out.extend_from_slice(&make_mp4_box(b"moov", &moov_payload));

    // 3. moof box
    // mfhd
    let mut mfhd_payload = Vec::new();
    mfhd_payload.extend_from_slice(&[0, 0, 0, 0]);
    mfhd_payload.extend_from_slice(&1u32.to_be_bytes()); // seq 1
    let mfhd = make_mp4_box(b"mfhd", &mfhd_payload);

    // tfhd
    let mut tfhd_payload = Vec::new();
    tfhd_payload.push(0);
    tfhd_payload.extend_from_slice(&[0x02, 0x00, 0x00]); // default-base-is-moof
    tfhd_payload.extend_from_slice(&1u32.to_be_bytes()); // track_id = 1
    let tfhd = make_mp4_box(b"tfhd", &tfhd_payload);

    // tfdt: decode time in 90kHz ticks
    let decode_ticks = (start_us as u128 * 90 / 1000) as u64;
    let mut tfdt_payload = Vec::new();
    tfdt_payload.push(1); // version 1 (64-bit)
    tfdt_payload.extend_from_slice(&[0, 0, 0]);
    tfdt_payload.extend_from_slice(&decode_ticks.to_be_bytes());
    let tfdt = make_mp4_box(b"tfdt", &tfdt_payload);

    // trun
    let sample_duration_ticks = (duration_us as u128 * 90 / 1000) as u32;
    let sample_size = 64u32;
    let first_sample_flags = if is_keyframe {
        0x02000000u32 // sync sample (keyframe: sample_is_non_sync_sample = 0, sample_depends_on = 2)
    } else {
        0x00010000u32 // non-sync sample (sample_is_non_sync_sample = 1)
    };

    let mut trun_payload = Vec::new();
    trun_payload.push(0); // version 0
    trun_payload.extend_from_slice(&[0x00, 0x00, 0x05]); // flags: data_offset_present | first_sample_flags_present
    trun_payload.extend_from_slice(&1u32.to_be_bytes()); // sample_count = 1
    trun_payload.extend_from_slice(&0i32.to_be_bytes()); // placeholder for data_offset
    trun_payload.extend_from_slice(&first_sample_flags.to_be_bytes());
    // sample entries: duration and size (since flags not per-sample)
    // Note: trun flags 0x000100 (sample-duration-present) + 0x000200 (sample-size-present)
    let trun_flags = 0x000305u32; // data_offset + first_sample_flags + sample_duration + sample_size
    trun_payload[1] = ((trun_flags >> 16) & 0xFF) as u8;
    trun_payload[2] = ((trun_flags >> 8) & 0xFF) as u8;
    trun_payload[3] = (trun_flags & 0xFF) as u8;
    trun_payload.extend_from_slice(&sample_duration_ticks.to_be_bytes());
    trun_payload.extend_from_slice(&sample_size.to_be_bytes());

    let trun_temp = make_mp4_box(b"trun", &trun_payload);
    let mut traf_payload = Vec::new();
    traf_payload.extend_from_slice(&tfhd);
    traf_payload.extend_from_slice(&tfdt);
    traf_payload.extend_from_slice(&trun_temp);
    let traf = make_mp4_box(b"traf", &traf_payload);

    let mut moof_payload = Vec::new();
    moof_payload.extend_from_slice(&mfhd);
    moof_payload.extend_from_slice(&traf);
    let moof_size = (moof_payload.len() + 8) as i32;

    // Fixup data_offset in trun (offset from start of moof to mdat payload)
    // moof_size + 8 bytes (mdat header)
    let data_offset = moof_size + 8;
    // data_offset is at index 8 in trun_payload (after version/flags (4) and sample_count (4))
    trun_payload[8..12].copy_from_slice(&data_offset.to_be_bytes());
    let trun_final = make_mp4_box(b"trun", &trun_payload);

    let mut traf_payload_final = Vec::new();
    traf_payload_final.extend_from_slice(&tfhd);
    traf_payload_final.extend_from_slice(&tfdt);
    traf_payload_final.extend_from_slice(&trun_final);
    let traf_final = make_mp4_box(b"traf", &traf_payload_final);

    let mut moof_payload_final = Vec::new();
    moof_payload_final.extend_from_slice(&mfhd);
    moof_payload_final.extend_from_slice(&traf_final);
    let moof = make_mp4_box(b"moof", &moof_payload_final);
    out.extend_from_slice(&moof);

    // 4. mdat box containing 64-byte sample payload
    let mut sample_payload = vec![0xABu8; sample_size as usize];
    if is_keyframe {
        sample_payload[0..5].copy_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65]); // H.264 IDR NAL
    } else {
        sample_payload[0..5].copy_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x41]); // H.264 Non-IDR
    }
    let mdat = make_mp4_box(b"mdat", &sample_payload);
    out.extend_from_slice(&mdat);

    out
}

/// Generates a standard-compliant PCM WAV file with valid headers and sample data.
pub fn generate_valid_wav_segment(duration_us: u64, sample_rate: u32, channels: u16) -> Vec<u8> {
    let total_samples = ((duration_us as u128 * sample_rate as u128) / 1_000_000) as usize;
    let data_bytes = (total_samples * channels as usize * 2) as u32; // 16-bit PCM
    let riff_chunk_size = 36 + data_bytes;

    let mut out = Vec::with_capacity((44 + data_bytes) as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_chunk_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");

    // fmt subchunk
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // 16 for PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM format = 1
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * channels as u32 * 2;
    out.extend_from_slice(&byte_rate.to_le_bytes());
    let block_align = channels * 2;
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    // data subchunk
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    // Fill with audio samples (alternating values to avoid all-zeros)
    for i in 0..total_samples {
        let sample_val = ((i % 100) as i16 * 100) as i16;
        for _ in 0..channels {
            out.extend_from_slice(&sample_val.to_le_bytes());
        }
    }

    out
}

/// 16-bit PCM WAV from interleaved sample frames. Frame count is `samples.len() / channels`.
pub fn generate_pcm16_wav(sample_rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
    assert!(channels > 0);
    assert_eq!(samples.len() % channels as usize, 0);
    let data_bytes = (samples.len() * 2) as u32;
    let riff_chunk_size = 36 + data_bytes;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_chunk_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * channels as u32 * 2;
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_synthetic_stream_pts_progression() {
        let mut stream = SyntheticCaptureStream::new(30, 48_000, 0.0);

        let f0 = stream.next_video_frame_pts_us();
        let f1 = stream.next_video_frame_pts_us();
        let f2 = stream.next_video_frame_pts_us();

        assert_eq!(f0, 0);
        assert_eq!(f1, 33_333); // 1/30 sec in us
        assert_eq!(f2, 66_666);

        let (a0, d0) = stream.next_audio_chunk_us(1024);
        assert_eq!(a0, 0);
        assert_eq!(d0, 21_333); // 1024/48000 sec in us
    }
}
