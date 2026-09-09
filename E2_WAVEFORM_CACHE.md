# E2 — Real waveform cache and channel-aware audio reads

Work package ID / status: **E2 / contract-tested** (native decode/playback remains planned as E3)

Commit + working-tree changes (including frontend tracking limitations): uncommitted implementation on 2026-09-08. Parent git may omit `front-end/` if that directory is still a `160000` gitlink; inventory frontend files explicitly before review.

OS / architecture / device / SDK / compiler / dependency versions: local macOS development tree; `cargo test --manifest-path src-tauri/Cargo.toml`, `cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app`, and `npm run build` in `front-end/`.

Fixture or real capture provenance / recording configuration: synthetic `ProjectBundle` fixtures with committed PCM16 WAV journal records (`generate_pcm16_wav`); µ-law header mutation for unsupported-encoding gaps.

Reproduction commands and manual steps:

```sh
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app
# in front-end/
npm run build
```

Manual: Stop a desktop recording, open Edit Studio, and confirm mic/system lanes show real amplitude (constant tone is visibly energetic; digital silence is a hairline; a missing WAV is a dim gap, not a fake bar). Browser emulation cannot open projects.

Expected outcome: viewport queries return per-channel then `max_energy` peak/RMS buckets mapped through retained intervals; rebuildable caches under `cache/waveforms/` miss after same-size PCM rewrites; cancellation stops analysis; unsupported encodings and missing files are gaps with diagnostics.

Observed outcome and measured values: see `src-tauri/src/project/pcm.rs` tests (zero/constant/opposite-polarity energy, 48 kHz frame index 48,000 = 1 s, µ-law rejection), `src-tauri/src/project/waveform.rs` tests (real WAV zero/0.5/stereo, late segment vs missing file, stale-cache fingerprint, analysis cancellation, unsupported encoding), and `src-tauri/tests/waveform_tests.rs` (serialized `project_waveform` command boundary and missing-file gaps).

Artifact paths (local/private where appropriate): in-test tempdirs; on-disk cache files `cache/waveforms/{track_id}/{fnv}.wf1`.

Checks not run and exact reason: real-device decode/compositor (F1/F2); live desktop waveform drawing of a captured session (requires the Tauri app and a recorded bundle); Windows read-only project open (reader returns an explicit unsupported error on non-unix).

Open gates / next smallest package: **E3 is contract-tested** ([E3_PLAYBACK_EDITS.md](E3_PLAYBACK_EDITS.md)). **F1/F2 are contract-tested**. **E4** is next for export.

## Review fixes — 2026-09-08

Cache version 3 uses a cancellable full-file fingerprint and validates its header/body checksum and numeric bounds. Writes use exclusively created unpredictable temporary files; cache reads reject FIFOs and symlinks. Viewport queries filter source intersections before touching media and aggregate one segment cache at a time. PCM shorter than journal metadata creates an unavailable tail rather than invented silence.

Eight waveform unit tests and the serialized waveform integration test pass, including interior PCM changes, corrupted cache payloads, a temporary symlink pointing to metadata, a FIFO at the cache path and 10,000 segments with only the visible segment accessible. Source/metadata targets remain unchanged.


Repository handoff (2026-09-09): the broken frontend gitlink has been replaced by ordinary staged frontend source files. Existing implementation changes remain uncommitted. See REVIEW_E1_E4_F1_F2.md for the consolidated resolution and qualification limits.
