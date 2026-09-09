> **Resolution update — 2026-09-09:** The original findings below are preserved as review history. Their code fixes are now implemented. See the resolution table at the end for current behavior, verification and remaining qualification gates.

# Review of E1, E2, E3, E4, F1 and F2

Reviewed 2026-09-08 against the six task evidence documents and sections 10.4–10.8 of AEROSHOOT_MASTER_PLAN.md. Scope includes the current uncommitted Rust, Swift and frontend files, including files omitted by the frontend gitlink. This is a review, not an implementation change.

**Verdict: do not mark all six tasks finished.** E1 is substantially implemented. E2 and E3 have correctness gaps; E3 implements planning and a simulated clock, not synchronized media playback. F1/F2 and E4 remain limited experiments, as their evidence documents partly acknowledge. Several bugs also violate those narrower contracts.

## Findings

P1 = fix before relying on the feature; P2 = substantive correctness or acceptance gap.

### 1. P1 — Waveform cache writes can overwrite source or metadata through a temporary symlink (E2)

[src-tauri/src/project/waveform.rs:525](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/src/project/waveform.rs:525) uses `File::create` on the deterministic `.tmp` sibling of a validated `.wf1` cache path. Only the final cache path was checked for symlinks. A preexisting `.tmp` symlink is followed and its target truncated before the rename. A supplied project can therefore turn waveform analysis into a write to unrelated writable files, including source media. This violates the no-source-mutation contract even without a concurrent race.

Reproduce in a disposable project: build its cache, remove the `.wf1`, put a symlink at the corresponding `.tmp` pointing to a disposable metadata file, query again. Use an exclusively created, unpredictable temporary file with no-follow semantics; validate/open cache files as regular files too.

### 2. P1 — Video export uses project source time as the time inside every segment (E4/F2)

[src-tauri/src/export/mod.rs:297–305](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/src/export/mod.rs:297) passes `source_us` directly to `decode_h264_frame`. It ignores the selected segment's start and media timestamp mapping. The decoder at [src-tauri/native/macos/AeroShootMedia.swift:62](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/native/macos/AeroShootMedia.swift:62) interprets that value as an AVAsset time. For independently written zero-based segments, a segment starting at source 2 seconds must decode its first frame at local zero, not at 2 seconds. Later segments can fail to decode or select the wrong frame.

The current red/green parity test does not independently assert that the second picture is green: it recomputes the reference using the same faulty evaluator. The fixture writer also writes only two 30 fps frames, whereas the test declares each segment to span 200 ms. A temporary native probe confirmed that the second evaluated cut frame is byte-for-byte identical to the first red frame even though decoding the second source at local zero produces green. Add assertions against known source colors/motion and real segment/container timestamps.

### 3. P1 — Unsupported or failed video decode still produces a “completed” export (E4/F2)

[src-tauri/src/export/mod.rs:305–323](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/src/export/mod.rs:305) discards decode errors and holds the previous screen frame or renders background; webcam decode errors disappear as an absent layer. In particular, [src-tauri/native/macos/AeroShootMedia.swift:14](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/native/macos/AeroShootMedia.swift:14) rejects any source image dimension above 512, without scaling it first. A normal captured screen is therefore rejected even when the output canvas is only 64×64. The job can publish a background-only or frozen MP4 with no diagnostic.

Known journal gaps may deliberately hold a frame; failure to decode an allegedly available source must be reported. Reject unsupported inputs/settings upfront or decode/scale them, and validate the output before publishing. The current finish path only finishes the writer, syncs and publishes; it does not independently validate the output despite the UI's “decoded natively” success text.

### 4. P1 — PCM sample timing describes a whole chunk as the duration of every sample (E4)

[src-tauri/native/macos/AeroShootExport.swift:264–278](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/native/macos/AeroShootExport.swift:264) supplies `sampleCount: frames`, one timing entry, and `duration: frames / sampleRate`. For a shared timing entry the duration must describe one sample frame (`1 / sampleRate`). A 4,800-frame block at 48 kHz currently declares 0.1 seconds per sample rather than per block. This makes the CMSampleBuffer timing invalid for the intended audio timeline, irrespective of whether AAC conversion reconstructs timing from the sample rate.

A native probe calling the current function confirmed that `CMSampleBufferGetDuration` reports **480 seconds for a 100 ms block**. Use one-frame duration and verify decoded audio timestamps/sample count, not merely AVAsset's overall container duration.

### 5. P1 — Audio export neither resamples nor mixes all channels (E4)

[src-tauri/src/export/audio.rs:154–158](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/src/export/audio.rs:154) calculates a number of 48 kHz output frames, reads that many source frames at the source rate, and copies only `buf[i * channels]`. Sample rates other than 48 kHz play at the wrong speed and can leave silence or omit samples. Stereo content entirely in the right channel exports as silence.

Reproduction cases: one second of constant 24 kHz mono becomes 24,000 nonzero samples followed by 24,000 zero samples in the 48 kHz output; `[0, 16384]` stereo becomes all zeros. Resample by source/output frame positions and implement an explicit channel mix/preservation policy. The existing 48 kHz mono fixture cannot detect either issue.

### 6. P2 — Most timeline clicks send an invalid integer argument (E3)

[front-end/src/components/timeline/TimelineStudio.tsx:78](/Users/georgepaterakis/Desktop/AeroShoot.AI/front-end/src/components/timeline/TimelineStudio.tsx:78) calculates `progress * durationUs`. `useTimeline.ts` clamps it but does not round it; [front-end/src/lib/ipc.ts:428–429](/Users/georgepaterakis/Desktop/AeroShoot.AI/front-end/src/lib/ipc.ts:428) forwards it unchanged to [src-tauri/src/lib.rs:245](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/src/lib.rs:245), which requires `u64`. For example, one pixel into a 300-pixel, 1-second timeline produces 3333.3333333333335 microseconds. JSON deserialization rejects that fraction, and the UI only logs a warning. Round and validate microseconds at the IPC boundary and test the actual serialized argument.

### 7. P2 — Failed persistence leaves edits applied in memory (E3)

[src-tauri/src/project/revision.rs:189–196](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/src/project/revision.rs:189) changes the undo/redo stacks, revision and intervals before saving. `undo` and `redo` do the same. On IO failure the command returns an error, but history has already changed; the reader summary and playback remain on the old document. Subsequent requests can get stale-revision errors, and export reads a different document from the visible editor.

Reproduce by making `project.json` a directory in a disposable project before applying a cut. Prepare/save the next document first, then atomically update in-memory history on success.

### 8. P2 — Undo reuses revision identities, defeating stale-write rejection (E3/E4)

[src-tauri/src/project/revision.rs:240–243](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/src/project/revision.rs:240) restores the previous document's revision. Cut A creates revision 1, undo restores 0, and a different cut B creates revision 1 again. A delayed request based on A now passes the revision guard against B. Default export filenames also reuse `-r1.mp4` for different content.

Keep a monotonically increasing mutation revision separate from the historical content restored by undo/redo. Existing tests explicitly expect revision rollback and should be updated.

### 9. P2 — Editing while playing freezes the clock but leaves state “playing” (E3)

[src-tauri/src/playback/mod.rs:147](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/src/playback/mod.rs:147) clears `play_anchor` during `apply_document` but leaves `PlaybackState::Playing` unchanged when the position is inside the edited duration. `advance` then returns without advancing. Ripple-cut/undo/redo during playback leaves the UI showing Pause while the playhead stalls. Either pause explicitly or preserve/rebase the running clock, updating the current position first.

### 10. P2 — Same-size interior PCM changes leave stale waveform data (E2)

[src-tauri/src/project/waveform.rs:391–417](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/src/project/waveform.rs:391) fingerprints only 256 bytes at the start, middle and end, plus file/data lengths. Edits elsewhere in a WAV do not change its cache key. Changing samples at 100–110 ms in a one-second WAV leaves a cached silent waveform; deleting the cache then reveals the signal. Hash the relevant source content or use a trustworthy source identity/version. The current whole-file constant-to-zero test only changes the sampled regions.

### 11. P2 — A viewport query loads/analyzes every segment (E2)

[src-tauri/src/project/waveform.rs:103–113](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/src/project/waveform.rs:103) builds/loads every segment and retains all cache arrays before looking up the requested range. A tiny viewport in a long recording therefore scans the whole audio track and memory grows with the entire project. The per-segment cache cap does not bound total query memory. Filter by retained source ranges first and use a bounded cache working set or streamed aggregation. Acceptance specifically asks for bounded long-input and viewport behavior.

### 12. P2 — Export drops the last valid frame for non-frame-aligned durations (E4)

[src-tauri/src/export/mod.rs:438](/Users/georgepaterakis/Desktop/AeroShoot.AI/src-tauri/src/export/mod.rs:438) floors `duration * fps`. For 250 ms at 10 fps it emits only frames at 0 and 100 ms, although 200 ms is also before the exclusive end. A short cut below one frame is rejected even though time zero is a valid sample. Use the count of rational timestamps strictly below the duration, and explicitly handle the final sample duration; check bounds before narrowing to u32.

### 13. P2 — The native preview does not track scrolling or clipping (F1)

[front-end/src/components/canvas/NativePreviewHost.tsx:64–72](/Users/georgepaterakis/Desktop/AeroShoot.AI/front-end/src/components/canvas/NativePreviewHost.tsx:64) observes size, window resize and document visibility only. Its host is in an `overflow-auto` container (`EditStudioScene.tsx`). Scrolling changes `getBoundingClientRect` without changing size, so no new geometry is sent. The AppKit sibling stays at its old position above the webview and can obscure/intercept unrelated controls. It also lacks the DOM ancestor's clipping.

Track scroll/position changes and visible intersection; send clipped geometry or hide the overlay outside the visible viewport. Test against the actual Tauri/React page, not only an NSWindow fixture.

### 14. P2 — Failed project replacement leaves a stale editor (E1)

[front-end/src/components/scenes/EditStudioScene.tsx:82–92](/Users/georgepaterakis/Desktop/AeroShoot.AI/front-end/src/components/scenes/EditStudioScene.tsx:82) closes the old backend project before opening the new one, but does not clear/restore the frontend state if opening fails. Open valid A, then try an invalid path: A remains visible with a dead handle and all operations fail. `open_project_impl` already constructs the new reader before replacing the old one. Use that atomic replacement behavior, or clear the editor after a successful close.

## Task acceptance assessment

| Task | What is present | Assessment |
|---|---|---|
| E1 | Bounded metadata reader, journal-derived paged index, shared reader lease, Stop project path, real editor metadata | Substantially implemented; failed replacement UI needs fixing. Windows reader intentionally unsupported. |
| E2 | Supported WAV parsing, per-channel energy, real waveform drawing, cache and cancellation checks | Partial: unsafe cache write, stale cache identity and whole-track query behavior violate acceptance. |
| E3 | Rust seek plans, interval mapping, bounded open file list, edit persistence and undo/redo | Not complete as synchronized playback. There is no running video decoder/compositor/audio output. `advance()` always uses `Instant`, while `clock_kind()` labels it Audio merely because an audio segment exists. The ordinary UI has no enabled manual trim/ripple-cut control; the only cut call is behind the disabled silence workflow. Correctness issues remain in seek, history and clock updates. |
| F1 | AppKit surface, fixed color and fixture presentation, basic geometry/generation/hit contracts | Feasibility slice only; live Tauri viewport/scroll/lifetime acceptance remains open and scrolling has a code-level defect. |
| F2 | Owned BGRA interface, WGPU offscreen composition, macOS codec fixture parity | Feasibility slice only. No real captured segmented H.264 qualification or live preview integration; source dimensions/time mapping are not production-ready. Windows, Rec.709 and zero-copy are explicitly deferred. |
| E4 | Immutable document clone, background worker, H.264/AAC writer, status/cancel and temp publication | Experimental export slice only. UI forces 64×64 at 10 fps; backend caps output dimensions at 512, video at 120 frames and audio at 8 seconds. Bugs above prevent accepting even the narrower correctness claims. |

The documents' explicit “not hardware-qualified” caveats are appropriate. However, “contract-tested” does not imply that full task acceptance is met. Missing audio playback, decode scheduling and UI editing are missing implementation, not just hardware qualification. Not choosing FFmpeg is a recorded architecture decision; I am not treating that alone as a bug.

## Verification and limitations

- Frontend `npm run build`: passed.
- `cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app`: passed.
- Existing Rust tests: all **117** passed across the completed runs: 76 unit, 22 general integration, 7 project reader, 5 playback, 1 waveform, 4 export and 2 media interop. The first sandboxed run failed on native fixture/GPU access; rerunning the export and F2 binaries outside the sandbox passed. This was an environment restriction, not evidence of a native test regression.
- Eight temporary Rust probes reproduced findings 1, 5 (two cases), 7, 8, 9, 10 and 12. These probes assert the observed buggy behavior; their passing result confirms reproduction, not feature correctness.
- A temporary native cut probe reproduced finding 2: the second reference frame stays red rather than becoming green.
- A Swift/CoreMedia probe reproduced finding 4: 0.1 s of PCM has a reported sample-buffer duration of 480 s.
- Standalone F1 preview contract: passed outside the sandbox (child overlay, decoded fixture, HUD hit test). The initial sandboxed run failed at H.264 fixture creation.
- Actual Tauri/React user flows, live capture, 1080p export, Windows and long-duration performance were not qualified in this review.

Temporary probes only used disposable fixtures and were removed after execution. No production-code fixes are part of this review.

Additional coverage needed: a captured multi-segment screen with movement; different sample rates and asymmetric stereo; expected content assertions independent of the evaluator; fractional UI seeks; failed persistence and stale revision branches; long viewport queries; real Tauri scroll/resize/close/reopen; cancellation during mux/publication; disk-full behavior.

The repository records `front-end` as mode 160000 (a gitlink), but `git -C front-end status` resolves to the parent repository in this checkout. Frontend files were inspected directly and built. Before handing this work off, ensure those source changes are actually committed in a recoverable frontend repository or converted to ordinary tracked files; the parent change list currently cannot represent them.


## Resolution — 2026-09-09

| Finding | Resolution and regression evidence |
|---|---|
| 1: waveform temporary symlink | Exclusive random temporary files; symlink target preservation and FIFO rejection tests. |
| 2: segment timestamps | Global source time becomes segment-local decode time; export independently asserts red then green at a cut. |
| 3: decoder dimensions/failure | 4096 maximum, 1080p input preserved; invalid available video fails with no output. |
| 4: PCM timing | Per-sample CoreMedia duration is 1/sampleRate; native output duration is checked. |
| 5: resampling/channels | Bounded 48 kHz stereo mixer preserves right-channel content, filters downsampling and handles cuts consistently across chunks. |
| 6: fractional seek IPC | Frontend rounds/clamps edited microseconds before invoke. |
| 7: failed persistence | Persist first, mutate history only on success; regression checks failed-save stacks and document. |
| 8: reused revisions | Undo/redo allocate fresh revision IDs; disk revision and edit lock reject stale independent editors. |
| 9: editing while playing | Advance then pause and discard clock/audio state; command-boundary regression. |
| 10: stale waveform identity | Full PCM fingerprint plus cache-body checksum; interior edit and corrupt-cache regressions. |
| 11: viewport memory/work | Only intersecting retained source spans load audio; one segment cache at a time; 10,000 irrelevant segments remain unopened. |
| 12: final export frame | Checked ceiling frame count, exact writer end time; 250 ms/10 fps export produces three 1080p frames. |
| 13: overlay scrolling/clipping | Changed geometry measured continuously, ancestor clipping and native mask/hit tests, generation-qualified teardown. |
| 14: failed project replacement | Frontend no longer closes first; backend opens atomically under command lock; old handle/position survive failure. |

Additional implementation closes the original E3/E4 scope gaps: native media worker, real audio output clock, direct AppKit project-frame presentation, manual keep/delete ranges, output settings, streaming audio, safe publication and post-finalization validation. Decoder/compositor/export share one evaluator; playback/export share one audio mixer. Export holds a source lease, and cancellation remains pending until cleanup completes.

Validation: complete Rust suite (131 tests), desktop-feature Rust check, frontend TypeScript/Vite build, and standalone Swift native preview contracts. Fixtures include asymmetric stereo, 24/44.1/192 kHz resampling, animated H.264/backward seek, source-resolution preservation, independent cut colors, corrupt media and cancellation after a rendered frame. One parallel edit-history run failed intermittently, passed in isolation and on rerun; edit locking now explicitly unlocks before closing to avoid fork-inherited descriptors delaying release. The final suite passed all 131 tests after that change.

Frontend handoff is repaired: the broken mode-160000 gitlink is removed and frontend source files are staged as ordinary files; node_modules/dist remain ignored. No commits were created and existing source work was preserved.

Remaining qualification, not claimed as completed: live Tauri scroll/resize/click/reopen and sustained real-capture A/V sync, native seek latency over thousands of segments, long-duration/thermal throughput, calibrated color, disk-full injection and Windows. A temporary desktop test app launched, but computer-use actions could not target its newly created application identity. The native contracts and build checks do not substitute for those live-device gates. A repeatable silent 16-second fixture generator is retained in `src-tauri/examples/studio_fixture.rs`.
