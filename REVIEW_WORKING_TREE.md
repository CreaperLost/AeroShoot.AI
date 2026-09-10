# AeroShoot.AI working-tree review

Date: 2026-09-09; user-testing addendum: 2026-09-10. Scope: the uncommitted tree at `/Users/georgepaterakis/Desktop/AeroShoot.AI`, HEAD `5e09149782b4a967deefe5b2e6ee9d0750feb7c9`; source of truth: `AEROSHOOT_MASTER_PLAN.md`. Status vocabulary: planned → implemented → contract-tested → hardware-qualified.

**Verdict: unsafe to treat as a product slice. Needs fixes before any commit.**

The components are substantially implemented, but passing component tests does not establish the integrated lifecycle, HUD/editor settings, or analysis-to-edit contracts. Two durability blockers can turn failure into apparent success. Additional reproducible defects affect silence cuts, telemetry continuity, webcam geometry, and frontend ownership. The owner's live-app smoke test also found the Record and Edit previews unusable, a malformed webcam popup, recording output of only about one second, and unavailable timeline preview. These are major product-slice failures even where their underlying causes are not yet isolated. No feature is hardware-qualified by this review.

Only this review document was changed by the reviewer. No fixes, commits, resets, or unrelated reversions. Temporary probes are outside the repository at `/private/tmp/aeroshoot-review-probes/`. Test/build outputs are generated artifacts. The earlier review was used as a warm-start and is superseded by the reconciled findings below.

## Major issues found in owner testing (2026-09-10 addendum)

These observations come from the owner's live-app test and supplied screenshot (`codex-clipboard-cc89abfb-bb3f-4269-a64c-1745124dc706.png`), not from the reviewer's automated or synthetic test runs. They should be treated as release-blocking integrated UX failures and reproduced under instrumentation before assigning a root cause.

### U1 — Major: Record and Edit previews are broken and scrollable

**Remediation status (2026-09-10):** implemented and accepted in owner testing. Responsive scene grids, bounded native-preview geometry, scene-transition sizing, and recording aspect-ratio persistence were repaired. This records owner acceptance of the issue-specific fix; it does not promote the package to hardware-qualified.

Both scenes present an unusable preview rather than a stable canvas fitted to its viewport. In the supplied image, the Edit preview is displaced/clipped within the workspace and the page/panel exposes scrolling where the preview should remain bounded. The Record scene likewise does not present the intended composed scene cleanly. This is direct evidence that the current viewport/native-preview integration does not meet the F1 acceptance gate, regardless of component and playback test results.

### U2 — Major: webcam popup/composition is malformed

**Remediation status (2026-09-10):** implementation complete; awaiting owner verification. The sticky camera HUD is now recording-only, uses the selected recording camera mailbox, requests optional camera permission from the explicit Record action, clips/fills its native image to the selected shape, passes input through to its drag surface, remains always-on-top and movable, and hides without stopping the recording or destroying the reusable native surface.

The Record scene shows a large detached white webcam circle overlapping the screen capture, with incorrect content/geometry instead of a correctly sized and positioned camera bubble. This is more severe than the non-square distortion already described in M1: the live composition itself is visibly unusable. The cause may involve HUD/window placement, mailbox content, clipping, transforms, or shared layout ownership; the screenshot does not distinguish among them.

### U3 — Major: recording produces only about one second of usable screen media

The owner's live test reports that recording does not function beyond roughly one second of screen output. This symptom was not exercised by the synthetic writer tests and is not contradicted by their passing result. Capture duration, segment commits, manifest timing, Stop finalization, and independent media decoding must be inspected from the affected project bundle before narrowing the defect.

### U4 — Major: timeline preview does not work

The Edit scene shows `Video preview unavailable` for both the screen and webcam visual tracks. It also displays implausible/inconsistent timing metadata—approximately `193799.00s` source/edited duration and a `3229:58.99` total—while the owner reports an approximately one-second recording. This makes timeline playback and editing unusable and suggests that media availability and/or timebase/duration propagation is broken in the real handoff. The screenshot corroborates the unavailable-preview and bad-timing UI states; it does not by itself identify their source.

## Findings, ordered by severity

Paths below are relative to the repository root; line numbers refer to the reviewed working tree.

### B1 — Blocker: actual journal failures lose pending publication

**Files:** `src-tauri/src/project/segment_writer.rs:248`, `:382`, `:410`, `:416`; `src-tauri/src/capture/macos.rs` (`c_segment_callback`).

`finish_pending_journal` saves `pending_publication` only in the special `DurabilityFault::FailJournalAppend` branch. The ordinary `journal.append(...)?` error exits without saving it. Both commit callers have already published the file; the synthetic caller has also discarded its active handle/path. Its next `finalize()` therefore returns `Ok(None)` despite the absent journal record. The native callback additionally constructs a throwaway writer, so in-memory pending state cannot survive that callback's return.

**Reproduced:** link the real freshly built `aeroshoot_lib.rlib`; start/write a segment; call `journal.inject_fail_next_appends(1)`; finalize. First call errors, `has_pending_publication()` is false, second finalize returns `Ok(None)`, the final media file exists, and the journal has zero records. The existing writer-level fault test takes a different branch and misses this. This refutes the general H1 pending-publication/retry claim even within software scope.

### B2 — Blocker: native Stop failure is forgotten on retry

**Files:** `src-tauri/src/commands/mod.rs:1063`, `:1069`, `:1144`, `:1160`; `src-tauri/src/capture/macos.rs` (`stop_with_result`); `src-tauri/native/macos/AeroShootCapture.swift:2700`; `src-tauri/src/session/state.rs`.

Stop takes and consumes `native_session`. On native failure it restores `ActiveSession` with no native handle and no retained native Stop outcome. A second Stop is allowed from Error; `was_native` is now false, so both the failed-native-finalization check and the required-screen-media check disappear. It can save the manifest and return Completed/projectPath. The same bypass follows a first Stop that reports no screen media. A native preparation failure also retains an owner with no native handle, without an explicit unsuccessful terminal outcome.

This is a deterministic control-flow defect; reproducing a real native failure was not attempted. `AppState::new_test` disables native capture, so the existing ownership test cannot cover this branch. Retaining a project bundle is not equivalent to retaining its finalization outcome.

### H1 — High: failed Pause can leave native capture paused while reporting Recording

**Files:** `src-tauri/native/macos/AeroShootCapture.swift:2326`; `src-tauri/src/commands/mod.rs:895`, `:917`, `:946`; `front-end/src/hooks/useRecording.ts:75`.

Swift sets `paused=true` before draining/finalizing. A failure returns to Rust without restoring sample admission or transitioning to an honest non-recording/error state. Rust keeps Recording. On native finalization failure, Resume merely refuses with “retry Pause or Stop.” More subtly, successful native finalization sets `native_pause_unacked=false` before `PauseStarted` is journaled: if that append fails, Resume returns Recording as a no-op although Swift is still paused. The displayed timer may advance during a frozen capture.

Pause/Resume catches only log the immediate command error. However, the earlier review overstated error invisibility: while polling remains active, `lastRuntimeError` can reach the alert. The state inconsistency remains a defect even when its diagnostic appears. Needs native-adapter/fault coverage for both finalization failure and journal failure after native success.

### H2 — High: frontend loses recording ownership and cannot recover failed sessions

**Files:** `front-end/src/hooks/useRecording.ts:8`, `:18`, `:116`; `front-end/src/App.tsx:164`; `front-end/src/components/navigation/TopNavBar.tsx:60`; `front-end/src/components/recording-hud/RecordingFloatingDock.tsx:76`; `src-tauri/src/commands/mod.rs:516`.

Recording state starts as local `idle` and is queried only after the hook already believes it is Recording/Paused. The always-enabled scene switch unmounts RecordScene and its hook. Return to RecordScene during an active recording and the dock shows Record, with no initial backend snapshot or Stop action. For a paused backend, Start is rejected because it still owns the bundle; the user cannot reach Resume/Stop through the dock.

Likewise, failed Start/Stop sets the hook to Error, stops polling, and shows only Record. The backend intentionally refuses another Start while retaining recoverable ownership. The frontend provides no Stop retry/recovery action, making that safety mechanism a UI dead end. While switching to Edit during recording, the retained `live_preview` flag can also keep the media worker on the live mailbox instead of project evaluation. This is software wiring, not a hardware qualification question.

### H3 — High: silence suggestions are not bound to their analyzed revision

**Files:** `front-end/src/components/silence-modal/SilenceModal.tsx:68`, `:275`; `front-end/src/stores/projectStore.ts:248`, `:257`; `src-tauri/src/commands/mod.rs:1294`, `:1930`.

Detection returns edited-time cuts without a project/revision token. Closing the modal preserves them; applying another timeline revision does not clear/remap them. Reopening the modal submits those old coordinates with the **current** revision, defeating the backend stale-revision check. Example: detect silence at edited [5s,6s], close the modal, delete [0s,2s], reopen and apply: it removes original source [7s,8s], not the analyzed silence. An in-flight scan can also complete after closing/replacing the project without a result identity guard.

**Reproduced against the actual TypeScript store:** a revision 1 suggestion at [5s,6s] survives `applyOpenedProject` revision 2 unchanged. The normal S1 backend tests do not exercise this UI sequence. Cuts remain undoable, but the operation can remove speech the user never selected.

### H4 — High: HUD snapshots overwrite editor layout and selected device

**Files:** `front-end/src/stores/settingsStore.ts:99`, `:117`, `:122`; `front-end/src/App.tsx:80`; `front-end/src/components/inspector/InspectorPanel.tsx:32`; `src-tauri/src/hud/mod.rs` (`reconcile_cameras`).

The selected camera changes only in Zustand; no command updates `HudOwner.camera_id`. Reconciliation initially chooses the first camera, and every HUD snapshot writes that ID back into `selectedCameraId`. Focus/device refresh can therefore revert camera B to camera A, including before the next Start.

The same snapshot overwrites `cameraBubble`, which is also the editor's view of persisted layout. `hydrateLayout` does not update HudOwner. Open/undo a project with different shape/mirror settings, then receive a HUD snapshot: inspector values change while the project and native evaluator do not. The next unrelated inspector change persists those overwritten values back into the project.

**Reproduced against the actual stores:** a HUD snapshot resets B→A and squircle/unmirrored→circle/mirrored while `openedProject.layout` remains squircle/unmirrored. These are concrete parallel-package ownership collisions; Rust-only subscriber tests cannot establish the frontend convergence claim.

### H5 — High: corrupt telemetry does not break zoom continuity

**Files:** `src-tauri/src/telemetry/reader.rs:145`, `:181`, `:350`; `src-tauri/src/zoom/mod.rs:433`.

The reader records corrupt interior lines/unknown records as diagnostics and continues without a gap or invalid interval. Sequence discontinuities are not converted into uncertainty either. Dwell generation can consequently join events across missing/corrupt telemetry and invent continuous interest.

**Reproduced through the real reader and generator:** move at 0.2s, corrupt interior record, same-position move at 1.2s → corruption diagnostic **and one dwell zoom** spanning the missing record. Explicit typed gaps and denied telemetry do pass their existing tests; that does not cover damaged/omitted gap records. Z1's general “gapped telemetry does not fabricate zooms” claim needs narrowing or repair.

### H6 — High: HUD fallback is applied after native capture starts

**Files:** `src-tauri/src/lib.rs:102`, `:111`, `:118`, `:27`; `src-tauri/native/macos/AeroShootCapture.swift:1954`, `:2089`.

The actual HUD window is hidden only by `sync_hud_after_session` after the blocking Start has returned successfully. ScreenCaptureKit starts producing frames before optional device startup, mouse startup, and that return. A HUD already visible while idle remains visible through that initial capture interval, despite `exclusionEstablished=false`. Start errors also bypass the post-Start synchronization via `??`.

The snapshot's desired-hidden state is not proof that the AppKit window was hidden before capture. The promised fallback must precede native admission and handle failed preparation. Whether the current filter happens to exclude the HUD is an open hardware gate, not a justification for this ordering.

### M1 — Medium: circle/squircle webcam clips distort non-square video

**Files:** `src-tauri/src/render/mod.rs:230`, `:254`, `:742`.

`webcam_bubble_size` returns a square for these shapes, but the webcam layer keeps full-frame UV; no aspect-preserving cover crop is applied before clipping. Both CPU and GPU squeeze the complete 16:9 source into that square.

**Reproduced:** a 1280×720 source becomes a 122×122 webcam layer with `uv_w=uv_h=1`. A circle drawn in the input becomes an ellipse inside the round bubble. Solid-color clipping and CPU/GPU agreement cannot detect this shared geometric mistake. Test a non-uniform aspect reference in both render paths.

### M2 — Medium: Record inspector and HUD styling are disconnected from native pixels

**Files:** `front-end/src/components/inspector/InspectorPanel.tsx:33`; `front-end/src/components/canvas/CapturePreview.tsx`; `src-tauri/native/macos/AeroShootLivePreview.swift:124`; `src-tauri/src/playback/engine.rs:84`; `front-end/src/components/camera-overlay/HudOnlyRoot.tsx:60`.

In RecordScene, inspector persistence explicitly returns early. Live preview uses a separate fixed Core Image composition (1280×720, camera at a hardcoded rectangle), not the selected EditLayout. Start options do not transfer the customizer layout to the new project, and opening the recording hydrates its default project layout. Aspect/padding/shape choices made before recording therefore do not survive as promised scene parameters.

HUD presents the raw camera mailbox; its CSS mirror/rounded classes do not configure a corresponding native pixel transform/clip. Native hit mode is a separate mechanism. These missing data paths should be fixed or the controls constrained before asking a hardware experiment to establish styled pixel parity.

### M3 — Medium: deferred `rect_16_9` remains selectable through HUD and accepted by Rust

**Files:** `front-end/src/components/camera-overlay/HudOnlyRoot.tsx:10`; `src-tauri/src/hud/mod.rs` (`HudShape`); `src-tauri/src/project/layout.rs:193`; `src-tauri/src/render/mod.rs:808`.

The inspector disables the option, but HUD shape cycling includes it. Shared snapshots bring it back into editor settings and layout validation accepts it. The renderer maps it to no clip and uses source-aspect sizing, not a guaranteed 16:9 crop. The feature is not disabled at the effective boundary as §10.1/A1 claim.

### M4 — Medium: failed recording handoff can discard the previously usable project

**Files:** `front-end/src/hooks/useRecording.ts:105`; `src-tauri/src/commands/mod.rs:1662`.

Stop-and-open explicitly closes the previous backend project before opening the new one. If the new open fails, the frontend retains the old project DTO but its backend handle is gone. Subsequent editor operations fail. Backend `open_project_impl` already stages the replacement before assigning it, so the frontend close defeats the E1 preserve-on-failed-replacement contract. The existing backend test cannot cover this sequence.

### M5 — Medium: qualification evidence is not a reproducible clean pass

**Files:** `src-tauri/tests/recording_lifecycle_tests.rs:360`; `AEROSHOOT_MASTER_PLAN.md:527`, `:930`.

The first requested lifecycle run failed `pause_boundaries_and_late_track_preserve_source_time` with `Lock(AlreadyLocked { pid: 0 })`; the isolated case and subsequent full 7-test binary passed. Root cause is not established. Record it as an intermittent failure, not “all passed.” B1 additionally demonstrates that the current fault tests miss a directly reachable error branch. These facts refute blanket confidence in H1's evidence even without running native capture.

### L1 — Low: stale plan/review guidance can send the next agent to the wrong work

See the plan reconciliation below. The stale Start timeout comment and previous review's incorrect startup/aspect claims are explicitly corrected rather than repeated as defects.

## Plan versus tree and package status

No reviewed status promotes a package to hardware-qualified. The problem is over-broad **contract-tested** interpretation and stale implementation instructions.

| Package | Review assessment |
| --- | --- |
| E1 | Real metadata/open/handoff implemented; backend replacement contract exists, but frontend handoff violates it (M4). |
| E2 | Real PCM/waveform/cache code exists; waveform integration test passed. No new specific E2 defect established. |
| E3/E4 | Project preview and export both use `SceneEvaluator`; revisions, source mapping and output safety exist. Playback passed; all 8 export tests passed with native resources available. End-to-end UI ownership and styling defects remain. |
| F1 | AppKit feasibility contracts only. Live Record preview is a separate pipeline; live styled/tracked pixels remain unqualified. |
| F2 | GPU/CPU comparison and all 5 native interop tests passed outside the sandbox; the sandbox failures remain documented. No live production qualification. |
| Z1/Z2 | Source-anchored persistence and screen-only UV wiring confirmed; corruption continuity defect H5 remains. |
| A1 | Local wallpaper ingest and CPU/GPU clips/shadows exist; shape aspect, control wiring and deferred-crop enforcement are incomplete (M1–M3). |
| S1 | Real gap-aware PCM detector and one-revision ripple command confirmed; stale frontend suggestions violate the integrated contract (H3). |
| H2 software | Separate `{supported, authorized}` DTO, 100ms uncertainty, explicit gaps and baked cursor confirmed. Live permission/geometry/alignment gates remain open. Reader corruption is a Z1 consumption defect, not evidence that the native denial hook fabricates events. |
| H3 software | Verified root routing, dedicated HUD owner and no second HUD recording session confirmed. Actual device/layout convergence and hide-before-start ordering are defective (H4/H6). |
| H1 lifecycle slice | Existing tests cover selected synthetic failures; B1/B2/H1 and intermittent M5 prevent treating the full claimed retry/ownership contract as established. |

Specific plan mismatches:

- §10.1's **W1 if targeting Windows** next assignment is current; it does not mistakenly reassign A1. Its blanket package list needs the defects above recorded before macOS work is considered closed.
- §1.1 still says production `previewAvailable:false` and lists failure injection as future evidence without distinguishing existing tests from missing experiments.
- §10.2's frontend gitlink warning is stale: `git ls-files --stage front-end` shows ordinary `100644` entries, not `160000`. S1/H3 evidence repeats this warning even though the E1–F2 resolution paragraph says it was repaired.
- §10.2's native-preview row says availability remains false; playback updates it after successful presentation. Its export row still says 512px/120 frames, while code permits even dimensions through 4096 and `MAX_EXPORT_FRAMES=u32::MAX` (with duration/frame validation).
- §10.5 E1 step 1 says Stop lacks projectPath and calls open/close proposed names. All are implemented and registered.
- Z1 evidence's “next Z2” and Z2 evidence's “next A1 or S1” are historical and stale as current handoff guidance.
- H1 pending-publication, retry and ownership descriptions exceed the real error-path guarantees (B1/B2). H3 says the HUD is hidden while a session is active but window synchronization happens too late (H6). A1/§10.1 says `rect_16_9` stays disabled but the HUD admits it (M3).
- H2's evidence lists 15 telemetry and 10 zoom library tests; this tree's requested filters ran 14 telemetry and 9 zoom tests. Historical counts must not be copied as current-run evidence.
- HDR, ProRes, cursor replacement, keyboard capture and filler-word/ASR remain deferred; export validation allows H.264/AAC. No new support was inferred.

## Shared-file collision and path audit

No conflict markers were found in the reviewed source/plan, and `git diff --check` passed. Command registration and TypeScript compiled; the issues are semantic ownership collisions rather than missing textual merge resolution.

- `lib.rs`/`commands/mod.rs`: registrations are present; lifecycle locking exists. Pause/Resume remain synchronous Tauri commands despite potentially long native waits. B2/H1/H6 cover meaningful lifecycle/window ordering failures.
- `capture/macos.rs`/Swift/`segment_writer.rs`: typed errors and no-overwrite publication are wired; Stop does not salvage-scan leftover temps. B1/B2 show where retry truth is lost.
- `render/mod.rs`/`export/mod.rs`: editor and export consume the same evaluator; zoom UV applies only to `LayerRole::Screen`. Wallpaper reads validated local `assets/` paths; no export-time URL fetch was found. Shared evaluation does not prove shared math is correct (M1).
- `hud/`, `main.tsx`, settings/inspector: verified identity chooses studio/HUD/rejected before mount; HUD does not call Start. The idle monitor is a distinct capture pipeline that is stopped before recording, not a second HUD recording session. Shared frontend camera state is the substantive collision (H4).
- `telemetry/`/`zoom/`: explicit permission/gap paths are honest within their tests; corrupt/omitted records are not a continuity barrier (H5).
- S1: detector streams PCM, resets on explicit missing/unsupported media, uses sample frames/channel energy, and calls the real ripple command. Result identity is lost in the frontend (H3).

## Corrections to the earlier review

1. **Start-completion finding withdrawn:** `ActiveRecorder.startScreen` requires completion and errors after 15 seconds (`AeroShootCapture.swift:2089–2105`). Rust's “future timeout” comment is stale, not executable behavior.
2. **Blanket fixed-preview distortion finding withdrawn:** live preview explicitly fits source aspect with the minimum axis scale. Fixed 1280×720 alone does not prove distortion. The reproduced distortion is in circle/squircle scene layers (M1).
3. **Retained CVPixelBuffer corruption not established:** no live recycling experiment was run; reference retention alone is insufficient to assert corrupted pixels.
4. **Pause error invisibility narrowed:** immediate catches console-log, but session diagnostics are polled into the alert while Recording/Paused. Failed state/ownership handling, not universal invisibility, is the confirmed issue.

## Hardware gates still open — exact missing experiments

None of the following was run in this review. Synthetic VideoToolbox/GPU tests, even outside the sandbox, do not close them.

| Gate | Missing experiment |
| --- | --- |
| F1 | Launch the real Tauri app; attach project preview; scroll/resize/occlude and change backing scale; verify styled/zoomed pixels track the React viewport without intercepting controls; close/reopen repeatedly without dangling views/callbacks. |
| H1 | Signed/dev authorized four-track display+webcam+mic+system capture for at least two 2-second rotations; Pause during live capture; verify closed containers and journal ordering; Resume into new sequence IDs; Stop and independently decode every segment/keyframe start. Inject writer/journal/Stop failures and verify retries preserve honest outcomes. |
| H1 durability/soak | Kill the actual recording process after a real native journaled commit, reopen/recover and independently decode prior segments. Run 60 minutes measuring four-track skew, memory, dropped frames and gaps. Exercise disk-full/unmount failure; do not equate dropping a synthetic writer with kill-after-native-commit. |
| H2 | Signed app Input Monitoring grant, deny, and mid-recording revoke; recording must continue without fabricated input. Move a captured window across negative-origin and mixed-DPI displays; preserve unsupported geometry until measured. Compare physical clicks to decoded media frames. Exercise live tap timeout, user-disable, sustained overflow, and stop while moving. |
| H3 | With HUD visibly enabled, verify ScreenCaptureKit self-exclusion in recorded pixels; circle/squircle transparent-corner click-through; chrome drag/controls and keyboard focus/z-order; camera mailbox presentation during recording without another capture session. Also verify the fallback hides the real window before the first frame. |
| A1/F1/E4 | Non-uniform styled 9:16 project with wallpaper, screen radius/shadow, webcam shape/mirror, and zoom: compare real AppKit preview to decoded H.264 export pixels, including cuts/gaps. Include a non-square webcam aspect marker. |
| S1 | Real mic/system recording: inspect detected silence, apply/undo/reopen cuts, and confirm retained speech and audio/video alignment. No ASR/filler-word experiment is implied. |

Windows W1–W3, signed packaging, defined CSP (currently null), color/zero-copy/performance targets, and device-specific 60fps/4K qualification remain open. They are not implicitly qualified by the macOS software checks.

## Test ledger and progress

The tree was inventoried with `git status --short` and `git diff --stat` before reading/editing. Initial tracked diff: 63 files, 7,414 insertions, 1,490 deletions, plus untracked package files. Review did not stage anything.

| Check | Observed result |
| --- | --- |
| Requested combined layout/silence/zoom/HUD/lifecycle Cargo command | HUD 6/6 and layout 5/5 passed; lifecycle 6/7 with M5 lock failure; Cargo stopped before silence/zoom. |
| Lifecycle isolated rerun, then full compiled binary rerun | Isolated failing case passed; full binary 7/7 passed. Initial failure retained above. |
| Already-built silence/zoom binaries from that Cargo build | Silence 6/6, zoom 4/4 passed. |
| `npm run build` in `front-end/` | Passed TypeScript and Vite production build. No browser UI test runner is configured. |
| `git diff --check`; conflict-marker scan | Passed / no conflict markers found. |
| Independent Rust probes using the real compiled library | B1, M1 and H5 reproduced; source in `/private/tmp/aeroshoot-review-probes/probes.rs`. |
| Independent real TypeScript store probes (IPC stubbed; no React/native surface) | H3/H4 reproduced. This does not simulate a full browser or native app. |
| `cargo check --features tauri-app` | Passed with real Swift bridge; no `AEROSHOOT_SKIP_SWIFT` used. |
| Requested lib render/hud/telemetry/zoom plus project writer/layout | 45/46 passed in sandbox; GPU comparison failed because Metal exposed no adapter. Same GPU test passed with sandbox disabled. |
| Additional suites with `--no-fail-fast` | Integration 24/24; project_reader 9/9; waveform 1/1; playback 6/6; silence 6/6; zoom 4/4 passed. Export 0/8 in sandbox: every case failed while creating its H.264 fixture. Media interop passed its status test then aborted with “Rust cannot catch foreign exceptions.” With sandbox disabled, export 8/8 and media interop 5/5 passed. |
| `sh script/test-mouse-telemetry.sh` | Passed synthetic hook contracts, no event tap installed. |
| `sh script/test-recording-writer.sh` | Sandbox run trapped at `AeroShootCapture.swift:2753`, the IOSurface-backed `CVPixelBufferCreate` precondition, after compiling successfully. Unsandboxed rerun passed: two committed segments each for screen, webcam, mic and system, using synthetic samples; no ScreenCaptureKit session. |

All requested checks have completed. No failure was skipped: native-resource failures were retained and rerun outside the sandbox; the intermittent lifecycle lock failure remains unexplained despite successful isolated/full reruns. Compiler output included Swift deprecation/unused-variable warnings and Rust's `block v0.1.6` future-incompatibility warning. A passing compile is not a zero-warning result.

Reproduction commands (from the repository root, except frontend build):

```sh
cargo test --manifest-path src-tauri/Cargo.toml --test layout_tests --test silence_tests --test zoom_tests --test hud_tests --test recording_lifecycle_tests
cargo test --manifest-path src-tauri/Cargo.toml --lib -- render:: hud:: telemetry:: zoom:: project::segment_writer project::layout
cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app
cargo test --manifest-path src-tauri/Cargo.toml --test silence_tests --test zoom_tests --test integration_tests --test project_reader_tests --test waveform_tests --test playback_tests --test media_interop_tests --test export_tests --no-fail-fast
sh script/test-mouse-telemetry.sh
sh script/test-recording-writer.sh
# In front-end/:
npm run build
```

The already-built lifecycle test binary was rerun with `--exact pause_boundaries_and_late_track_preserve_source_time`, then without a filter. The already-built lib binary was rerun with `--exact render::tests::gpu_cpu_clip_shadow_wallpaper_within_tolerance` outside the sandbox. Export and media interop were rerun with `cargo test --manifest-path src-tauri/Cargo.toml --test export_tests --test media_interop_tests --no-fail-fast` outside the sandbox. The writer script was likewise rerun outside the sandbox. No live-capture test was substituted for a synthetic test.

The standalone Rust probe was compiled with `rustc --edition=2021 /private/tmp/aeroshoot-review-probes/probes.rs --extern aeroshoot_lib=src-tauri/target/debug/deps/libaeroshoot_lib.rlib -L dependency=src-tauri/target/debug/deps -o /private/tmp/aeroshoot-review-probes/probes` and then executed. The TypeScript probe transpiled/imported the actual `settingsStore.ts`, `projectStore.ts`, and `types.ts` with the installed TypeScript compiler and Zustand; IPC was stubbed and no UI/native app was controlled.

Not run: every live experiment listed in the hardware table, a signed app launch, Windows checks, power-loss/storage-unmount experiments, a React/browser interaction suite, or the entire unfiltered Cargo library suite. The native Stop and Pause cases are source-traced rather than executed against live ScreenCaptureKit. Only the review document was intentionally edited; the master plan and implementation remain unchanged by this reviewer. The initial and final tracked diff statistics match (the review document is untracked).
