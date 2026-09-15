# AeroShoot.AI — Master Architectural & Implementation Plan

> **Persistent Blueprint & Development Roadmap**
> Target Platforms: macOS (Apple Silicon ARM64, macOS 13+) & Windows 11 (x86_64 / ARM64)
> Plan revision: 2026-09-08 (Native integration status & agent execution guide)
> Current baseline: Completed two-scene studio frontend with dedicated Record Scene & Edit Studio Scene, physical hardware device deck (zero mock devices, real displays, mics with VU meter, webcams with hotplug), responsive aspect-ratio canvas (16:9, 9:16, 4:3, 1:1), and full-height multi-track timeline studio; shared Rust recording foundations (session, journal, segment writer, recovery); and in-progress macOS ScreenCaptureKit/AVFoundation capture bridge.
> Core Framework: Tauri v2 (Rust) + React / TypeScript / Vite + Native OS Capture Modules
> **Start here when implementing:** [Section 10 — Agent Execution Guide](#10-agent-execution-guide) provides the current task selection rule, actual file map, ordered work packages, and acceptance examples. Read it together with the subsystem requirements; it does not waive native qualification gates.
>
> **Current recording recovery work:** [`ACTIVE_RECORDING_WORK.md`](ACTIVE_RECORDING_WORK.md) is the maintained short list of observed defects, priorities, and qualification gates. Use it for day-to-day sequencing; this document remains the architectural target.

---

## 1. Executive Summary & Vision

**AeroShoot.AI** is a next-generation screen recording and video creation studio combining the low-level capture power of **OBS Studio** with the automated post-production polish of **Screen Studio** and **Riverside**.

### Core Value Proposition

1. **Isolated Multi-Stream Recording**: Simultaneously captures high-quality, hardware-encoded screen video, isolated webcam video, system audio, and microphone audio onto separate tracks.
2. **Telemetry-Driven Smart Zoom**: Captures raw global mouse coordinates and click events during recording to allow fluid, camera-director-style smooth zooms and pans in post-production.
3. **Non-Destructive In-App Studio Editor**: Full post-processing workspace allowing users to adjust zoom keyframes, reposition/resize the webcam bubble, customize canvas backgrounds (padding, rounded corners, drop shadows), and edit audio.
4. **Intelligent Post-Processing (AI Jump Cuts)**: Audio-based silence and filler detection with user-guided timeline ripple editing to trim dead air across all synchronized tracks.
5. **Measured Performance**: Native Rust + C++/Swift media processing with bounded memory. Initial qualification targets 1080p30; 1080p60 and 4K60 are enabled only after device-specific capture, encoder, and thermal checks. No zero-lag guarantee.

### 1.1 Implementation Status and Decision Policy

This document specifies the target architecture. Requirements are not claims that the current implementation satisfies them. Record qualification results with the tested commit, toolchain, OS, device, configuration, procedure, and artifacts; passing synthetic tests cannot close native capture or preview gates.

| Area | Baseline observed on 2026-09-08 | Required next evidence |
| --- | --- | --- |
| Desktop Studio & GUI | **E1–E4 and F1/F2 contract-tested**: Stop opens a real project; waveforms come from PCM; Rust owns playback; an AppKit overlay exists; WGPU can composite a styled scene; VideoToolbox can encode/decode it; E4 muxes H.264/AAC from the same evaluator. Production frames are still unavailable (`previewAvailable: false`). | Live Tauri preview qualification; do not treat HTML video or a solid overlay as production playback. |

| Shared recording foundations | Rust session state machine, monotonic clock, bounded queues, segment writer, project manifest, journal, and recovery engine implemented. | Failure-injection, concurrency, actual media validation, and cross-platform durability acceptance. |
| macOS capture | `AeroShootCapture.swift` and `capture/macos.rs` integrate rotating AVAssetWriter containers with Rust `TrackSegmentWriter::commit_native_segment`. | Real decoding, clock alignment, durability failure injection, pause/shutdown and device qualification remain open. |
| Mouse telemetry | `MouseHookMac.swift` and `telemetry/native.rs` implement a bounded v2 transition stream. Z1 reads v1/v2 JSONL and generates source-anchored zooms; Z2 persists accepted/manual keyframes in `project.json` and applies the same evaluator UV crop in preview and export. **H2 software slice is `contract-tested`:** deny/revoke/overflow/shutdown emit typed gaps, permission IPC is a boolean pair, 100 ms geometry uncertainty is preserved, and empty/denied telemetry does not fabricate zooms. | Live Input Monitoring grant/deny/revoke on a signed app, mixed-DPI/negative-origin window transforms, and click-to-frame alignment remain **unqualified**. Do not mark H2 `hardware-qualified`. No cursor replacement or keyboard capture. |
| Native preview and export | F1 overlay, F2 WGPU/VideoToolbox interop, and E4 H.264/AAC export are contract-tested. FFmpeg is not pinned. | Rec.709/zero-copy; live window qualification; 1080p export; Windows media adapters. |
| Windows | Planned native adapters (`WGCBridge.cpp`, WASAPI). | Windows x64 feasibility and vertical slice; ARM64 qualification separately. |

Keep short independent media segments as the baseline. Change storage format, preview transport, or platform encoder strategy only through a recorded decision with measurements, recovery implications, compatibility, and acceptance tests. Do not treat unmeasured concerns about file counts, webview embedding, or keyframe overhead as proof of failure.

---

## 2. Technical Architecture & Component Breakdown

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                             FRONTEND (TAURI V2)                             │
│       React + TypeScript + Vite + Tailwind CSS + Lucide Icons               │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │     Top Navigation Bar & Scene Switcher Card ([Record] ↔ [Edit])      │  │
│  ├───────────────────────────────────┬───────────────────────────────────┤  │
│  │        RECORD SCENE               │        EDIT STUDIO SCENE          │  │
│  │  - Physical Device Control Deck   │  - Playback Video Player Preview  │  │
│  │    (Real Displays, Mics, Webcams) │  - Full-Height Multi-Track        │  │
│  │  - Dynamic Canvas (16:9, 9:16...) │    Timeline (Screen/Cam/Mic/Sys)  │  │
│  │  - Floating Recording Dock        │  - AI Silence Detection Cuts      │  │
│  │  - Canvas & Camera Inspector      │  - Smart Zoom Keyframe Inspector  │  │
│  └───────────────────────────────────┴───────────────────────────────────┘  │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ IPC (Tauri Commands & Events)
┌──────────────────────────────────────▼──────────────────────────────────────┐
│                            CORE ENGINE (RUST)                               │
│  ┌────────────────────┬────────────────────┬─────────────────────────────┐  │
│  │ Session & Project  │ Telemetry Engine   │ Silence Detection & DSP     │  │
│  │ Manager (Bundler)  │ (Normalizer/Sync)  │ (Rust DSP / FFmpeg Audio)   │  │
│  └────────────────────┴────────────────────┴─────────────────────────────┘  │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │ Post-Processing Export Engine (WGPU / Metal / Direct3D / FFmpeg)      │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ FFI / C-ABI Bridges
        ┌──────────────────────────────┴──────────────────────────────┐
        ▼                                                             ▼
┌──────────────────────────────┐              ┌──────────────────────────────┐
│     MACOS NATIVE CORE        │              │    WINDOWS 11 NATIVE CORE    │
│  (Swift / Objective-C++)     │              │     (C++20 / WinRT)          │
│  - ScreenCaptureKit (SCKit)  │              │  - Windows.Graphics.Capture  │
│    (Display/Window/App/Audio)│              │    (Direct3D11 / DXGI)       │
│  - AVFoundation (Webcam/Mic) │              │  - Media Foundation          │
│  - VideoToolbox (HW Enc)     │              │  - WASAPI (Loopback + Mic)   │
│  - CGEventTap (Mouse Events) │              │  - SetWindowsHookEx (Mouse)  │
└──────────────────────────────┘              └──────────────────────────────┘
```

---

## 3. Detailed Subsystem Specifications

### 3.1 Native Capture Pipelines

#### A. macOS Engine (Apple Silicon / ARM64)

* **Screen Capture**: `ScreenCaptureKit` (`SCStream`), with macOS 13 as the supported minimum. Gate newer APIs by OS availability; do not imply support for macOS 12.3.
  * Retain `CVPixelBuffer` / IOSurface-backed frames and use Metal texture views where supported. Measure required conversion and encoder copies; zero-copy is an optimization, not a cross-platform guarantee.
  * Exclude the AeroShoot app & camera preview HUD from the captured screen stream using `SCContentFilter`.
  * Native display, specific window, and custom rectangular crop selection.
* **System Audio**: Built directly into `ScreenCaptureKit` via `SCStreamOutput` audio sample buffers (`SCStreamDelegate` handles lifecycle/errors) (eliminates the need for virtual audio drivers like BlackHole).
* **Webcam Capture**: `AVCaptureSession` capturing raw YUV/NV12 frames from built-in FaceTime HD / Studio Display / external USB-C cameras.
* **Microphone Audio**: `AVCaptureDevice` or `CoreAudio` with customizable sample rate (48 kHz default).
* **Hardware Encoding**: Apple `VideoToolbox` (`VTCompressionSession`) produces encoded H.264 packets for the initial recording format. A separate muxer writes fMP4. Probe hardware availability and concurrent screen/webcam encoder capacity; HEVC and ProRes are later, capability-gated options.
* **Existing bridge integration**: The active `ActiveRecorder` / `RotatingMediaWriter` path in `AeroShootCapture.swift` rotates AVAssetWriter containers and submits finalized temporary files to Rust for publication and journaling. Older prototype code remains in the same file; follow the exported `aeroshoot_macos_start` call chain before editing. The rotation strategy and common clock/shutdown contracts still need hardware qualification. A hardware encoder probe alone does not prove the production path or concurrent encoder capacity.
* **Permitted alternative to the baseline encoder/mux split**: Evaluate `AVAssetWriter` segment-output delegates and `preferredOutputSegmentInterval` if useful. Apple supports segment output without requiring a writer restart for every segment. An adapter must still attach appropriate initialization metadata, prove independent keyframe starts, preserve PTS/DTS, and submit completed data through the shared commit protocol. Select one production adapter after measurement; do not maintain competing persistence paths.

#### B. Windows 11 Engine (x86_64 / ARM64)

* **Screen Capture**: `Windows.Graphics.Capture` (WGC) backed by Direct3D11 GPU textures. Avoid CPU staging readback on the normal recording path; retain/copy GPU resources with explicit synchronization as required by the frame pool and encoder.
  * Native window/display capture. Border suppression requires supported APIs, applicable packaging/capability configuration, and successful user consent via `GraphicsCaptureAccess.RequestAccessAsync(Borderless)`; retain the system border if unavailable or denied.
  * Handle resize, minimized/closed windows, display removal, rotation, and per-monitor DPI explicitly. Desktop Duplication captures a display and is not an equivalent fallback for isolated window capture; never silently broaden the capture scope. Stop or ask for source reselection when the selected source becomes unavailable.
* **System Audio**: `WASAPI` loopback mode (`AUDCLNT_STREAMFLAGS_LOOPBACK`) on the default playback endpoint.
  * Test startup with no render streams, prolonged silence, audio returning, and endpoint loss. No packets must never freeze the session clock. Use device/QPC timing to distinguish valid silence from loss or a discontinuity; record explicit gaps and render silence through the shared timeline, or generate precisely timestamped PCM silence when the storage contract requires it. A background silent-render stream is a measured compatibility fallback, not a universal requirement. Never count missing packets as elapsed-time zero or silently change endpoints.
* **Webcam Capture**: Media Foundation source reader for native camera frames; do not add a parallel DirectShow/C# stack initially.
* **Microphone Audio**: `WASAPI` capture mode on the active input endpoint.
* **Hardware Encoding**: `Media Foundation` hardware MFTs discovered at runtime. Vendor encoder SDKs are separate optional integrations, not assumed MFT features. Qualify Windows ARM64 separately, including native dependencies, drivers, and encoder availability. Offer tested software encoding at reduced settings only after reporting the performance impact.

---

### 3.2 Mouse Telemetry & Event Logging Engine

To achieve the "Screen Studio" post-processing zoom effect, cursor movement must be recorded out-of-band as metadata rather than baked into the pixels:

#### Telemetry contract (`telemetry/events.jsonl`)

* Record append-only, versioned events with integer session-relative microseconds (`t_us`), a sequence number, and `geometry_id`. Persist changes to source geometry separately, including physical pixel bounds, logical-to-physical transform, crop, rotation, content rectangle, and output dimensions.
* Store source-relative normalized coordinates without clamping off-source events; include `inside_source` and visibility. Recalculate transforms when a window moves/resizes or crosses monitors. A single initial `scale_factor` is insufficient.
* Separate movement, button transitions, scroll events, and cursor-shape changes. A click is an event, not a cursor type. Coalesce high-frequency movement under load, but preserve button transitions; record a telemetry gap if the queue overflows.
* Disable the OS cursor in screen capture when recording a separately rendered cursor. Capture cursor image/shape IDs, hotspot, visibility, and timestamp, using platform adapters; draw cursor and click effects inside the screen transform before webcam compositing.
* If separate cursor capture or telemetry permission is unavailable, select an explicit baked-cursor mode and disable cursor replacement. Record the mode in the manifest to prevent double cursors.
* macOS: use a listen-only event tap for required mouse events, with permission preflight for the chosen API and OS. Do not assume Accessibility is universally required for basic cursor position. Windows: use `WH_MOUSE_LL` on a dedicated message-pump thread; callbacks enqueue small records and return immediately.

#### Event payload and platform requirements

* The implemented v2 telemetry subset uses a tagged payload for movement, button transitions, scroll and gaps. The full target schema also includes cursor changes: `move`; `button_down`/`button_up` with a button identifier (including auxiliary buttons); `scroll` with signed X/Y deltas and explicit units; `cursor_changed` with an asset/shape reference and hotspot; and `gap` with reason and affected time interval. Preserve platform scroll phase/precision when available. Modifier state is optional and must distinguish unknown from an empty set.
* Button transitions are authoritative. Clicks and double-clicks are derived annotations with references to their originating transitions; consumers must not animate both a derived click and its source transition as separate clicks. Maintain held-button state and reset it to unknown across telemetry gaps until resynchronized.
* Version payload changes explicitly. Existing v1 `Click` records have unknown button/derivation provenance; do not invent it during migration. Validate geometry and cursor references, and bound cursor asset sizes and cache memory.
* macOS permission preflight distinguishes listening/Input Monitoring from event modification/Accessibility. Use the listening-access APIs where applicable and test the actual event mask, tap location, signed app, and supported OS versions. Handle timeout/user-input tap-disable notifications: record the gap, revalidate authorization, and re-enable only where appropriate; do not repeatedly prompt or restart after an explicit denial.
* `NSCursor.current` is application-local; `currentSystem` was documented as system-wide but is deprecated and is not a dependable cross-version foundation. Qualify public cursor extraction on supported versions. If reliable extraction is unavailable, use the baked-cursor mode already defined above. Do not assume standard global cursor-shape identification is guaranteed, and do not use private cursor APIs.
* Windows `MSLLHOOKSTRUCT.pt` is documented as per-monitor-aware screen coordinates. Define the conversion to source physical pixels once and test negative desktop origins, rotation, mixed DPI, and moved windows. The hook does not provide stationary cursor-shape notifications; use a bounded cursor polling adapter or another qualified mechanism. Keep callbacks immediate, account for silent timeout removal, and report telemetry-health uncertainty rather than promising reliable removal detection. Do not hardcode an assumed 200 ms timeout.

Legacy v1 example (do not use this shape for new native v2 events):

```json
{"version":1,"seq":42,"t_us":1042000,"geometry_id":"g1","kind":"move","norm_x":0.4521,"norm_y":0.7832,"inside_source":true,"visible":true,"cursor_id":"arrow-1"}
```

---

### 3.3 Storage Architecture: Non-Destructive Project Bundles

Keep immutable source media separate from editable project state. A `.aero` project is a directory during capture/editing; any archive packaging is a later explicit operation.

```text
Project_Session_[UUID].aero/
├── manifest.json             # Schema version, track IDs, formats, time mapping, status
├── project.json              # Revisioned timeline, source intervals, styling, keyframes
├── journal.jsonl             # Append-only committed segment/discontinuity records
├── telemetry/
│   ├── events.jsonl          # Incrementally recoverable event stream
│   ├── geometry.jsonl        # Timestamped source geometry revisions
│   └── cursors/              # Cursor images and hotspots
├── media/
│   ├── screen/000001.mp4     # Independent H.264 fMP4 segments
│   ├── webcam/000001.mp4     # Independent H.264 fMP4 segments
│   ├── system/000001.wav     # Bounded PCM WAV segments, initially 48 kHz
│   └── mic/000001.wav        # Bounded PCM WAV segments, initially 48 kHz
└── cache/                    # Regenerable proxies, thumbnails, waveforms
```

* Target short video segments (initially 2 seconds), each with initialization metadata and an independent starting keyframe. Encoders produce packets; the muxer preserves PTS/DTS and writes fragments. WAV contains PCM, never mislabeled AAC. Short WAV segments avoid long-session RIFF size limits.
* Commit order: finish and flush segment data, make it durable using platform file APIs, atomically rename its temporary file within the project filesystem, then durably append its index entry. Checkpoint manifest/project snapshots via temporary file + atomic replacement, retaining a prior valid revision. Define and test platform/filesystem durability behavior.
* Recovery scans journal and actual media, ignores partial log records, validates complete fragments, and rebuilds indexes. Salvage a truncated tail only after demux validation. Mark per-track gaps and actual recoverable duration; fragmentation alone does not guarantee the last second survives power loss.
* Use a single writer lock per project, schema-version checks and explicit migrations, disk-space preflight and monitoring. On disk exhaustion, stop capture and preserve committed segments. Caches can be deleted; source media cannot be overwritten by edits or export.

#### Segment, durability, and recovery contracts

* Two seconds is a target duration, not fabricated timestamp metadata. At exactly 30 fps and 48 kHz, two seconds contains 60 video frames and 96,000 PCM sample frames. Capture buffers need not align with file boundaries: split PCM by complete sample frames and retain rational video/audio timebases. Neither buffer boundaries nor separate files inherently cause drift. Record actual boundaries for variable frame rates and other frame-rate ratios.
* Finalize RIFF/WAVE chunk sizes and format metadata before commitment. Do not assume the data-size field is always at byte 40; parse chunks and padding. An incomplete temporary WAV tail may be salvaged only with validated format metadata and complete sample-frame alignment into a new committed segment. Raw PCM is an optional explicit format decision, not a prerequisite for crash recovery.
* Budget open files and decoder instances independently of stored file count. Four tracks at two-second targets produce approximately 7,200 files/hour, but readers must open only the active working set plus a bounded cache. Measure commit latency, directory scan time, seek/export cost, memory, power, and keyframe bitrate overhead on qualified filesystems before changing segment duration or adopting single-file storage.
* New-project creation must be exclusive, without a check-then-create race. Hold an OS-backed exclusive project lock for capture, editing, and recovery, including competing writers in the same process. Process termination must release ownership safely; a PID text file alone is insufficient. Read-only inspection must not mutate a live project.
* Allocate segment IDs without reusing existing committed or temporary destinations. Create temporary files exclusively, reject symlinks/reparse-point escapes, and commit without overwriting a destination. A fresh writer or journal retry must never replace source media. Validate track IDs before constructing paths.
* Synchronize data and required directory/name metadata before acknowledging commitment. Specify and test the platform file APIs for macOS/APFS and Windows/NTFS; document weaker guarantees or reject unsupported destinations. Atomic visibility is not equivalent to power-loss durability. Storage failures propagate to an explicit recoverable result; never swallow finalization or journal errors and report completion.
* Snapshot replacement retains a previously validated revision. Temporary and backup paths must be safe against symlinks, collisions, and concurrent writers. Propagate backup/write/sync failures. Recovery may use a validated backup if the primary is corrupt; it must reject unsupported schema versions instead of silently inventing a new project.
* The recovery entry point itself tolerates an incomplete final journal record under exclusive ownership. Distinguish a truncated tail from corruption in committed interior records. Repair before further appends, preserve sequence continuity, and use bounded reads rather than loading an unlimited journal into memory.
* Validate referenced and discovered files through the same path-containment checks before opening them. Bound manifest/log/asset/media parsing; validate MP4 initialization, box lengths, sample references, PTS/DTS, and independent segment decodability, and WAV format/data sizes and sample alignment. A recognizable header or matching byte count is not proof of valid media. Use actual decodable fixtures and demux/decode checks.
* Recover renamed-but-unjournaled segments from validated media timing and persisted clock mappings. Never infer duration as file count multiplied by two seconds. If placement is ambiguous, preserve the file as unresolved rather than fabricate alignment. Persist a reconciled index with track IDs, segment paths, source intervals, format revisions, pause intervals, and explicit gaps; rerunning recovery must be idempotent. Reconcile an unmatched pause start to the last recoverable source boundary without extending duration to recovery wall time.

---

### 3.4 The Floating Camera HUD (Real-Time Overlay)

* A secondary, frameless, transparent Tauri webview window.
* Properties:

  * `always_on_top: true`
  * Draggable around the desktop.
  * Shape toggles: **Circle**, **Squircle / Rounded Rectangle**, **16:9 Standard Rectangle**.
  * Configurable borders, shadows, and mirroring toggle.
  * Excluded through `SCContentFilter` on macOS and `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on owned Windows HUD windows. Verify exclusion on supported configurations; hide HUD windows during recording if exclusion cannot be established. Do not promise unconditional exclusion.
* **Preview transport**: Attach a native Metal/Direct3D rendering surface to the HUD window; Tauri provides controls and styling parameters. The native camera session fans out to encoder and a bounded, latest-frame preview queue. Do not reopen the camera in browser `getUserMedia` or serialize raw frames over IPC. Prototype transparent surface placement, clipping, and window ownership in Phase 0; fall back to a native HUD if webview embedding fails.
* **Window identity**: Explicitly map the `main` label to the studio and `camera_overlay` to a HUD-only root before mounting components. An explicit packaged-safe URL such as `index.html#overlay` may reinforce this identity. Reject mismatched or unknown privileged window identities. The HUD must not mount the complete studio or initialize an independent recording session.
* **State synchronization**: Rust owns the camera/HUD settings revision. Each window subscribes to revisioned events and obtains a snapshot on startup/reconnect; stale commands are rejected and missed revisions trigger resynchronization. Zustand holds a local view of this state, not a separate authority. Do not synchronize frame buffers through these events.
* **Hit testing**: Qualify whether transparent corners intercept clicks. Provide platform hit testing or a tested supported shape so intended transparent regions pass input through while drag handles and controls remain usable. Test keyboard focus, dragging, resizing, clipping, alpha blending, and capture exclusion together on each platform.
* **Fallback ownership**: If native surface embedding fails its gate, use a native HUD presentation adapter with the same Rust settings and command contracts. Platform-specific drawing/hit testing is acceptable; duplicated session logic and independent settings authorities are not.

---

### 3.5 Timeline Studio & Visual Editor

The editor uses React and Canvas for timeline controls, with a native media preview surface. One Rust timeline evaluator and WGPU scene compositor drive both preview and export; do not implement independent WebGL and native renderers with divergent styling or keyframe behavior. Native decoders feed bounded frame queues; one audio playback clock drives preview, and seeking cancels stale decode work. Proxies, thumbnails, and waveforms are background, cancelable caches.

#### Preview surface contract and feasibility decision

* Baseline: a platform adapter owns an AppKit view/Metal surface or a Windows rendering surface associated with the studio window. Tauri/React provides controls and sends throttled viewport geometry (logical rectangle, backing scale, visibility, clip, and revision); the adapter owns physical sizing, z-order, focus, and surface lifetime. A native view is not a DOM element. Select the actual child-view/underlay arrangement through the Phase 0 spike, not an assumption about webview internals.
* WGPU composites the preview directly into this surface. The HUD receives a bounded latest-frame camera feed from the same native capture session. Raw pixels never enter ordinary Tauri command/event serialization. React preview controls do not imply that an HTML video element must display the pixels.
* Test live resize, inspector/timeline changes, moving across DPI boundaries, minimize/restore, occlusion, transparency, and window destruction during playback. Record layout-to-surface update latency and rendering artifacts, and define pass thresholds in the experiment before running it.
* If the baseline fails, evaluate a separate native preview window first. An encoded preview stream delivered over a scoped custom protocol is a separate fallback requiring a recorded architecture decision: preserve the single compositor, bound queues, validate WebKit/WebView2 codec support, quantify encode/decode latency and copies, and secure project access. A custom URL scheme alone does not make raw images playable by an HTML video element. Loopback HTTP adds authentication/origin/lifecycle obligations and is not the default.

Editor features:

1. **Multi-Track Timeline**:
   * Screen track with auto-generated thumbnail strips.
   * Webcam track (can be toggled, resized, styled, repositioned at keyframes).
   * Audio tracks with interactive waveform visualizers (RMS & peak energy).
2. **Auto-Zoom & Pan Engine**:
   * Algorithm analyzes the telemetry event log to find periods of user interaction, mouse dwell times, and clicks.
   * Generates default smooth cubic Bézier camera pans/zooms (e.g. 1.5x, 2.0x).
   * User can manually slide, extend, zoom in/out, or delete any zoom keyframe block.
3. **Canvas Customizer**:
   * Background wallpaper: solid colors, modern gradients, blurred desktop background.
   * Window frame styling: adjustable corner radius (0px to 32px), drop shadow blur/opacity, canvas aspect ratio (16:9, 4:3, 9:16 for shorts/TikTok).
4. **Webcam Bubble Customizer**:
   * Position (any corner, custom coordinate, or side-by-side split screen).
   * Size scaling (small, medium, large, full).
   * Shape selection and border color/width.

---

### 3.6 Silence Detection & Smart Jump Cuts

* **Scope**: Energy-based silence detection is deterministic DSP, not filler-word recognition. Transcription, word timestamps, and model selection are a separate later feature with explicit local/cloud processing choices. No media upload by default.
* **Silence Detection Engine**:
  * Analyzes the microphone or master audio track via RMS thresholding (e.g., `-38 dBFS`) and minimum duration (e.g., `> 400ms`).
  * Implemented in Rust with a correct scalar reference; enable SIMD only after equivalence tests and measurements. Iterator code alone is not evidence of SIMD acceleration.
  * Define input layout, channel count, sample rate, channel selection, and source offset explicitly. The existing detector takes mono samples; callers must enforce that contract until multichannel support is added. For multichannel analysis, compute per-channel energy and use a documented aggregation policy (initially, silence only when every selected channel is below threshold), avoiding cancellation from signed downmixing. Squaring interleaved samples does not itself cause phase cancellation, but treating interleaved scalar samples as mono produces wrong durations.
  * Compute times from sample-frame indices, not scalar sample counts across channels. Test opposite-polarity stereo, one silent channel, channel layouts, short tails, invalid rates/configurations, background noise, and padding near edit boundaries. A user-adjustable fixed threshold is the baseline; adaptive noise-floor tracking is an optional evaluated enhancement, not a prerequisite for correctness.
* **User Post-Process Controls**:
  * **Silence Threshold Slider** (`-60 dB` to `-20 dB`).
  * **Min Silence Duration Slider** (`200ms` to `1500ms`).
  * **Padding Buffer** (keeps `50ms` before and after speech to avoid clipping syllables).
  * **Preview & Select**: Highlights detected silence blocks on the timeline. The user can review, uncheck specific blocks, or click "Apply Cuts" to ripple-delete across screen, webcam, and audio tracks simultaneously.

---

### 3.7 Post-Processing Export Engine

* **Initial format**: MP4 with H.264 video and AAC audio, SDR Rec.709. HEVC in MP4 and ProRes in MOV are later options with encoder/container capability checks. Resolution and frame rate are negotiated, not guaranteed at 4K60 on every target.
* **Decode → evaluate → composite → encode → mux**: Decode required source intervals, evaluate the shared timeline at exact rational output frame times, render background → transformed screen and cursor → webcam → overlays, then encode and mux. Preserve aspect ratio and define crop/letterbox behavior.
* **Color contract**: Store source primaries, transfer, matrix, range, rotation, and pixel aspect metadata. Initially request/convert to SDR Rec.709, use a defined linear-light compositing space and premultiplied alpha, and produce tagged encoder output. HDR sources require explicit tone mapping or an unsupported-mode error; HDR export is deferred.
* **GPU interoperability**: WGPU is the shared compositor. Validate native decoder/encoder texture interchange on Metal and Direct3D before relying on it; allow a bounded, measured copy/conversion fallback. Encoder adapters own platform resources and synchronization.
* **Audio**: Apply the same source-time mapping as video to both audio tracks, resample/mix, apply short boundary fades, and optionally normalize loudness with a true-peak limit. Re-encode at arbitrary cut boundaries rather than relying on keyframe-only stream copying.
* **Jobs**: Export uses an immutable project revision, progress/cancel support, bounded queues and an output temporary file. Finalize and atomically rename on success. Never overwrite source tracks. Throttle or defer export while recording.
* **Media dependencies**: Use a pinned FFmpeg/libavformat build for mux/demux and required decode/audio operations, with native hardware encoder adapters. Document build flags, included codecs, distribution licenses/notices, and architecture-specific binaries before packaging; do not assume a system FFmpeg install.
  * Define a common decoder adapter returning timestamped frames with explicit ownership. Platform hardware decoders are preferred where qualified; FFmpeg software/hardware-backed decoding can implement the same native contract. “Native” means outside the webview, not an exclusion of FFmpeg. Interop and bounded-copy fallbacks must be measured for the selected backend.
  * Record the exact FFmpeg configuration, dependencies, license combination, source delivery, notices, and applicable relinking/replacement obligations for the distribution method. Static and dynamic linking have different compliance work; GPL/nonfree options require explicit review before enabling. Package and test signed libraries for each supported architecture; licensing, code signing, notarization, and codec patents are separate checks. Do not assume LGPL compliance from the library name or automatic GPL conversion merely from static linking.

### 3.8 Session Clock, Timeline, and Cut Semantics

* Establish one monotonic session epoch **before** starting capture. Map each native source timestamp/timebase to that epoch, preserving original timestamps and mapping metadata. Never substitute callback arrival time for media PTS. For macOS bridge media clocks to host time; for Windows map WGC timestamps and WASAPI device/QPC positions to the session clock.
* Persist integer microseconds for project/telemetry times and retain rational codec timebases and audio sample counts. Detect discontinuities and estimate audio clock drift over long sessions; use bounded adaptive resampling for playback/export alignment, preserving original source data and corrections in metadata.
* Choose fixed encoded dimensions per recording track; fit resized source content into that canvas and persist its active content rectangle for telemetry transforms. If codec parameters must change, open a new segment and record its format revision. Variable-rate capture retains actual timestamps; constant-rate export samples the timeline explicitly.
* Late-starting or disconnected tracks have explicit gaps, not an invented zero offset. Screen gaps hold the last valid frame (black if none); webcam gaps hide the bubble; missing audio becomes silence. Source closure stops screen capture recoverably; camera/mic loss can continue with a visible warning. Do not silently switch audio endpoints or capture sources.
* State machine: `Idle → Preparing → Recording → Paused → Recording → Stopping → Completed`, with explicit failed/recoverable outcomes. Commands are serialized and start/stop are idempotent. Pause retains the monotonic source clock and records a common excluded interval; resume opens new segments. Sleep/lock interrupts capture and requires explicit resume after revalidation. Stop drains callbacks/encoders before closing muxers.
* Timeline uses sorted, non-overlapping, half-open retained source intervals `[start_us, end_us)`. Edited time maps to source time through their cumulative lengths. Every video, audio, telemetry event, and source-anchored zoom keyframe uses that same mapping. Cuts never rewrite source files. Define interpolation at cuts to avoid panning through removed material; keep edits reversible with undo/redo.
* **Zoom boundary policy**: Initially treat each cut as an intentional visual discontinuity. Restrict the original source-anchored Bézier curve to each retained interval, preserving its evaluated value and derivatives within that interval; do not interpolate across removed source time. At the edited cut boundary, select the next retained interval under half-open semantics. An optional later smoothing transition must have an explicit edited-time duration and policy shared by preview/export; never silently reparameterize the whole curve. Test cuts inside zooms, at keyframes, and across geometry changes, including undo/redo.
* **Session serialization**: One command executor/lock covers validation, resource mutation, state transition, and response caching. Stable session IDs and request/revision IDs govern retries. Publish a recording snapshot only after its resources are installed; repeated Start must not create competing sessions. Preparation/finalization failures retain recoverable project ownership and diagnostics. Stop succeeds only after native shutdown acknowledgment, writer completion, and persistence checks.
* **Clock anchors**: Use native clock-to-host correlations with retained rational anchors. Do not initialize per-track offsets from the time an asynchronously queued callback happens to run. Preserve late starts and drift corrections in the manifest/index so recovery does not depend on process memory or queue scheduling.

### 3.9 Ownership, Backpressure, and IPC

* Rust owns sessions, project revisions, jobs, and persistence. React holds view state and submits revision-checked commands; multiple windows subscribe to snapshots/events and cannot independently mutate a recording session.
* Native FFI uses opaque handles, explicit retain/release ownership, versioned structures, error codes, and a shutdown handshake. No Rust panic or native exception crosses the C ABI. Callbacks must stop before resources are freed.
  * Shutdown requests stop, prevents new callback admission, drains or cancels outstanding work, joins workers/acknowledges native completion, then releases resources. Setting a Boolean is not a drain acknowledgment. Timeouts and writer completion errors are surfaced; handles remain valid until safe release. Exercise teardown while callbacks, previews, and stop retries are in flight.
* Capture callbacks enqueue references into bounded queues and never wait for disk, encoding, rendering, or UI. Saturated video queues drop frames with timestamp/gap counters; audio overflow creates an explicit discontinuity and error status. Sustained overload triggers a controlled stop, not unbounded allocation.
* IPC carries commands, metadata, throttled meters, and job status. All raw media stays in native/Rust processing. Scope Tauri capabilities per window; validate project paths and resource IDs, restrict local media access to the opened project, and disallow arbitrary shell execution or remote navigation in privileged windows.
  * Test actual serialized Rust/TypeScript requests, responses, enum tags, optional fields, and errors. Browser fixtures must obey the same contracts and identify synthetic mode visibly. Use a defined CSP, least-privilege per-window capabilities, and a project-scoped media protocol. UI timers/meters consume authoritative session snapshots; unsupported capture settings or permissions produce actionable status rather than fabricated devices, success, or media.

---

## 4. Planned Repository Structure

```text
AeroShoot.AI/
├── .github/
│   └── workflows/                # Cross-platform CI/CD (macOS ARM & Windows 11)
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── src/
│   │   ├── main.rs               # Application entrypoint
│   │   ├── commands/             # Tauri IPC commands
│   │   │   ├── capture.rs        # Start/stop/pause recording
│   │   │   ├── project.rs        # Project bundle management
│   │   │   ├── telemetry.rs      # Mouse tracking controller
│   │   │   ├── dsp.rs            # Silence detection & audio cuts
│   │   │   └── export.rs         # Render & export triggers
│   │   ├── capture/              # Platform abstraction layer
│   │   │   ├── mod.rs            # Common traits: ScreenCapturer, AudioCapturer
│   │   │   ├── macos.rs          # FFI bindings to macOS native framework
│   │   │   └── windows.rs        # FFI bindings to Windows native framework
│   │   ├── session/              # State machine, clock mapping, bounded queues
│   │   ├── project/              # Versioned schema, segments, journal, recovery
│   │   ├── telemetry/            # Mouse events, geometry, cursor assets
│   │   ├── timeline/             # Shared time mapping and keyframe evaluator
│   │   ├── media/                # Decode, resample, encode, mux adapters
│   │   ├── render/               # Shared WGPU compositor and native surfaces
│   │   ├── playback/             # Audio-clock scheduling and seeking
│   │   └── export/               # Export job orchestration
│   └── native/
│       ├── macos/                # Swift/Objective-C++ ScreenCaptureKit wrapper
│       │   ├── Package.swift
│       │   ├── SCKitBridge.swift
│       │   └── MouseHookMac.swift
│       └── windows/              # C++20 WinRT Windows Graphics Capture wrapper
│           ├── CMakeLists.txt
│           ├── WGCBridge.cpp
│           └── MouseHookWin.cpp
├── front-end/
│   ├── package.json
│   ├── vite.config.ts
│   ├── tsconfig.json
│   ├── tailwind.config.js
│   └── src/
│       ├── main.tsx
│       ├── App.tsx
│       ├── components/
│       │   ├── navigation/       # TopNavBar (Scene switcher card, badges)
│       │   ├── scenes/           # RecordScene, EditStudioScene
│       │   ├── recording-hud/    # DeviceControlDeck, RecordingFloatingDock, RecordingHUD
│       │   ├── canvas/           # StudioCanvas (Responsive aspect ratios 16:9, 9:16, 4:3, 1:1)
│       │   ├── camera-overlay/   # CameraOverlay (Movable bubble, 16:9 aspect, mirror decoupled)
│       │   ├── timeline/         # TimelineStudio (Multi-track, waveforms, zoom keyframes)
│       │   ├── inspector/        # InspectorPanel (Wallpapers, shadows, padding, camera styling)
│       │   └── silence-modal/    # SilenceModal (Threshold, min duration, ripple cuts)
│       ├── stores/
│       │   ├── projectStore.ts   # Zustand state for timeline & keyframes
│       │   └── settingsStore.ts  # Device selections, activeScene, quality preferences
│       └── lib/
│           ├── ipc.ts            # Resilient native bridge & real-device discovery
│           └── types.ts          # Studio type definitions
└── AEROSHOOT_MASTER_PLAN.md      # This persistent master blueprint
```

---

## 5. Phased Implementation Roadmap

### Phase 0: Architecture Feasibility Gates

* Pin toolchains and one React major; define platform capability reports and a device/OS qualification matrix. macOS 13+ ARM64 and Windows 11 x64 are first targets; Windows ARM64 remains a planned target until independently qualified.
* Build minimal native spikes on **both** platforms: screen + webcam hardware encoding concurrently, system/mic timestamps, cursor exclusion, and HUD exclusion.
* Prove native preview surface embedding and WGPU-to-encoder interoperability. Select and document bounded-copy fallbacks where necessary.
* Exit gate: short synchronized recordings and a styled preview/export on both systems, with measured queue memory, copy cost, encoder availability, and dependency packaging feasibility. Resolve failed gates before building the full editor.
* **Evidence**: Check in a reproducible procedure and result record for each native experiment. Include native surface placement and transparent-corner hit testing, identical preview/export scene evaluation, four-track timestamp alignment, cold permission denial, and forced termination. Select the encoder/muxer and preview adapters explicitly. Missing platform hardware is an open gate, not a pass.
* Shared Phase 1 contracts and synthetic tests may proceed while feasibility experiments are open. Production editor implementation and claims of native qualification depend on the applicable Phase 0 gates; a synthetic studio shell does not satisfy them.

### Phase 1: Shared Recording Foundations & Studio Shell (Implemented; Acceptance Gaps Remain)

* Initialized Tauri v2, React/TypeScript/Vite, Tailwind CSS and Zustand, with least-privilege window capabilities.
* **Studio GUI Overhaul**:
  - Implemented clean top navigation bar with a segmented **Scene Switcher Card** (`RecordScene` vs `EditStudioScene`).
  - Built physical-only **`DeviceControlDeck`**: real displays, real webcams (with first-class "No Camera" option), and real microphones (with live animated VU meter), completely eliminating mock devices.
  - Added hardware **hotplug listener** (`devicechange` on `navigator.mediaDevices`) for dynamic device sync.
  - Implemented responsive **`StudioCanvas`** supporting immediate switching between `16:9`, `9:16` (Reels/Shorts), `4:3`, and `1:1` formats.
  - Built non-overlapping **`RecordingFloatingDock`**, **`CameraOverlay`** (with decoupled video mirroring and exact 16:9 sizing), **`InspectorPanel`**, and full-height **`TimelineStudio`**.
* Implemented clock/timebase contracts, session state machine, native ownership interfaces, bounded queues, manifest schema, segment writer, journal, and recovery in Rust.
* **Exit gate**: Shared acceptance matrix tests pass; frontend compiles with zero warnings/errors (`tsc && vite build`).

### Phase 2: macOS Vertical Slice

* Integrate ScreenCaptureKit screen/system audio, AVFoundation camera/mic, VideoToolbox encoding and the shared muxer.
* Wire native HUD preview, permission handling, cursor events and changing source geometry into the common session clock and bundle writer.
* Replace the prototype's independent persistence path with the chosen production adapter. Complete segments must enter the shared no-overwrite commit/journal protocol; audio uses bounded finalized WAV segments. Record native clock mappings, gaps, and cursor mode. Native pause/resume closes/reopens segments, and native stop reports writer failures and timeouts to Rust.
* Exit gate: a 60-minute 1080p30 recording with both audio tracks and webcam; target audio/video skew ≤ 20 ms, bounded queue memory, and reported frame drops. Test forced termination, disk-full, denial/revocation, resize, pause/resume, and device removal. These are acceptance targets, not measured results.

### Phase 3: Windows Vertical Slice & Platform Parity

* Integrate WGC/D3D11, WASAPI loopback and mic, Media Foundation camera/encoder, and cursor/HUD handling using the same contracts.
* Repeat Phase 2 gates on qualified x64 hardware; test mixed DPI, GPU/device loss, endpoint changes, border consent, and minimize/close behavior. Qualify ARM64 separately before shipping that binary.
* Include silent endpoint startup, silence-to-audio return, explicit silent buffers, timestamp discontinuities, stationary cursor changes, and mouse-hook overload. Compare physical/source coordinates across monitors without double-applying DPI scaling.

### Phase 4: Shared Preview, Timeline & Basic Export

* Implement native decode/playback, the source-to-edited-time evaluator, WGPU compositor, and MP4 H.264/AAC export before advanced styling.
* Add cached thumbnails/waveforms, multi-track timeline, reversible trims, webcam layout, cursor rendering, and basic background styling.
* Exit gate: seek/cut fixtures align all tracks; representative preview frames match exported frames within a documented codec/color tolerance; export cancellation preserves sources and previous outputs.
* Use a bounded decoder/file working set for long projects and report peak open descriptors and seek latency. Verify missing-track gap behavior and the explicit zoom discontinuity policy at every cut. GUI controls must render the opened project rather than substitute sample content.

### Phase 5: Smart Editing & Advanced Styling

* Add telemetry-generated zoom trajectories, editable keyframes, canvas and camera styling, deterministic silence suggestions and reviewed ripple cuts.
* Test cuts through zooms, source geometry changes, silent/missing mic tracks, and undo/redo. Silence padding is user-tunable and suggestions remain reversible.
* Validate mono/multichannel timing, opposite-polarity inputs, channel selection, and fixed-threshold behavior with noise. Add adaptive thresholds or smooth cut transitions only as explicit, tested features with reproducible preview/export behavior.

### Phase 6: Performance Qualification & Distribution

* Benchmark 1080p60/4K modes, encoder contention, long sessions, thermal behavior, memory and disk throughput; enable only supported combinations.
* Add signed/notarized macOS packages and signed Windows installers, dependency notices, CI for shared tests and platform builds, plus hardware smoke tests. OS capture/permission/GPU behavior requires real-device checks beyond hosted CI.
* Defer HDR, ProRes, speech/filler recognition, and vendor-specific encoder SDKs until the baseline is stable and their capability/licensing requirements are resolved.

### Shared Acceptance Matrix

These checks close shared requirements; they do not replace the real-device Phase 0–3 gates. Keep fixtures small and deterministic and assert outcomes through public command/recovery interfaces, not just helper functions.

| Contract | Required cases | Passing evidence |
| --- | --- | --- |
| Build and IPC | Headless Rust tests; desktop-feature builds; frontend build; serialized boundary tests; both window roots | Correct payloads/errors and window identity; no browser-only mock masking a desktop failure |
| Command ownership | Concurrent Start/Stop/Pause/Resume, retries, second session, preparation and stop failures | One active session, stable retry results, consistent snapshots, recoverable failures |
| Clock and timeline | Rational frame rates, late starts, queue delays, clock drift, idle audio, pause/resume, gaps | Queue scheduling cannot change media offsets; sample-frame timing and retained intervals align all tracks |
| Native ownership contract | In-flight callbacks, backpressure, cancellation, repeated stop, shutdown timeout | Bounded references/queues and no free-before-drain; failures reach the session result |
| Storage ownership | Same-process and cross-process writers, restart, destination collision, recovery during capture | Exclusive mutation and no overwritten source or active snapshot |
| Commit failures | Inject failure at write, header finalization, sync, rename, journal append, and snapshot replacement | No false completion; prior committed data remains usable; orphaned complete media is recoverable |
| Recovery | Partial final log line, corrupt interior line, primary/backup damage, renamed unindexed segment, truncated MP4/WAV, unmatched pause | Actual timing/decodability verified; persistent reconciled index; gaps/unresolved media explicit; repeated recovery stable |
| Input security | Absolute/traversal paths, symlinks/reparse points in media and snapshot paths, oversized inputs, unknown versions | No access outside the opened project, bounded resource use, no silent schema rewrite |
| Media fixtures | Complete decodable video/WAV, malformed headers, bad sample ranges, incompatible format changes | Demux/decode and timestamp checks, not only signatures, sizes, or existence |
| HUD and settings | Startup/reconnect, missed/out-of-order events, stale revisions, shape changes | HUD-only UI and convergent settings; no independently mutable session |
| Long projects | Thousands of segments, sustained telemetry, seek/export workload | Bounded memory/open files; reported commit/scan/seek performance; no unmeasured latency claims |

### Immediate Implementation Order & Next Steps

1. **Step 1: Native macOS Recording & Segment Commit Integration (Phase 2)**:
   - **Implementation update (2026-09-08):** Native rotated containers now enter `TrackSegmentWriter::commit_native_segment` as finalized temporary files. Rust validates, syncs, publishes without replacement, syncs the directory, and journals the supplied clock anchor; the synchronous callback returns persistence failure to Swift. Removed Stop’s independent salvage/commit path. Real-device Start/Stop, decoding, synchronization, and long-session qualification remain open; Step 1 is not yet hardware-qualified.
   - Continue from the active `AeroShootCapture.swift` → `capture/macos.rs` → `TrackSegmentWriter::commit_native_segment` integration; qualify it instead of creating another persistence path.
   - Ensure native ScreenCaptureKit screen frames, system audio loopback, webcam frames, and microphone PCM chunks write out verified, timestamped `.mp4` and `.wav` segments into the active `.aero/media/` project bundle.
   - Verify that clicking `Start Recording` in the UI records real media and `Stop Recording` commits the project bundle.

2. **Step 2: Global Mouse Telemetry Capture (`CGEventTap` on macOS)**:
   - **Implementation update (2026-09-09):** H2 software contracts are tested: typed `input_monitoring_unavailable` / `input_monitoring_revoked` gaps, overflow→gap, no callbacks after stop, baked-cursor permission UI that does not block recording, and 100 ms geometry uncertainty. Hardware capability-matrix experiments remain open (see §10.4 H2).
   - **Implementation update (2026-09-08):** Added `MouseHookMac.swift`, bounded native event capture, v2 Rust telemetry persistence, explicit Input Monitoring UI, pause/overflow/permission gaps, and baked-cursor manifest metadata. Native hardware permission/timing and mixed-DPI geometry qualification remain open.
   - Wire `MouseHookMac.swift` using a passive listen-only `CGEventTap` to record mouse position, clicks, and dwell times.
   - Stream normalized coordinates and button transitions into `telemetry/events.jsonl` aligned with the monotonic session clock.

3. **Step 3: Real Media Playback & Waveforms in Edit Studio (Phase 4)**:
   - **Implementation update (2026-09-08):** E1 is implemented and contract-tested. Stop returns a resolvable `projectPath`; `open_project` / `close_project` / `project_segments` load a bounded read-only segment index into the editor. The Edit Studio no longer injects a sample session.
   - **Implementation update (2026-09-08):** E4 is contract-tested for an immutable-revision H.264/AAC export job that uses the same timeline/scene evaluator as preview. Collision, cancel, and source-path policies preserve sources. `previewAvailable` becomes true after a native project frame is presented.
   - Connect the `EditStudioScene` video preview to playback the recorded screen and webcam video tracks from the `.aero` project bundle.
   - Generate true audio waveforms (RMS/peak) on the `TimelineStudio` mic and system audio tracks instead of sample placeholder waveforms.

4. **Step 4: Smart Auto-Zoom Keyframing Algorithm (Phase 5)**:
   - **Implementation update (2026-09-09):** Z1 is contract-tested. `telemetry/reader.rs` reads v1 `kind` and v2 `payload` JSONL; `zoom/` derives interest, clusters with explicit config, and evaluates source-anchored cubic Bézier transforms with the Section 3.8 cut discontinuity. `project_zoom_suggestions` returns suggestions to the Edit Studio timeline.
   - **Implementation update (2026-09-09):** Z2 is contract-tested. Accepted and manual zooms persist in `project.json` with undo/redo; regeneration cannot overwrite existing or dismissed ids. `SceneEvaluator::preview_at` applies the Z1 camera UV crop to the screen layer, so preview and export share the transform.
   - Implement a version-aware telemetry reader and derive interest from supported button transitions and cursor dwell, then generate smooth cubic Bézier zoom keyframes. Current telemetry does not capture typing; do not invent keyboard activity or add keyboard collection as part of this task.
   - Allow user adjustment and preview directly on the canvas.

5. **Step 5: Windows 11 Native Core & Platform Parity (Phase 3)**:
   - Implement `WGCBridge.cpp` with `Windows.Graphics.Capture` and Direct3D11.
   - Implement WASAPI loopback and microphone capture on Windows.

---

## 6. Permissions & Security Protocol

### macOS

* Request Screen Recording permission through the supported OS capture permission flow and handle denial, revocation and any restart requirement. Do not rely on an assumed `NSScreenCaptureUsageDescription` key as authorization.
* Include `NSCameraUsageDescription` and `NSMicrophoneUsageDescription` for the selected camera/mic APIs. Determine Input Monitoring/Accessibility requirements from the actual event API and supported OS versions; request only what that feature needs. Telemetry denial must not prevent ordinary recording.
* Validate signing, hardened runtime and any sandbox entitlements against the chosen distribution channel on real signed builds.

### Windows 11

* Check camera/microphone privacy, capture API support, and borderless consent independently; retain supported capture with its border when consent is denied.
* Set `PerMonitorV2` DPI awareness and validate physical/logical coordinate transforms on mixed-DPI displays.

### Local Data and Project Loading

* Keep recording and analysis local by default. Project bundles contain private screen/audio/input data; do not include media, window titles or telemetry in diagnostic uploads by default.
* Validate schema, paths (including symlink escape), file sizes and media metadata when opening a project. Treat project files as untrusted input; never execute embedded content. Any future cloud speech processing needs an explicit user choice before media leaves the machine.

---

## 7. Instructions for Future AI Agents & Developers

When picking up work on this repository:

1. **Always reference this file (`AEROSHOOT_MASTER_PLAN.md`)** as the source of truth for architectural choices and milestones.
2. **Follow the Phased Roadmap**: Complete the Phase 0 preview/export feasibility spikes early; build production post-processing only after capture, synchronization, and recovery foundations are stable.
3. **Preserve Cross-Platform Isolation**: Keep platform-specific code strictly contained in `src-tauri/native/macos/` and `src-tauri/native/windows/` with clean Rust FFI boundaries in `src-tauri/src/capture/`.
4. **Performance Rule**: Never serialize raw video frames through ordinary Tauri command/event IPC. The webview owns controls and editing UI; native surfaces display the baseline preview. Any encoded webview-preview fallback requires the explicit decision and measurements in Section 3.5. Recording, compositing, and encoding remain in native/Rust workers.
5. **Qualification Rule**: Update status only with linked acceptance evidence. Do not equate scaffolded interfaces, header-only fixtures, passing helper tests, or a synthetic UI with completed native features. Preserve the distinction between observed defects, platform risks, and design alternatives.

## 8. Architecture Review References

Reviewed through 2026-09-08. These sources establish API behavior and constraints; performance, native surface embedding, and recovery durability still require the Phase 0–3 experiments. Recheck API availability and distribution obligations against pinned toolchains before release.

* [Apple: SCStreamOutput](https://developer.apple.com/documentation/screencapturekit/scstreamoutput) — output samples are separate from stream lifecycle callbacks.
* [Apple: ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit) — screen capture and its permission flow.
* [Microsoft: IsBorderRequired](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired) — border suppression is consent/capability dependent.
* [Microsoft: SetWindowDisplayAffinity](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity) — owned-window capture exclusion and its limits.
* [Microsoft: IAudioClock2::GetDevicePosition](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclock2-getdeviceposition) — device position and QPC correlation.
* [FFmpeg: format/muxer documentation](https://ffmpeg.org/ffmpeg-formats.html) — fragmented MOV/MP4 behavior; durability and project recovery need additional application logic.
* [Apple: AVAssetWriterDelegate](https://developer.apple.com/documentation/avfoundation/avassetwriterdelegate) and [Author fragmented MPEG-4 content with AVAssetWriter](https://developer.apple.com/videos/play/wwdc2020/10011/) — segment-output APIs; not proof that the current writer implements the project commit protocol.
* [Apple: NSCursor.currentSystem](https://developer.apple.com/documentation/appkit/nscursor/currentsystem) — historical system-wide behavior; check SDK deprecation and runtime support instead of assuming application-local or universally available behavior.
* [Apple: Advances in macOS Security](https://developer.apple.com/videos/play/wwdc2019/701/) and [CGPreflightListenEventAccess](https://developer.apple.com/documentation/coregraphics/cgpreflightlisteneventaccess()) — distinguish event listening from event modification permissions.
* [Microsoft: MSLLHOOKSTRUCT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-msllhookstruct) and [LowLevelMouseProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelmouseproc) — per-monitor-aware coordinates, message-pump requirements, and timeout removal.
* [Microsoft: Loopback Recording](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording) — endpoint loopback behavior and event-driven support; test silence/idle behavior on qualified devices.
* [FFmpeg: License and Legal Considerations](https://ffmpeg.org/legal.html) — configuration-dependent licensing and distribution guidance; select and document the actual shipped build.

## 9. Decisions Incorporated in This Revision

| Decision | Rationale |
| --- | --- |
| Retain short independent fMP4/WAV segments | File count is not simultaneous descriptor count; measure overhead and preserve bounded recovery units |
| Keep native-surface preview as the baseline | A platform integration risk is a feasibility gate, not proof of an architectural impossibility |
| Permit an evaluated AVAssetWriter segment adapter | Segment APIs exist; whichever adapter is selected must satisfy the same clock and durable storage contracts |
| Preserve baked-cursor fallback | Reliable cursor extraction and shape identification are platform/version capabilities, not universal guarantees |
| Separate Input Monitoring and Accessibility preflight | Listen-only and modifying event paths have different authorization requirements |
| Use explicit sample-frame/channel contracts for DSP | RMS squares do not cancel opposite polarity; channel misuse can corrupt timing or detection policy |
| Require reproducible acceptance evidence | Tests, native spikes, packaging checks, and performance measurements qualify different claims |

---

## 10. Agent Execution Guide

This section turns the architecture into small implementation assignments. **Existing** means observed in the working tree on 2026-09-08. **Proposed** means a file, command, or contract still needs implementation. A proposed name is not an API you can already call. Recheck this file and the working tree before starting; do not use old line numbers or comments as proof of behavior.

### 10.1 Select the next assignment

1. Read Sections 1.1, 3.3, 3.8, 3.9 and the relevant subsystem section in this file before modifying native recording.
2. Inspect `git status --short` and `git diff --stat`. Preserve existing working-tree changes. Do not restart Steps 1–2 merely because their hardware gates are still open.
3. Unless the user selects a different task, **the next bounded coding assignment is W1 if targeting Windows**. Remaining A1 compositor, H3 HUD software, H2 telemetry software, H1 failure-injection lifecycle, A1, S1, E1–E4, F1, F2, Z1 and Z2 are contract-tested. F2/E4 use system AVFoundation/VideoToolbox, explicit Rec.709 and the native playback worker; sustained device qualification remains open; do not implement a production renderer as a web-only substitute. Do not mark F1/H1/H2/H3 hardware-qualified from CI.
4. Production playback still needs live preview qualification. 16:9 webcam crop (`rect_16_9`) stays disabled. Do not mark A1 or F1 hardware-qualified.
5. Keep H1/H2 qualification open until measured. If a device or permission is unavailable, write the exact missing experiment and continue independent shared work. Do not mark a gate passed, replace it with synthetic media, or stop all unrelated implementation.
6. For a user-directed Windows task, start W1; the numbering of “Immediate Steps” is product priority, not permission to skip Phase 0 or Windows feasibility gates.

| Roadmap area | Work packages | Start condition |
| --- | --- | --- |
| Remaining recording/telemetry acceptance | H1, H2 | H1 failure-injection slice and H2 software slice are `contract-tested`. Live 4-track decode/soak and the H2 hardware matrix remain open. |
| Native camera HUD | H3 | H3 software slice is `contract-tested`. Live exclusion/transparent-corners/focus remain open; native embedding still depends on F1 hardware qualification. |
| Native media feasibility | F1, F2 | F1 and F2 are contract-tested on macOS. FFmpeg remains unpinned. Windows media is open. |
| Immediate Step 3 | E1 → E2 → E3 | E1–E4, F1, F2 and Z1 are contract-tested. |
| Basic export | E4 | E3 + F2. **E4 is contract-tested**. |
| Immediate Step 4 | Z1 → Z2 | **Z1 and Z2 are contract-tested**. |
| Advanced styling and silence | A1, S1 | **A1 and S1 are contract-tested**, including remaining A1 CPU/GPU clip/shadow/wallpaper blit. |
| Immediate Step 5 | W1 → W2 → W3 | Windows x64 SDK/hardware; do not infer parity from macOS. |
| Distribution | R1 | Working slices and measured capability results. |

**Status vocabulary:** `planned` → `implemented` → `contract-tested` → `hardware-qualified`. Record these separately per platform and feature. A passing compile establishes buildability; a passing synthetic test establishes only its asserted contract. “Done” for a work package means its listed output and checks are complete, not that the entire phase is qualified.

### 10.2 Actual repository map and observed traps

All paths below are repository-relative. Section 4 is a target layout and includes directories that do not yet exist.

| Responsibility | Existing entry points | What to check before changing it |
| --- | --- | --- |
| Tauri command registration | `src-tauri/src/lib.rs` | A Rust helper is not callable from React until registered in `generate_handler!`; match argument and result serialization. |
| Session orchestration | `src-tauri/src/commands/mod.rs`, `session/state.rs`, `session/clock.rs` | Keep Start/Pause/Resume/Stop ownership serialized; read retries and error paths, not only the successful path. |
| Native capture | `src-tauri/native/macos/AeroShootCapture.swift`, `src-tauri/src/capture/macos.rs`, `src-tauri/build.rs` | The active path is `aeroshoot_macos_start` → `ActiveRecorder`; `SCKitBridge.swift` is not a current file. Do not edit only the legacy prototype. |
| Segment publication/recovery | `project/segment_writer.rs`, `project/journal.rs`, `project/recovery.rs`, `project/media_validator.rs`, `project/lock.rs` under `src-tauri/src/` | Native publication uses `commit_native_segment`; general `commit_segment` is a separate existing method. Do not assume their durability properties are identical. Container validation is not full decoding. |
| Native telemetry | `src-tauri/native/macos/MouseHookMac.swift`, `src-tauri/src/telemetry/native.rs`, `src-tauri/src/telemetry/reader.rs` | New v2 events have a tagged `payload`; the old `telemetry/event.rs` types are v1. `reader.rs` accepts both. Native logger creation is for fresh recording bundles, not replay. |
| Native preview overlay | `src-tauri/native/macos/AeroShootPreview.swift`, `src-tauri/src/playback/preview.rs`, `src-tauri/src/playback/native.rs`, `front-end/src/components/canvas/NativePreviewHost.tsx` | Child overlay above WKWebView. Geometry is CSS points; pixels stay in-process. `previewAvailable` remains false. Browser HTML video is not this surface. Windows is unimplemented. |
| Decoder/compositor/encoder | `src-tauri/src/media/`, `src-tauri/src/render/`, `src-tauri/native/macos/AeroShootMedia.swift` | Owned BGRA frames; WGPU 27 offscreen scene; VideoToolbox H.264; encoder occupancy ≤ 1. FFmpeg is not in `Cargo.toml`. Do not serialize frames through IPC. |
| Export job | `src-tauri/src/export/`, `src-tauri/native/macos/AeroShootExport.swift`, `front-end/src/components/scenes/EditStudioScene.tsx` | Immutable revision; H.264+AAC only; dest cannot be inside the bundle or overwrite sources; temp + fsync + hard_link; status has no pixels. Browser emulation is not an export. Bound to 512px / 120 frames in this slice. |
| Rust timeline/DSP | `src-tauri/src/timeline/{interval,mapper}.rs`, `src-tauri/src/dsp/silence.rs`, `src-tauri/src/project/silence.rs` | S1 streams project PCM with frame-based RMS, max-energy (or selected channel), and gap resets. Zero window/step and non-finite thresholds are rejected. `TimelineMapper::new` sorts but does not reject overlaps. Exact edited end currently returns a source end sentinel. |
| UI/native boundary | `front-end/src/lib/ipc.ts`, `front-end/src/lib/types.ts` | Browser emulation is not native success. Project/open DTOs now match the Rust `OpenedProject` contract, including `zooms`. Zoom edit commands and `detect_silence` throw in the browser emulator. Do not parse v2 disk records as the old TypeScript `kind: "click"` interface. |
| Record-to-edit handoff | `front-end/src/hooks/useRecording.ts`, `front-end/src/components/scenes/RecordScene.tsx` | Stop returns `projectPath` and RecordScene switches to Edit only after `open_project` succeeds. A caught stop/open error must not display a sample session. |
| Editor state and timing | `front-end/src/stores/projectStore.ts`, `front-end/src/hooks/useTimeline.ts`, `front-end/src/App.tsx` | Ordinary sessions start with an empty editor. Audio lanes query `project_waveform` (E2). Playhead position is polled from Rust `playback_status` (E3); `requestAnimationFrame` is not the media clock. Native project frames are delivered by the Rust media worker to AppKit; availability is set after successful presentation. |
| Canvas, waveforms, editing | `front-end/src/components/scenes/EditStudioScene.tsx`, `components/canvas/NativePreviewHost.tsx`, `components/canvas/StudioCanvas.tsx`, `components/timeline/TimelineStudio.tsx`, `components/waveform/WaveformRenderer.tsx` | Record Scene owns live camera preview. Edit Studio mounts the F1 AppKit overlay host, not an HTML `<video>` element. F2 can composite/encode offscreen. Ripple cuts persist through `project.json`. Export starts a native H.264/AAC job (E4). Timeline can accept, move, resize and delete zooms; pending suggestions stay dashed until accepted. S1 jump-cut detection streams opened-project PCM (mic, else system) and applies one `project_ripple_cuts` revision. |
| App packaging | `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `src-tauri/Info.plist` | Current CSP is null. The hidden `camera_overlay` window is a separate root to verify. Signed permission behavior is not established by a development launch. |

**Frontend tracking warning:** `git ls-files --stage front-end` currently reports a mode `160000` submodule entry, but this checkout has no `front-end/.git`. Therefore the parent diff may omit frontend edits. Before a frontend delivery, inventory changed files explicitly and preserve a reviewable patch or equivalent artifact. Diagnose the intended submodule remote/history before repairing metadata; do not delete the directory, replace the gitlink, or reset the checkout just to make status look clean.

**Dependency warning:** FFmpeg bindings and native Windows adapters are architectural targets, not installed implementations. F2 pinned `wgpu 27.0.1` and used VideoToolbox for decode/encode. Choose/pin FFmpeg through a recorded packaging/license review. Do not guess APIs from this document or install several competing media stacks.

### 10.3 Invariants to apply in every work package

| Topic | Required rule | Concrete example or failure to reject |
| --- | --- | --- |
| Time units | Use integer microseconds for source/edited time; keep rational native PTS separately. Convert milliseconds only at the UI boundary. | `durationMs: 400` means `400_000` microseconds, not 400. Never use file modification time as media time. |
| Track identity | Use manifest track IDs and a deliberate serialization adapter. | Current bundle IDs are `screen`, `webcam`, `system`, `mic`; Rust audio track-type values are `system_audio`/`mic_audio`, while UI aliases differ. Do not infer type from display labels. |
| Time mapping | Map edited time to source time once, then select each track's segment at that source instant. | With retained `[0,2s)` and `[5s,10s)`, edited `2s` maps to source `5s`; edited `2.5s` maps to `5.5s`. |
| Endpoints | Use half-open media intervals; end-of-playback is a state, not another decodable frame. | The exact edited duration may position a UI playhead, but must not request a sample at the exclusive source end. |
| Native clocks | Preserve original timestamp/timebase and source-relative host anchor; distinguish original sample PTS from container-rebased PTS. | Do not subtract a raw capture PTS from a decoder PTS unless a measured mapping establishes that they share a timebase and origin. Unknown legacy anchors remain unknown. |
| Missing media | Represent gaps separately from valid silence and valid black frames. | A missing mic file is not evidence of silence suitable for an automatic cut. A late webcam must stay hidden until its first available sample. |
| Project mutation | Source media is immutable. Changes to edits, caches and recovery each need their own explicit ownership. | Opening a browser should not invoke a repairing journal writer or create a telemetry logger as a side effect. |
| Failure | Return a typed error and keep previously usable state. | A failed export cannot overwrite an earlier export; a failed Stop cannot display a sample session as the new recording. |
| Resource bounds | Declare numeric queue, cache, file and input limits in code/config and test them. Values need measured tuning. | Avoid `read_to_string` on unlimited JSONL or loading an hour of PCM into React. Return only viewport-sized waveform data. |
| Cursor | Respect `cursorMode`; missing shape/visibility is unknown. | With `baked`, the captured cursor is already in the screen pixels. Do not draw another cursor. |
| Compatibility | Validate versions at the boundary and migrate only with an explicit rule. | Never parse v2 telemetry as the old TypeScript `kind: "click"` interface or silently rewrite unknown project versions. |

### 10.4 Foundation and native qualification work packages

#### H1 — Close recording persistence and lifecycle gaps

**Status:** `contract-tested` on 2026-09-09 for the **failure-injection / lifecycle slice only** (pause finalize-before-ack, resume fresh intervals, injected sync/journal/finalization errors, retained ownership, no-overwrite publication). **Not hardware-qualified.** Do not claim live 4-track independent decode or a 60-minute soak.

**Read:** Sections 3.1, 3.3, 3.8–3.9; native segment evidence; active native/Rust Stop and Pause paths.

**Implement/verify in order:** identify which thread owns each writer; verify pause finalizes native segments before acknowledgment; verify resume starts fresh segments; trace commit failure into session diagnostics and Stop; retain recoverable ownership after preparation/finalization failure; verify retries do not turn an earlier failure into false success. Audit native and non-native publication methods for no-overwrite, sync ordering and error propagation. Do not restore Stop's removed salvage scan.

**Policy (this revision):** Pause does **not** report `Paused` if native or Rust finalization or `PauseStarted` journaling failed. The command returns `Err`, records a typed `RuntimeError` on session diagnostics (`-610` writer, `-611` journal, `-612` native pause), and keeps the session. Resume after a successful pause opens a new segment sequence; it does not append to a closed file. `commit_segment` and `commit_native_segment` both publish with no-overwrite hard-link + directory sync + journal; a journal failure after publish leaves a pending publication so retry cannot claim success without journaling. Stop does not scan leftover temps.

**Writer ownership:** Swift `RotatingMediaWriter` owns `AVAssetWriter` on capture/rotation queues. Pause calls `pauseAndFinalize` (drain queues, `rotateSegment` / Rust `commit_native_segment`) before the Tauri command acknowledges Pause. Rust `TrackSegmentWriter` is owned by the session command thread for the synthetic path, and is constructed throwaway on the Swift callback thread for native publication.

**Output:** targeted lifecycle/failure tests plus the hardware procedure below. **CI pass:** finalize error surfaces; pause does not report paused if finalize failed; resume after successful pause creates a new journal interval; injected sync/journal/finalization failure returns an error with sources/prior commits preserved; pause boundaries and late tracks preserve source time in fixtures. **Not claimed:** finalized segments decode independently on a live 4-track hardware recording; 60-minute four-track skew/memory/drop soak.

**Hardware procedure (not run — do not mark `hardware-qualified`):**

1. Signed/dev build with Screen Recording (and Camera/Mic if used) authorized. Destination on the same APFS volume as the project folder.
2. Start a 4-track recording (display + webcam + mic + system audio) for at least two 2-second segment rotations.
3. Pause; wait until the UI shows Paused (must not show Paused if a writer error is surfaced). Confirm each track's last pre-pause file is a closed container (`*.mp4` / `*.wav`, not `*.tmp`) and `journal.jsonl` has `pause_started` after the last `segment_committed` for that interval.
4. Resume; confirm the next files are new sequence IDs and are not appended to the closed pre-pause files.
5. Stop cleanly. Independently decode each committed segment (e.g. AVAssetReader / `afinfo`) and confirm each video segment starts on a keyframe.
6. Repeat: record until at least one segment is journaled, then force-quit (`kill -9`) the process. Reopen via recovery. Previously journaled files must still decode; do not expect the in-flight temp to survive.
7. Optional disk-full / unmount injection on the project volume: Start/Pause/Stop must return an error, not `Completed`/`Paused`, and must not overwrite earlier committed files.

**Missing experiment (exact):** a live ScreenCaptureKit 4-track recording was **not** run in this evidence set. Independent decode of hardware-finalized segments, pause-during-live-capture, force-kill after a real native commit, and the 60-minute skew/memory/drop soak remain open. CI used synthetic fMP4/WAV fixtures and injected `DurabilityFault` / journal failures only.

#### H2 — Qualify telemetry before trusting automatic zooms

**Status:** `contract-tested` on 2026-09-09 for the software slice (permission/gap/overflow/shutdown contracts, honest permission JSON, 100 ms geometry uncertainty, no fabricated zooms from denied telemetry). **Not hardware-qualified.**

**Read:** Section 3.2, native hook and v2 schema.

**Implement/verify in order:** exercise explicit permission grant/denial/revocation; confirm ordinary recording remains available; align visible physical clicks with media; move a window across negative-origin/mixed-DPI displays; establish source content/crop transforms; test event-tap timeout, user disable, overflow and shutdown. Preserve the current 100 ms geometry uncertainty intervals until a more precise adapter has measured evidence.

**Output:** a supported-source capability matrix and reproducible evidence. **Pass:** no double cursor, no button provenance invented across a gap, bounded memory and no callbacks after teardown. Application capture and missing window physical transforms remain unsupported/uncertain until implemented and measured. Do not add keystroke collection to infer “typing interest.”

**Software slice (this revision):** CI covers typed deny/revoke/overflow/user-disable/timeout gaps, stop-after-teardown, unclamped off-source coordinates, application/window-without-physical as unsupported, session-clock `t_us` conversion, `sampling_interval_us == 100000`, and `project_zoom_suggestions` on denied JSONL. Recording start does not require Input Monitoring; cursor mode stays `baked`.

**Hardware capability matrix (not run — do not claim pass):**

| Experiment | Source | Expected software behavior | Hardware evidence required |
| --- | --- | --- | --- |
| Grant Input Monitoring | Display | v2 move/button stream; no `input_monitoring_unavailable` after grant | Signed/notarized app, TCC grant, then one recording |
| Deny Input Monitoring | Display | Recording still starts; telemetry is gaps only; no fabricated clicks; baked cursor | Deny at preflight; Start Recording; inspect `telemetry/events.jsonl` |
| Revoke mid-session | Display | `input_monitoring_revoked` gap; held buttons unknown; recording continues | Grant, start, revoke in System Settings during record |
| Click-to-frame alignment | Display | Session-clock `t_us` from CGEvent host time | Overlay physical click vs decoded frame; **not claimed from CI** |
| Negative-origin window | Window on left-of-primary display | Window geometry stays unsupported without physical transforms; off-source not clamped to edges | Move a captured window onto a display with negative Quartz origin |
| Mixed-DPI window | Window spanning or moving across 1x/2x displays | Keep unsupported; do not invent a physical transform | Two displays at different backing scales |
| Application capture | `application:` source | `unsupported_source_geometry`; no auto-zoom from that source | Select an app capture source if/when the picker exposes one |
| Event-tap timeout / user disable | Display | `event_tap_disabled`; timeout may re-enable only if still authorized; user disable does not re-prompt | Induce tap disable on a live tap |
| Overflow / shutdown | Display | Bounded queue; overflow gap; no tap callbacks after `stop` | Sustained motion under load; stop while moving the pointer |

**Do not claim:** live mouse-tap alignment, mixed-DPI physical transforms, or that auto-zoom matches the click you saw. Do not tighten `geometry_uncertainty_us` / `sampling_interval_us` below 100 ms without measured adapter evidence.

#### H3 — HUD-only window and shared settings

**Status:** `contract-tested` on 2026-09-09 for window-label routing, revisioned settings, snapshot recovery, camera-removal visibility, HUD-close-without-stop, and attach-to-existing-mailbox (no second capture session). **Not hardware-qualified.** F1 remains contract-tested, not hardware-qualified. Live `SCContentFilter` exclusion, transparent-corner hit testing, and overlay focus stay open.

**Read:** Section 3.4, `front-end/src/main.tsx`, `src-tauri/tauri.conf.json`, `settingsStore.ts` and native camera ownership. `main.tsx` now selects `studio` / `hud` / `rejected` from a verified `window_identity` label before mounting. The configured `camera_overlay` window is still not proof of exclusion or live embedding.

**Sequence:** select the UI root from verified window identity before mounting → create a HUD-only component → expose Rust-owned revisioned settings/snapshots → test reconnect/stale updates → attach the F1-qualified native preview surface to the existing camera capture session → verify capture exclusion and hit testing. Do not create another camera capture session or a second recording state machine for the overlay.

**Acceptance:** opening/reloading `camera_overlay` mounts only the HUD, changing settings in either window converges to one revision, dropped events trigger snapshot recovery, camera device removal is visible, and closing the HUD does not stop source recording. Qualify transparent corners, focus and self-exclusion on real platforms; use the Section 3.4 fallback if exclusion/embedding cannot be established.

**Software slice (this revision):** `resolve_ui_root` / `window_identity` reject unknown labels; `HudOnlyRoot` does not mount Record/Edit or call `start_recording`; Rust `HudOwner` is the settings authority (Zustand is a view); `hud_close` detaches the overlay `PreviewOwner` only; live frames read `LivePreviewFrames` / `camera_frame()` from the existing session mailbox. Exclusion is reported `exclusionEstablished: false`; HUD is hidden while a session is active (Section 3.4 fallback).

**Hardware experiments not run (do not mark passed):**

| Experiment | Required setup | Why it is still open |
| --- | --- | --- |
| Self-exclusion | Real ScreenCaptureKit display capture with the HUD visible | Confirm `SCContentFilter` excluding the AeroShoot process/HUD window; inspect recorded pixels for HUD chrome |
| Transparent corners | Live `camera_overlay` over another app | Clicks on circle/squircle corners must pass through; drag handles and controls remain usable |
| Overlay focus | Keyboard and window z-order on a real desktop | HUD must not steal keystrokes from the recorded app except on chrome |
| Live mailbox present | Recording + HUD attach on device | Camera latest-frame queue presents on the HUD AppKit overlay without a second `AVCaptureSession` |

#### F1 — Minimal native preview feasibility spike

**Status:** `contract-tested` on 2026-09-08 for the macOS AppKit child-overlay spike (fixed frame, one in-process H.264 fixture, geometry/hit/lifetime contracts). Not hardware-qualified. Windows is open.

**Prerequisite:** a small, actually decodable recording fixture; Sections 3.5 and Phase 0. Implemented modules: `src-tauri/src/playback/preview.rs`, `src-tauri/src/playback/native.rs`, `src-tauri/native/macos/AeroShootPreview.swift`, `front-end/src/components/canvas/NativePreviewHost.tsx`.

**Sequence:** first embed an owned native view and draw a fixed frame; next attach one decoded screen stream; then prove resize/backing-scale/occlusion and lifetime handling; then test the hidden HUD window and transparent hit regions. Measure copies, memory and view placement. Keep the spike small enough to discard without rewriting the editor.

**Output:** an architecture decision recording the chosen native surface arrangement, ownership/thread rules, tested OS/device, measurements and failure modes. **Pass:** the view tracks the React viewport without intercepting unrelated controls; close/reopen leaves no dangling view or callback. A video visible in a browser alone does not close this native gate. Windows needs its own experiment; macOS success does not qualify Windows. Live Tauri/React tracking was not run in this evidence set; do not promote F1 to `hardware-qualified` from the synthetic contracts.

#### F2 — Decoder, compositor and encoder interoperability

**Status:** `contract-tested` on 2026-09-08 for a macOS VideoToolbox decoder/encoder plus a WGPU 27 offscreen compositor. Not hardware-qualified. FFmpeg is unpinned.

**Prerequisite:** F1 and the media dependency policy in Section 3.7. Implemented modules: `src-tauri/src/media/`, `src-tauri/src/render/`, `src-tauri/native/macos/AeroShootMedia.swift`.

**Sequence:** select one decoder interface with owned timestamped frames; pin the actual dependency build; decode real segmented H.264 and PCM; feed one WGPU scene to preview and an encoder; verify color interpretation, output dimensions and timestamps; measure any required copies and concurrent encoder limits. Record software fallback settings only after testing them.

**Output:** a minimal styled preview/export pair plus dependency/version/build-flag records. **Pass:** independently decoded exported frames match the preview reference within a stated tolerance; resource counts stay bounded. Keep packaging/license obligations open until reviewed against the chosen binary configuration. This spike did not pin FFmpeg, convert to Rec.709, or qualify Windows.

### 10.5 E1 — Open real project metadata and build the segment index (implemented, contract-tested)

**Status:** `contract-tested` on 2026-09-08. Hardware-qualified native playback is out of scope.

**Scope:** shared project loading and honest editor state. This can proceed while native rendering gates are open. Do not implement a second renderer in this package.

1. Inspect `StopRecordingResult`: it currently returns only `sessionId`, `state`, and `durationUs`, with no project path or handle. Extend the successful response with a resolvable project identity/handle (or add a session-to-project lookup), preserving repeated-Stop results and updating IPC tests. Do not reconstruct the recording directory in React from a guessed path. Define a registered project-open command and owned project handle. **Proposed names:** `open_project`, `close_project`, `OpenedProject`; reuse equivalent existing contracts if they have appeared since this revision.
2. Under `src-tauri/src/project/`, add a bounded reader for manifest and committed journal metadata. The current `ProjectBundle::open_existing` acquires a writer lock and opens a potentially repairing journal; do not advertise it as a read-only inspection API without separating those behaviors. Reject unsafe paths, unsupported versions and active-writer conflicts according to an explicit ownership policy.
3. Build a per-track, source-time segment index from committed/recovered records. Preserve relative path, source interval, format, container timestamps and known clock anchors. Sort and validate; expose missing/conflicting entries as diagnostics. Do not treat directory listing order or a track's `relative_path` directory as the playable index.
4. Define the actual serialized DTO in Rust first, then update `lib/types.ts` and `lib/ipc.ts`. Include project identity/revision, manifest, track/segment summaries, source duration, retained intervals and diagnostics. Large indexes should be paged or queried, not copied through every UI update. Proposed command arguments should use a project handle after the initial user-selected path.
5. Change `useRecording.ts` to open the successful Stop result's real project. Return an explicit success/failure outcome to `RecordScene`; switch scenes only on success. Load actual metadata into `projectStore.ts`. Remove automatic sample population in `App.tsx` for ordinary desktop sessions; an explicit demo mode may retain fixtures.
6. Give an empty/new project an empty editor. Give missing media a visible unavailable state. Until E3 is ready, show “preview unavailable” rather than the current live camera or invented recording pixels. Keep Export/AI controls disabled when their backend operation is unavailable.

**Acceptance:** open two different bundles with different track counts/durations and observe different editor state; close/reopen restores the same metadata; failed Stop stays out of a falsely successful Edit state; malformed/escaping paths fail without mutation; absent optional tracks create no fabricated rows. Test the real serialized command boundary, not just a helper returning a hand-built object.

### 10.6 E2 — Real waveform cache and channel-aware audio reads (implemented, contract-tested)

**Status:** `contract-tested` on 2026-09-08. Native compositor F2 is contract-tested.

**Prerequisite:** E1's segment index (implemented). **Read:** Section 3.6, `dsp/silence.rs`, `WaveformRenderer.tsx`, `TimelineStudio.tsx`. Implemented in `src-tauri/src/project/pcm.rs` and `src-tauri/src/project/waveform.rs`.

1. Read validated WAV headers and supported PCM formats from real files; report unsupported encodings. Stream sample frames in bounded chunks. Preserve each segment's source offset, sample rate and channel count. Do not assume every WAV is 16-bit mono or every segment lasts exactly two seconds.
2. Compute per-channel peak and RMS buckets with sample-frame boundaries. For a bucket of N samples in one channel, `peak = max(abs(x))` and `RMS = sqrt(sum(x*x)/N)`. A stereo frame contains two samples but represents one time step. Do not average opposite-polarity channels before measuring energy.
3. Store rebuildable cache levels under `cache/`, keyed by source identity, channel policy, bucket size and analysis version. A corrupt/stale cache should trigger recomputation, not source edits. Reuse the same PCM reader later for silence detection.
4. Register a cancellable waveform query with project/track/range/resolution arguments. Fetch the visible time range at an appropriate cache level. Make gaps visibly distinct from low amplitude; map buckets through the shared retained intervals after cuts.
5. Replace `generateSyntheticWaveform` in normal project loading. Adjust the waveform view so zero-amplitude/gap display is deliberate; its current minimum bar height is decorative, not proof of audio energy.

**Acceptance:** zero PCM gives zero RMS/peak; a constant 0.5 channel gives RMS/peak 0.5; stereo `[a,-a]` remains energetic under the documented channel policy; a late segment starts at its actual source offset; a missing file is a gap; cache invalidation, cancellation and long input stay bounded. A 48 kHz frame index of 48,000 is one second regardless of channel count.

### 10.7 E3 — Synchronized playback, seek and basic timeline edits (implemented, contract-tested)

**Status:** `contract-tested` on 2026-09-08 for the playback owner, seek plans, clock policy, bounded working set, and edit revisions. F1 overlay, F2 interop, and E4 mux are contract-tested; native decode, stereo audio-device playback and AppKit presentation are implemented; sustained device qualification remains open.

**Prerequisites:** E1/E2 (implemented). F2 compositor and native playback worker are integrated; sustained device performance still needs qualification. Start with one screen track; add webcam and both audio tracks only after basic seeking works.

1. Add a native playback owner with explicit `closed`, `ready`, `playing`, `paused`, `seeking`, `ended`, `error` states. Give each open/seek a generation identifier; discard stale decoder results after a new seek or project switch.
2. Use the shared Rust evaluator to map edited time to source time and select segments. Seek to a preceding keyframe, then decode to the requested presentation time. Keep a bounded decoder/file working set. Do not instantiate one decoder per segment in the entire project.
3. Use the native audio playback clock when audio is present and a defined monotonic fallback when all audio is absent. Publish throttled positions/status to React. Replace the authoritative `requestAnimationFrame` clock in `useTimeline.ts`; UI interpolation may smooth display but cannot determine media offsets.
4. Draw screen and webcam into the selected native compositor; apply the missing-track policies in Section 3.8. Separate live recording preview from opened-project playback in `StudioCanvas`/`EditStudioScene`.
5. Define and persist a versioned `project.json` edit revision with validated retained source intervals and basic layout. Implement trims/ripple cuts/undo/redo in Rust, then return a new revision. Reject stale revision writes; one interval list applies to all synchronized tracks.

**Acceptance:** the retained-interval example in 10.3 holds exactly at cuts; all tracks seek together; repeated seek/project switching cannot show an older frame; project reopen preserves edits; undo restores prior content with a new revision ID; missing mic does not stall the clock; no decoder request uses the exclusive end sentinel. Measure seek latency and maximum open files over thousands of segments.

### 10.8 E4 — Basic export and preview parity (implemented, contract-tested)

**Status:** `contract-tested` on 2026-09-08 for an immutable-revision MP4 H.264/AAC job that samples the same timeline/scene evaluator as preview. Synthetic 1080p output and explicit Rec.709 tagging are tested; sustained throughput, calibrated color and Windows remain unqualified.

**Prerequisites:** E3 and F2. **Initial format:** MP4 H.264 video/AAC audio under the existing plan, not ProRes despite the former placeholder alert.

1. Capture an immutable edit revision and export settings into a native job. Reject source paths as output destinations; use a temporary output and a deliberate collision policy.
2. At each rational output frame time, call the same timeline/scene evaluator as preview. Render the resulting native frame and mix/resample source audio through the same retained intervals. Keep PTS/DTS and encoder delay handling explicit.
3. Expose job ID, progress, cancellation and typed failure. Throttle UI events. On success, finalize/validate/sync and publish the output; on cancel/error, preserve source media and any previous output.
4. Wire the existing Export button to this job and show the actual result. Disable unsupported codec/settings choices. Add basic background/webcam layout parity before advanced effects.

**Acceptance:** decode the produced file, compare representative frames at cuts/zooms/gaps against the preview reference, and verify audio duration/alignment. Cancellation, destination collision, disk failure and project edits during export cannot alter the captured revision or source files. Document color/codec comparison tolerances and measured throughput.

### 10.9 Z1/Z2 — Telemetry-driven zoom suggestions and editing

**Z1 status:** `contract-tested` on 2026-09-09 for a bounded v1/v2 telemetry reader, deterministic interest clustering, source-anchored cubic Bézier evaluation, and `project_zoom_suggestions`. H2 live telemetry qualification remains open; do not treat synthetic JSONL as physical click alignment.

**Z2 status:** `contract-tested` on 2026-09-09 for persist/undo of generated vs manual keyframes, regeneration that cannot overwrite existing or dismissed ids, Edit Studio accept/move/resize/delete, and shared `SceneEvaluator` UV crops. Live preview/export qualification of those crops remains open with F1/H2.

**Z1 prerequisites:** E1 metadata contracts (implemented); H2 limitations must be respected. **Z2 prerequisites:** E3/E4 shared scene evaluation plus this Z1 evaluator.

1. Implement a bounded, version-aware reader. V1 uses top-level `kind`; v2 uses `payload.kind`. Native v2 disk records use snake_case and omit the FFI-only `record` wrapper. Gap events may have no coordinates or geometry. Recover only a partial final JSONL line; report corrupt interior lines and unknown versions.
2. Build interest events from known button-down transitions and dwell intervals. Reset held-button knowledge across gaps; do not double-count a v1 click and a guessed transition. Ignore off-source coordinates, unsupported geometry and uncertainty intervals when generating targets. Do not clamp an off-source event into an artificial edge click.
3. Make clustering/dwell/merge thresholds explicit configuration. Use deterministic time/space grouping, tie-breaking and stable IDs. Define a maximum zoom, minimum hold time, transition duration and viewport bounds policy. Record generation version/config with suggestions so regeneration is reproducible.
4. Produce source-anchored zoom suggestions with references to contributing events. Generate smooth cubic Bézier transitions through a pure evaluator. Cuts retain the explicit discontinuity policy from Section 3.8; a suggested trajectory must not interpolate through removed time.
5. Z2: let users accept, move, resize and delete suggestions with undo/redo. Persist edits; distinguish manual keyframes from generated suggestions so regeneration cannot silently overwrite manual work. Preview and export consume the same evaluated transform.

**Z1 acceptance (met in contract tests):** identical input/config yields identical output; empty/denied/gapped telemetry does not fabricate zooms; auxiliary buttons are ignored and rapid primary clicks merge; a cut through a zoom evaluates independently in each retained interval; camera viewport is clamped, not the original recorded coordinates. No ML service or remote upload is used.

**Z2 acceptance (met in contract tests):** accept/dismiss/update/delete persist through `project.json` with undo; regeneration skips existing and dismissed ids and cannot reset a moved generated zoom; identity UV is the full frame and a 2× hold crops the screen layer only; a cut through a persisted zoom keeps the same source-evaluator discontinuity in preview mapping. Hardware-qualified preview/export of zoomed pixels remains open.

#### A1 — Advanced canvas and webcam styling

**Status:** `contract-tested` on 2026-09-09 for revisioned `EditLayout` persistence (save/reopen/undo), the `project_layout_update` command, aspect-correct screen placement (no stretch), webcam-only mirroring, solid/gradient/wallpaper backgrounds, screen rounded-rect clip + drop shadow, and webcam circle/squircle clip + optional shadow through `Scene::from_layout` + `Compositor::composite_cpu` / GPU `composite.wgsl` / `SceneEvaluator::preview_at`. Not hardware-qualified. Live AppKit preview/export of styled pixels remains open with F1/H1.

**Prerequisites:** E3/E4. Promote the existing inspector controls into validated, revisioned scene parameters: background, padding, aspect ratio, corner radius, shadow and webcam placement/shape/mirror. Apply every parameter through the shared native scene evaluator; CSS changes alone affect only the controls/preview shell. Import wallpaper assets into a validated project asset store with size/path limits instead of depending on external URLs during export.

**Supported in this slice (CPU blit and GPU textured quads):** solid and linear-gradient backgrounds; wallpaper decode/blit of the ingested `assets/` file (cover UV, never an export-time URL); padding; aspect-correct letterboxed screen; screen rounded-rect clip via `cornerRadiusPx`; screen drop shadow (`shadowBlurPx` / `shadowOpacity`) drawn as a silhouette behind the layer (does not punch the background); rectangular webcam bubble with corner/custom placement, size, border, and UV mirror on that layer only; circle and squircle clips on the **webcam layer only**. Zoom UV crops still apply after layout, on the screen layer only.

**Disabled:** 16:9 webcam bubble crop (`rect_16_9`); HDR; ProRes; cursor replacement. Do not mark A1 hardware-qualified.

**Acceptance:** save/reopen and undo preserve styling; wide/portrait/square outputs fit the intended source without accidental stretching; webcam mirroring affects the selected compositing layer only; representative preview/export frames match for all supported shapes and backgrounds within existing compositor tolerances. Keep unsupported effects disabled until they render in both paths. Do not add HDR, ProRes or new cursor replacement capabilities as styling shortcuts.

### 10.10 S1 — Real silence suggestions and reversible cuts

**Status:** contract-tested (not hardware-qualified).

**Prerequisites:** E2 PCM reader and E3 revision/interval edits. The previous `detect_silence` command fabricated a 1s/2s/1s buffer; that path now requires an opened project and streams E2 PCM.

1. `detect_silence(project_handle, track_id, config)` rejects stale handles, zero window/step, and non-finite thresholds. Optional `windowMs`/`stepMs` default to 20ms/10ms; optional `channelPolicy` matches waveform (`max_energy` or `channel:N`).
2. The detector operates on sample frames: per-channel RMS, then max-energy (or a selected channel). State is preserved across contiguous segment/chunk boundaries and reset on missing files, unsupported encodings, gaps, or discontinuities. Padding shrinks proposed removals; suggestion count is bounded to one ripple revision (`MAX_CUTS_PER_REVISION`).
3. Suggestions carry source-time diagnostics and edited `startUs`/`endUs` for the existing modal. Applying selected cuts is one `project_ripple_cuts` revision. Undo restores retained intervals; reopen preserves the applied result. Filler-word recognition and speech transcription remain deferred.

**Acceptance (met in contract tests):** a silent run crossing two contiguous WAV files is detected once; a missing file is not classified as silence; opposite-polarity stereo retains energy; selected channel timing uses sample frames; applying selected cuts is one shared revision; undo restores previous retained intervals; reopen preserves the result. Hardware-qualified jump cuts on live recordings remain open.

### 10.11 W1–W3 — Windows adapter sequence

These are proposed files/modules: `src-tauri/native/windows/WGCBridge.cpp`, companion WASAPI/Media Foundation adapters and `src-tauri/src/capture/windows.rs`. Keep dependencies/build logic target-gated in `build.rs` and Cargo; macOS must still build without Windows SDK libraries.

| Package | Ordered work | Required output and evidence |
| --- | --- | --- |
| W1: screen vertical slice | Establish C ABI ownership and QPC/session-clock mapping → enumerate/select a real display or window → WGC frame pool/D3D11 resources → hardware MFT probe → encode one video track → shared segment publication. | Real decodable recording plus consent, selected-window close/minimize/resize, GPU-loss and Stop-failure tests on Windows x64. Never broaden window capture into full-display capture silently. |
| W2: synchronized optional tracks | Add WASAPI mic → loopback including idle endpoint → Media Foundation webcam → explicit gaps/device loss → pause/drain/stop across all tracks. | Four-track timing test, no-audio startup, silence-to-audio return, endpoint removal and long-session skew/memory/drop report. Source clock must advance when no audio packets arrive. |
| W3: telemetry, preview and packaging parity | Add bounded mouse-hook thread/geometry conversions → native preview surface and compositor interop → export parity → packaging smoke tests. | Mixed-DPI/negative-origin tests, stationary-cursor capability decision, callback shutdown tests and the Windows equivalents of F1/F2/E4. Missing ARM64 hardware leaves ARM64 unqualified. |

Reuse shared commands, schemas, timeline and commit semantics. Do not create a separate Windows project format or editor. Confirm that the shared publication primitive and directory durability implementation actually work on the target filesystem; compiling the Rust interface is insufficient.

### 10.12 R1 — Performance, security and distribution

**Prerequisites:** functional capture, preview and export slices. Separate application changes from qualification artifacts.

1. Pin toolchains/dependencies and record reproducible native build commands. Add CI for shared contracts, frontend build and desktop feature builds on supported runners. Run real-device tests outside hosted CI where capture/permission/GPU behavior requires it.
2. Define a capability report and test 1080p30 first, then candidate 1080p60/4K modes. Record actual encoder selection, concurrent-track load, memory/open-file peaks, dropped frames, disk bandwidth and thermal behavior. Disable modes with no qualifying result rather than exposing a blanket “4K60 supported” setting.
3. Set a deliberate CSP and project-scoped media access policy, audit both window roots/capabilities, and test untrusted bundles. Review dependency build flags/notices and the chosen distribution obligations before packaging.
4. Build signed/notarized macOS and signed Windows artifacts through the approved release workflow. Test installation, launch, permissions, capture, reopen, export and uninstall on clean target machines. Do not log or upload private media/telemetry by default.

**Acceptance:** each shipping OS/architecture/mode has a linked result and reproducible artifact; unsupported modes have an honest UI state. Signing, distribution approval and external publication follow the user's release authorization; local implementation and build work do not require an invented approval checkpoint.

### 10.13 Validation commands, evidence and handoff

Run from the repository root unless stated otherwise. Run checks relevant to changed code; a documentation-only update needs link/symbol/consistency checks, not a native recording.

```sh
# Shared tests; on macOS the normal build also links the real Swift bridge.
cargo test --manifest-path src-tauri/Cargo.toml

# Desktop-feature type/build check; does not launch the app or prove capture.
cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app

# Synthetic native mouse contracts on macOS; installs no global event tap.
sh script/test-mouse-telemetry.sh

# Frontend type check + production build (execute in front-end/).
npm run build

# Whitespace / patch integrity.
git diff --check
```

`AEROSHOOT_SKIP_SWIFT` bypasses the real bridge. Do not use a skipped/stubbed build as native evidence. Finish editing Swift inputs before starting their compile; the compiler rejects files modified during a build. `script/codex.sh start` launches Vite only, not the native desktop app. There is no general frontend test runner configured yet; add a focused harness when behavioral UI tests require one, and report what actually ran.

For every completed package, append an evidence record to its relevant implementation note or a new named report:

```text
Work package ID / status:
Commit + working-tree changes (including frontend tracking limitations):
OS / architecture / device / SDK / compiler / dependency versions:
Fixture or real capture provenance / recording configuration:
Reproduction commands and manual steps:
Expected outcome:
Observed outcome and measured values:
Artifact paths (local/private where appropriate):
Checks not run and exact reason:
Open gates / next smallest package:
```

Before handing off, confirm that the user-visible behavior reaches the new backend, errors remain visible, serialization agrees on both sides, and affected checks pass. Update the task status in this file; do not copy a stale test count into a new claim. Keep unresolved defects and platform limits explicit. If a choice is already fixed by this plan, follow it; if measurements require changing an architectural choice, record the alternatives, evidence and recovery/compatibility consequences before adopting it.


## E1–E4 / F1–F2 review resolution — 2026-09-09

The 14 correctness findings from the E1–E4 / F1–F2 review have code fixes and focused regression coverage. E1 preserves an open project on failed replacement. E2 safely publishes content-validated waveform caches and bounds viewport work. E3 now includes actual native decode/composition/audio playback, manual trim/delete controls and durable monotonic edit history. E4 streams resampled stereo audio, preserves the final fractional frame, validates native output and offers 720p/1080p/4K. F1 tracks scrolling and clips geometry/hits with generation-qualified lifetime. F2 preserves capture dimensions, handles local segment PTS and declares explicit SDR Rec.709.

The frontend's broken gitlink was replaced with ordinary source entries in the index so these UI changes are recoverable. Changes remain uncommitted. This file distinguishes implemented behavior and automated tests from unqualified live desktop/device, sustained-load and Windows acceptance. Do not promote all packages to hardware-qualified based on the regression suite.

## Z1 evidence — 2026-09-09

Work package ID / status: Z1 / contract-tested (not hardware-qualified; H2 remains open)
Commit + working-tree changes (including frontend tracking limitations): uncommitted Z1 reader/generator/evaluator, `project_zoom_suggestions`, Edit Studio timeline display; prior E1–E4/F1/F2 work also remains uncommitted
OS / architecture / device / SDK / compiler / dependency versions: macOS 26.6.2 (25G83), aarch64-apple-darwin, rustc 1.98.1 (48a229cea 2026-09-01)
Fixture or real capture provenance / recording configuration: synthetic v1/v2 JSONL fixtures; no live Input Monitoring session
Reproduction commands and manual steps: `cargo test --manifest-path src-tauri/Cargo.toml --lib zoom`; `cargo test --manifest-path src-tauri/Cargo.toml --test zoom_tests`; `npm run build` in `front-end/`
Expected outcome: identical input/config yields identical suggestions; empty/denied/gapped/off-source/auxiliary telemetry does not fabricate zooms; cuts evaluate per retained source interval; IPC uses camelCase
Observed outcome and measured values: zoom lib tests, telemetry reader tests, and `zoom_tests` passed; frontend `tsc && vite build` succeeded
Artifact paths (local/private where appropriate): none (synthetic only)
Checks not run and exact reason: live mouse-tap alignment, mixed-DPI window geometry, and native preview/export sampling of the zoom transform (H2)
Open gates / next smallest package: Z2 editable/persisted keyframes consuming this evaluator in preview and export

## Z2 evidence — 2026-09-09

Work package ID / status: Z2 / contract-tested (not hardware-qualified; F1 live preview and H2 remain open)
Commit + working-tree changes (including frontend tracking limitations): uncommitted Z2 persistence/commands/UI and shared evaluator UV crop; Z1 and prior E1–E4/F1/F2 work also remain uncommitted
OS / architecture / device / SDK / compiler / dependency versions: macOS 26.6.2 (25G83), aarch64-apple-darwin, rustc 1.98.1 (48a229cea 2026-09-01)
Fixture or real capture provenance / recording configuration: synthetic v2 JSONL plus CPU compositor fixtures; no live Input Monitoring or AppKit preview session
Reproduction commands and manual steps: `cargo test --manifest-path src-tauri/Cargo.toml --lib zoom`; `cargo test --manifest-path src-tauri/Cargo.toml --lib render`; `cargo test --manifest-path src-tauri/Cargo.toml --lib project::revision`; `cargo test --manifest-path src-tauri/Cargo.toml --test zoom_tests`; `cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app`; `npm run build` in `front-end/`
Expected outcome: accept/dismiss/update/delete persist with undo; pending suggestions omit persisted/dismissed ids; preview/export share `SceneEvaluator` UV crops; a cut through a persisted zoom matches source-time evaluation
Observed outcome and measured values: zoom lib tests (9), render UV crop test, revision zoom persistence/undo test, and `zoom_tests` (3) passed; `tauri-app` check succeeded; frontend `tsc && vite build` succeeded
Artifact paths (local/private where appropriate): none (synthetic only)
Checks not run and exact reason: live AppKit preview of zoomed pixels, H.264 export pixel compare of a non-uniform zoomed frame, mixed-DPI window geometry (F1 / H2)
Open gates / next smallest package: A1 canvas/webcam styling through the shared evaluator, or S1 real silence detection

## A1 evidence — 2026-09-09

Work package ID / status: A1 / contract-tested (not hardware-qualified; F1 live preview and H1/H2 remain open)
Commit + working-tree changes (including frontend tracking limitations): uncommitted remaining A1 CPU/GPU rounded-rect/circle/squircle clip, drop shadows, wallpaper decode/blit, inspector enablement; prior A1 layout persistence and E1–E4/F1/F2/Z1/Z2/S1 work also remains uncommitted
OS / architecture / device / SDK / compiler / dependency versions: macOS 26.6.2 (25G83), aarch64-apple-darwin, rustc 1.98.1 (48a229cea 2026-09-01)
Fixture or real capture provenance / recording configuration: synthetic CPU/GPU compositor frames and empty-track project bundles; wallpaper decode uses a tiny local PNG ingested into `assets/`. No live AppKit preview or H.264 export pixel compare of a styled 9:16 frame
Reproduction commands and manual steps: `cargo test --manifest-path src-tauri/Cargo.toml --lib project::layout`; `cargo test --manifest-path src-tauri/Cargo.toml --lib render`; `cargo test --manifest-path src-tauri/Cargo.toml --test layout_tests`; `cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app`; `npm run build` in `front-end/`; `git diff --check`
Expected outcome: layout save/reopen/undo including radius/shadow/circle; 16:9 vs 9:16 screen placement keeps source aspect; webcam mirror does not flip screen; circle/squircle clip webcam only; screen rounded-rect uses cornerRadius; shadow darkens padding without punching a hole; wallpaper blit uses the ingested asset and rejects URLs; CPU vs GPU within COMPOSITOR_* tolerances; inspector enables wallpaper/radius/shadow/circle/squircle; `rect_16_9` stays disabled
Observed outcome and measured values: `project::layout` (3), `render` (9, including GPU vs CPU clip/shadow/wallpaper), and `layout_tests` (5) passed; `cargo check --features tauri-app` succeeded; frontend `tsc && vite build` succeeded; `git diff --check` clean
Artifact paths (local/private where appropriate): none (synthetic only)
Checks not run and exact reason: live AppKit preview of styled/portrait frames; H.264 export pixel compare of a non-uniform 9:16 layout (F1); hardware-qualified A1
Open gates / next smallest package: F1 live preview qualification of styled pixels; W1 if targeting Windows; H1/H2/H3 hardware qualification; `rect_16_9` webcam crop remains disabled

## S1 evidence — 2026-09-09

Work package ID / status: S1 / contract-tested (not hardware-qualified; H1/H2 remain open)
Commit + working-tree changes (including frontend tracking limitations): uncommitted real `detect_silence` project/track command, streaming frame-based detector, Jump Cuts modal/IPC; prior E1–E4/F1/F2/Z1/Z2/A1 work also remains uncommitted. Frontend gitlink may omit UI files from the parent diff.
OS / architecture / device / SDK / compiler / dependency versions: macOS 26.6.2 (25G83), aarch64-apple-darwin, rustc 1.98.1 (48a229cea 2026-09-01)
Fixture or real capture provenance / recording configuration: synthetic PCM16 WAV fixtures (contiguous silent segments, missing file, opposite-polarity stereo, mulaw); no live recording session
Reproduction commands and manual steps: `cargo test --manifest-path src-tauri/Cargo.toml --lib dsp`; `cargo test --manifest-path src-tauri/Cargo.toml --test silence_tests`; `cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app`; `npm run build` in `front-end/`; `git diff --check`
Expected outcome: silent run across two contiguous WAVs is one suggestion; missing/unreadable audio is a diagnostic gap, not a cut; opposite-polarity stereo is not silent; selected-channel timing uses sample frames; apply is one `project_ripple_cuts` revision; undo restores retained intervals; reopen preserves applied cuts; stale handles and zero/non-finite config are rejected
Observed outcome and measured values: dsp lib tests (7) and `silence_tests` (6) passed; `tauri-app` check succeeded; frontend `tsc && vite build` succeeded; `git diff --check` clean
Artifact paths (local/private where appropriate): none (synthetic only)
Checks not run and exact reason: live-mic jump cuts on a real recording; filler-word/ASR (deferred); H1/H2 hardware qualification
Open gates / next smallest package: F1/H1/H2 hardware qualification; filler-word recognition remains deferred

## H3 evidence — 2026-09-09

Work package ID / status: H3 / contract-tested (not hardware-qualified; F1 live exclusion/transparent-corners/focus remain open)
Commit + working-tree changes (including frontend tracking limitations): uncommitted HUD window-identity routing, Rust-owned revisioned HUD settings, HUD-only React root, F1 overlay attach on `camera_overlay` against the existing latest-frame mailbox; prior E1–E4/F1/F2/Z1/Z2/A1/S1 work also remains uncommitted. Frontend gitlink may omit UI files from the parent diff.
OS / architecture / device / SDK / compiler / dependency versions: macOS 26.6.2 (25G83), aarch64-apple-darwin, rustc 1.98.1 (48a229cea 2026-09-01)
Fixture or real capture provenance / recording configuration: command-level recording session in `AppState::new_test` (no native ScreenCaptureKit). No live HUD-on-display capture.
Reproduction commands and manual steps: `cargo test --manifest-path src-tauri/Cargo.toml --lib hud`; `cargo test --manifest-path src-tauri/Cargo.toml --test hud_tests`; `cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app`; `npm run build` in `front-end/`; `git diff --check`
Expected outcome: `camera_overlay` maps to HUD-only (unknown labels rejected); one settings revision across windows; stale updates rejected; dropped/out-of-order events recover from snapshot; camera removal is visible; `hud_close` leaves the recording session alive and does not start an independent capture session; HUD hidden during record until exclusion is hardware-qualified
Observed outcome and measured values: `hud` lib tests (6) and `hud_tests` (6) passed; `cargo check --features tauri-app` succeeded; frontend `tsc && vite build` succeeded; `git diff --check` clean
Artifact paths (local/private where appropriate): none (synthetic only)
Checks not run and exact reason: live ScreenCaptureKit self-exclusion with HUD visible; transparent-corner / squircle hit-testing on a real display; overlay keyboard focus; live AppKit present of the camera mailbox during a real recording (F1 hardware)
Open gates / next smallest package: F1 live exclusion/transparent-corners/focus; H1/H2 hardware qualification

## H2 evidence — 2026-09-09

Work package ID / status: H2 / contract-tested (not hardware-qualified; live Input Monitoring and mixed-DPI/click-to-frame experiments remain open)
Commit + working-tree changes (including frontend tracking limitations): uncommitted MouseHookMac permission/gap/shutdown contracts, native logger 100 ms sampling lock + boolean permission DTO, reader/zoom denied-gap tests, `mouse_telemetry_permission` serialization, MouseTelemetryControl copy that recording still works without tracking. Parallel A1/H1/H3 work also remains uncommitted.
OS / architecture / device / SDK / compiler / dependency versions: macOS 26.6.2 (25G83), aarch64-apple-darwin, rustc 1.98.1 (48a229cea 2026-09-01)
Fixture or real capture provenance / recording configuration: synthetic v2 JSONL and in-process Swift hook (no CGEventTap installed). No live display, no TCC grant/deny/revoke session.
Reproduction commands and manual steps: `cargo test --manifest-path src-tauri/Cargo.toml --lib telemetry`; `cargo test --manifest-path src-tauri/Cargo.toml --lib zoom`; `cargo test --manifest-path src-tauri/Cargo.toml --test zoom_tests`; `sh script/test-mouse-telemetry.sh`; `cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app`; `git diff --check`
Expected outcome: deny/revoke emit typed gaps and do not fabricate clicks or zooms; overflow is bounded and becomes a gap; stop ignores further events; permission JSON is `{supported, authorized}` not a capture bundle; geometry uncertainty stays 100 ms; application/window-without-physical stay unsupported
Observed outcome and measured values: `--lib telemetry` 15 passed; `--lib zoom` 10 passed including geometry uncertainty; `zoom_tests` 4 passed including denied suggestions; Swift mouse contracts passed (no event tap); `tauri-app` check succeeded; `git diff --check` clean
Artifact paths (local/private where appropriate): none (synthetic only)
Checks not run and exact reason: live grant/deny/revoke of Input Monitoring on a signed app; mixed-DPI window physical transforms; negative-origin window on a real desktop; click-to-frame pixel alignment against decoded media; event-tap timeout/user-disable on a live tap
Open gates / next smallest package: hardware capability-matrix in §10.4 H2; do not tighten the 100 ms adapter without measured evidence

## H1 evidence — 2026-09-09

Work package ID / status: H1 / contract-tested for the failure-injection slice only (not hardware-qualified)
Commit + working-tree changes (including frontend tracking limitations): uncommitted pause finalize-before-ack, native `pauseAndFinalize`, no-overwrite `commit_segment` aligned with `commit_native_segment`, pending-publication retry, session diagnostics on finalize/journal failure, retained ownership after prepare/stop failure; `src-tauri/tests/recording_lifecycle_tests.rs`. Parallel A1/H2/H3 work also remains uncommitted. No frontend changes.
OS / architecture / device / SDK / compiler / dependency versions: macOS 26.6.2 (25G83), aarch64-apple-darwin, rustc 1.98.1 (48a229cea 2026-09-01)
Fixture or real capture provenance / recording configuration: synthetic fMP4/WAV fixtures and `DurabilityFault` / journal inject hooks. No live ScreenCaptureKit session.
Reproduction commands and manual steps: `cargo test --manifest-path src-tauri/Cargo.toml --lib project::segment_writer`; `cargo test --manifest-path src-tauri/Cargo.toml --test recording_lifecycle_tests`; `cargo test --manifest-path src-tauri/Cargo.toml --test integration_tests`; `cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app`; `git diff --check`
Expected outcome: pause does not report Paused if finalize/`PauseStarted` fails; retry with the fault still injected does not invent success; resume after a successful pause journals a new segment interval and does not append to the closed file; injected sync/journal failure preserves prior commits; kill-after-commit (drop writer) leaves the journaled file; late webcam source times survive recovery; Stop does not scan leftover temps
Observed outcome and measured values: `project::segment_writer` 5 passed (including inject/pending retry); `recording_lifecycle_tests` 7 passed; `integration_tests` 24 passed; `cargo check --features tauri-app` succeeded; `git diff --check` clean
Artifact paths (local/private where appropriate): none (synthetic only)
Checks not run and exact reason: live ScreenCaptureKit 4-track recording; independent decode of hardware-finalized AVAssetWriter segments; pause-during-live-capture; `kill -9` after a real native commit; 60-minute four-track skew/memory/drop soak
Open gates / next smallest package: hardware procedure in §10.4 H1 (do not mark `hardware-qualified` from this CI slice)
