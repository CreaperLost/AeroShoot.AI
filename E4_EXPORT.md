# E4 — Basic export and preview parity

Status (2026-09-09): implemented and regression-tested on Apple M1/macOS. Native 1080p output is tested; sustained 1080p30/4K throughput and Windows are not qualified.

An export captures an immutable edit revision, track index and shared project lease. The background job uses the same SceneEvaluator and AudioMixer as playback. Source-local segment timestamps are used for decoding. Available-source decode failures fail the job; declared screen gaps hold the previous retained picture and webcam gaps omit the webcam. Exclusive-end timestamps are never decoded.

Output is H.264/AAC MP4. UI offers 720p, 1080p and 4K at 24/30/60 fps; default is 1080p30. Backend allows even dimensions up to 4096 and 10/15/24/25/30/60 fps, with a checked u32 frame-count bound. The former 512-pixel, 120-frame and eight-second caps are removed. Fractional final frames are retained using ceiling frame count, and the native writer ends the session at the exact edited duration.

Audio streams in at most 4,800-frame stereo chunks at 48 kHz. Mono duplicates into both channels; stereo preserves left/right; larger channel sets average even/odd channels into left/right. Windowed-sinc resampling includes downsampling filtering. Noncontiguous retained junctions get eight-millisecond fades. PCM sample timing is one/sampleRate per sample, not the duration of the entire buffer. Output duration validation allows 80 ms for AAC priming/delay; fractional-duration regression requires less than 20 ms.

Output paths cannot target source media or the project bundle. The unique partial file is finalized, decoded at first/last frame, checked for dimensions/duration and synced before publication. Hard-link publication or macOS exclusive rename prevents overwrite races. Cancellation stays pending until the worker cleans up; panic becomes a typed failure. Owner teardown cancels and joins outstanding work. Sources and existing destinations are preserved.

Color: explicit SDR Rec.709 conversion/tagging and full-range BGRA; encoded-space compositing. No HDR, linear-light or zero-copy claim. Preview/export mean tolerance remains 16 and region mean 48; source colors are also asserted independently of the shared evaluator.

Seven native export regressions pass: policy failures; independent red/green cut parity and immutable revision; cancellation race; cancellation before work; cancellation after first render with partial cleanup; corrupt available video with no publication; and 1920×1080 export of 250 ms at 10 fps with three frames, correct final green frame and duration. The seven-test run took 1.55 s including fixtures; this is not sustained export throughput.

Reproduce: `cargo test --manifest-path src-tauri/Cargo.toml --test export_tests`. Native tests require GPU/codec access. Frontend compilation and the desktop-feature Rust build are also checked.

Unqualified: sustained thermal load, actual disk-full injection, real captured multi-track A/V sync, Windows. Zooms and advanced layout controls belong to Z2/A1. FFmpeg is not a dependency: system AVFoundation/VideoToolbox is the selected macOS backend.
