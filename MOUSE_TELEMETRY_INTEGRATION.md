# macOS mouse telemetry — Step 2

Implemented on 2026-09-08 against baseline `033bb3ed036d5528b0566698a5cb5c7b715c9e74` plus the working-tree segment integration and telemetry changes.

## Recording path

`MouseHookMac.swift` installs a passive, listen-only session event tap on a dedicated run-loop thread. Mouse movement/drag, left/right/auxiliary button transitions and scroll are captured; keyboard input is not collected. CGEvent timestamps map from host uptime nanoseconds to the same CoreMedia host epoch and session offset used for media. See [Apple's CGEventTimestamp contract](https://developer.apple.com/documentation/coregraphics/cgeventtimestamp).

The tap only appends bounded records under a lock. Consecutive movement is coalesced without crossing button/scroll transitions. A 100 ms worker drains at most 2,048 records; overflow persists an explicit affected interval and lost-event count. Rust validates the version, sequence, geometry references and coordinates, then appends v2 JSONL records to `telemetry/events.jsonl` and revisions to `telemetry/geometry.jsonl`. Writes flush per drain; stop joins the event thread, drains the worker and syncs both files. Persistence failures cause a failed stop result.

Button transitions are authoritative, including auxiliary button identifiers. No duplicate derived click events are emitted. Dwell durations can be derived from stationary movement intervals and the project duration, excluding gaps; no automatic zoom or dwell annotation algorithm is claimed here. Initial held-button state, state following an overflow, and state across pause intervals are unknown. Visibility, cursor shapes and modifiers are omitted because this adapter does not reliably know them. V1 event types remain unchanged.

## Geometry and cursor behavior

Display and window bounds use Quartz global coordinates, including negative desktop origins. Coordinates are normalized without clamping. Revisions are recorded on observed movement/resize or display pixel/rotation changes. Display records include physical dimensions and logical-to-physical scale. Window records deliberately omit unknown physical dimensions; mixed-display window pixel transforms and capture content rectangles still require qualification. Application-wide capture has no single reliable source rectangle in this adapter and emits `unsupported_source_geometry`.

Geometry is sampled every 100 ms, not synchronized to captured frames. A change emits an uncertainty gap for that polling interval. Consumers must exclude these intervals from precision zoom generation. The manifest explicitly records `cursorMode: "baked"`; ScreenCaptureKit keeps its cursor enabled. This feature does not enable cursor replacement.

## Permission flow

The Record scene includes an explicit “Enable mouse tracking” action. It invokes the listening/Input Monitoring permission API independently of camera, microphone and Accessibility permissions. Preflight runs at recording startup without prompting. Unavailable permissions or unsupported geometry are persisted as gaps and reported through session diagnostics; ordinary recording continues. Permission changes apply to the next recording. Timeout-disabled taps recheck authorization before re-enabling. User-input-disabled taps are not automatically restarted.

## Verification

Environment: ARM64 macOS 26.6.2, Rust 1.98.1, Apple Swift 6.3.3. The bridge targets macOS 13.

- **Passed:** 55 Rust unit tests and 22 integration tests, including v2 schema round trips, auxiliary buttons, negative coordinates, scroll precision, gaps, invalid version/sequence/reference rejection and symlink protection.
- **Passed:** `sh script/test-mouse-telemetry.sh`: synthetic native timestamp, normalization, coalescing, overflow, pause, tap-disable and missing-sink failure contracts. It installs no event tap and records no user activity.
- **Passed:** `cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app` and frontend `npm run build` (TypeScript/Vite).

Live tap permission denial/revocation, signed-app behavior, real click/media alignment, window moves between mixed-DPI displays, long sessions and teardown under sustained physical input remain hardware acceptance gates. No hardware pass is inferred from synthetic tests.

Frontend note: this workspace represents `front-end` as a Git submodule entry but its checkout currently has no independent Git metadata. The permission UI files are edited on disk and built successfully; the parent repository's ordinary diff does not enumerate their changes.
