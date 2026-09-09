# E3 — Synchronized playback, seek and basic timeline edits

Status (2026-09-09): implemented on macOS; automated contracts pass. Sustained playback and live desktop interaction remain qualification gates, not claimed results.

The desktop starts one native media worker. It maps edited time through the shared retained intervals, decodes screen/webcam segments, composites with WGPU and presents BGRA directly to the AppKit surface. Pixels never cross JavaScript IPC. Generation checks guard decode completion, presentation, project replacement and frontend status updates. Decoder cache capacity is two segments. The playback runtime retains a shared project lease.

Audio uses AVAudioEngine/AVAudioPlayerNode at 48 kHz stereo with three 100 ms chunks queued ahead. Position comes from the player sample timeline with output presentation latency accounted for. The status reports Audio only when an output device is actually installed; silent/headless playback uses the monotonic clock. Seek discards queued audio and reinitializes at the new edited position. Errors stop playback and clear the displayed frame.

Manual trim and ripple-delete controls now operate on seconds-based range inputs and the playhead. Frontend seeks round to integer microseconds. An edit while playing pauses at the mapped position. Undo/redo restore content while allocating monotonically increasing revision IDs. Persistence precedes history mutation; a failed save leaves the document and stacks unchanged. A per-project edit lock and on-disk revision check reject concurrent stale writers.

Verification: six playback integration tests cover cut/undo/reopen, absent-audio clock advancement, edit-while-playing, unique generations, wire serialization and 2,048 segment seeks with at most eight open files. Revision unit tests cover failed persistence, stale independent writers and branch revision IDs. Native audio integration verifies that the device clock advances. Shared mixer tests cover asymmetric stereo, 24/44.1/192 kHz resampling, cut fades, chunk-independent output and bounded queries on a one-hour timeline.

Reproduce with `cargo test --manifest-path src-tauri/Cargo.toml`, `cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app`, and `npm run build` in `front-end`.

The repeatable silent desktop fixture is generated with `cargo run --manifest-path src-tauri/Cargo.toml --example studio_fixture -- /tmp/aeroshoot-validation-new`. It contains two eight-second screen segments (red then green), 720p source and 48 kHz PCM. Open its printed `.aero` path; play, seek across eight seconds, trim, undo/redo and reopen. Sustained A/V sync, measured native seek latency, real captured four-track playback and Windows require separate device qualification. The 2,048-seek test measures planning/file bounds, not decoder latency.
