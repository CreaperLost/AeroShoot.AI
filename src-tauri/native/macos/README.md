# macOS capture bridge

`AeroShootCapture.swift` is compiled into the Rust binary by `build.rs` for macOS targets with a
macOS 13 deployment target. It exposes a small C ABI; Rust owns the opaque recording handle and
always stops it before releasing project state.

The bridge provides:

- ScreenCaptureKit display, window, and application enumeration and capture, including optional
  system audio and exclusion of AeroShoot's own application from full-display capture.
- AVFoundation camera and microphone enumeration and capture.
- Camera, microphone, and Screen Recording permission preflight/request functions.
- hardware-required H.264 encoding through Apple's VideoToolbox-backed asset writer, with a
  two-second fragmented-MP4 interval.
- isolated 48 kHz PCM WAV files for system and microphone audio.
- per-sample native `CMTime` plus microseconds mapped onto the Rust session's monotonic epoch in
  `telemetry/media_timestamps.jsonl`.

The output callbacks run on dedicated serial queues. Video frames are dropped when the encoder is
backpressured; audio overflow is counted and exposed through the existing session-status command.
Stop first halts producers, drains all callback queues, then finalizes temporary outputs. Rust
durably syncs and atomically renames each file before appending its journal record.
