# E1 — Opened-project metadata and segment index

Work package ID / status: **E1 / contract-tested** (native playback and waveforms remain planned)

Commit + working-tree changes (including frontend tracking limitations): uncommitted implementation on 2026-09-08. Parent git may omit `front-end/` if that directory is still a `160000` gitlink; inventory frontend files explicitly before review.

OS / architecture / device / SDK / compiler / dependency versions: local macOS development tree; `cargo test --manifest-path src-tauri/Cargo.toml` and `npm run build` in `front-end/`.

Fixture or real capture provenance / recording configuration: synthetic `ProjectBundle` fixtures with committed WAV journal records; `start_recording_impl` / `stop_recording_impl` test sessions (native capture disabled in `AppState::new_test`).

Reproduction commands and manual steps:

```sh
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app
# in front-end/
npm run build
```

Manual: Stop a desktop recording, confirm Edit Studio shows that bundle’s track counts/durations, then open a second `.aero` folder and confirm the editor state changes. A failed Stop must remain on Record Scene.

Expected outcome: read-only open builds a per-track source-time index from the journal, pages segments, rejects escaping/malformed input without mutation, and does not fabricate missing tracks or preview pixels.

Observed outcome and measured values: see `src-tauri/tests/project_reader_tests.rs` (two bundles, reopen, paged DTO, first-file `relative_path` plus a later segment, stale writer lock, missing media diagnostics, truncated tail, corrupt interior, oversized/FIFO rejection, Stop `projectPath`, empty bundle).

Artifact paths (local/private where appropriate): in-test tempdirs only.

Checks not run and exact reason: real-device decode/playback (E3/F1); waveform cache (E2); Windows read-only flock lease (reader returns an explicit unsupported error on non-unix).

Open gates / next smallest package: **E2 is contract-tested** ([E2_WAVEFORM_CACHE.md](E2_WAVEFORM_CACHE.md)). **E3 — synchronized playback, seek and basic timeline edits** is next and still requires applicable native gates.

## Review fixes — 2026-09-08

Failed project replacement now uses the backend's atomic open behavior. The editor keeps the old project and live handle if the replacement is invalid. Regression coverage verifies the old position, paged segments and reader lease remain usable. `cargo test --test project_reader_tests`: 8 passed; frontend `npm run build`: passed. Live Tauri interaction is not claimed by these checks.


Repository handoff (2026-09-09): the broken frontend gitlink has been replaced by ordinary staged frontend source files. Existing implementation changes remain uncommitted. See REVIEW_E1_E4_F1_F2.md for the consolidated resolution and qualification limits.
