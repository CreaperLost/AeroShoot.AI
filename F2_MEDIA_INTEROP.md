# F2 — Decoder, compositor and encoder interoperability

Status (2026-09-09): macOS implementation and automated native regressions pass on Apple M1. Windows and sustained production performance remain unqualified.

AVAssetReader/AVAssetReaderTrackOutput supplies sequential BGRA samples. The decoder selects the last frame whose PTS is at or before the requested local timestamp, restarts on backward seeks and retains only current/next samples. A two-entry LRU bounds segment decoders. Input dimensions are preserved up to 4096; larger images are proportionally scaled through CoreImage. Invalid available media propagates errors instead of silently exporting backgrounds. File identity includes size and modification time.

Owned VideoFrame buffers feed the WGPU 27 compositor, native AppKit presentation and H.264 encoder. Decode, GPU upload/readback, channel swizzles, AppKit presentation and encoder input involve bounded copies; Metal zero-copy is not claimed. Audio uses the shared bounded stereo mixer described in E3/E4. The native player device-clock regression is included here.

Color contract: CoreGraphics and CoreImage use Rec.709; H.264 writers explicitly tag Rec.709 primaries, transfer and matrix. Rust describes full-range BGRA and encoded Rec.709 compositing. HDR/linear-light compositing are outside this implementation. CPU nearest-neighbor reference now samples pixel centers to match GPU rasterization; existing tolerances are unchanged (GPU/CPU mean ≤3, max ≤40; export mean ≤16, region mean ≤48).

Four native media integration tests cover GPU/CPU/export parity, 1920×1080 source preservation, animated multi-frame timestamp selection/backward seek and audio device clock progression. Export regressions independently assert red/green source content across a segment cut and validate a 1080p final frame. Tests use synthetic H.264/PCM fixtures, not private captures.

Run `cargo test --manifest-path src-tauri/Cargo.toml --test media_interop_tests --test export_tests` with native GPU/codec access. The complete Rust suite and `cargo check --features tauri-app` are also verified.

Architecture: macOS AVFoundation/VideoToolbox + WGPU, with owned buffer boundaries. No Homebrew FFmpeg assumption or bundled FFmpeg is introduced. A future FFmpeg backend needs its own pinned build and license review. Real captured screen/webcam/audio sessions, sustained 1080p30 performance, thermal load, software GPU fallback, calibrated color measurements and Windows require device qualification.
