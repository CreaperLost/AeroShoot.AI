# Native segment commit integration — 2026-09-08

Implemented against baseline commit `033bb3ed036d5528b0566698a5cb5c7b715c9e74`, with the working-tree changes accompanying this document.

Swift retains the existing rotating AVAssetWriter adapter for screen/webcam MP4 and system/microphone WAV. After finalization and native validation it submits the temporary file to Rust. `TrackSegmentWriter::commit_native_segment` checks the expected track/index path, rejects symlinks and malformed containers, preserves the supplied rational timestamp anchor, syncs the file, publishes using a same-filesystem hard link that cannot replace a destination, removes the temporary name, syncs the directory, and appends the durable journal record. A failure after publication leaves an orphan for explicit recovery and returns failure; it cannot become a successful callback. No media bytes pass through webview IPC.

The segment callback now returns an integer status. Swift latches a failed commit and reports it through rotation/stop. Callback registration happens before native startup. Stop no longer scans temporary/orphan files and silently journals them with inferred timestamps. Recovery remains a separate operation.

## Verification

Environment: macOS 26.6.2 (25G83), ARM64; Rust 1.98.1; Apple Swift 6.3.3. Swift bridge build targets macOS 13.

- `cargo test --manifest-path src-tauri/Cargo.toml`: 50 unit and 22 integration tests passed after the pipeline change.
- After adding regressions, `cargo test --manifest-path src-tauri/Cargo.toml project::segment_writer::tests`: all 3 tests passed, including native WAV publication/clock metadata/collision preservation and malformed media/path/symlink rejection.
- `cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app`: passed, including the native Swift bridge. An initial attempt was invalidated by a concurrent comment edit; the clean rerun passed.
- `npm run build` in `front-end`: TypeScript and Vite production build passed.

These are build and synthetic contract checks, not native media qualification. The existing MP4 validator is a container validator, not a full decoder. No screen, camera, or microphone was recorded for this verification.

## Remaining acceptance procedure

On a permission-authorized device, select a real display, camera, microphone and system audio. Start recording from the UI; verify that numbered media and matching journal entries appear while recording, then pause/resume and stop. Decode every segment independently and verify clock offsets across all tracks. Force termination after multiple commits and run recovery, checking that prior committed intervals survive. Inject disk-full/journal failures and confirm an error result. Run the master plan's 60-minute 1080p30 test and record A/V skew, frame drops, queue memory, resize/device-removal behavior and artifact paths before closing Step 1 or Phase 2.

Writer restart overhead, capture callback backpressure, long-session clock drift and real encoded MP4 compatibility remain unmeasured. This change consolidates persistence without selecting an unmeasured replacement encoder strategy.
