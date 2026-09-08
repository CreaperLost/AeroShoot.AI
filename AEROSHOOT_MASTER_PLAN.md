# AeroShoot.AI — Master Architectural & Implementation Plan

> **Persistent Blueprint & Development Roadmap**
> Target Platforms: macOS (Apple Silicon ARM64, macOS 13+) & Windows 11 (x86_64 / ARM64)
> Plan revision: 2026-09-08 (Post-GUI Overhaul & Real-Device Alignment)
> Current baseline: Completed two-scene studio frontend with dedicated Record Scene & Edit Studio Scene, physical hardware device deck (zero mock devices, real displays, mics with VU meter, webcams with hotplug), responsive aspect-ratio canvas (16:9, 9:16, 4:3, 1:1), and full-height multi-track timeline studio; shared Rust recording foundations (session, journal, segment writer, recovery); and in-progress macOS ScreenCaptureKit/AVFoundation capture bridge.
> Core Framework: Tauri v2 (Rust) + React / TypeScript / Vite + Native OS Capture Modules

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
| Desktop Studio & GUI | **Completed Overhaul**: Scene separation (`RecordScene` vs `EditStudioScene`) via top card; physical device deck (`DeviceControlDeck`) querying only real displays, webcams, and mics with hotplugging and animated VU meter; responsive multi-aspect canvas (`StudioCanvas`); floating dock; full-height multi-track `TimelineStudio`. | Connect live native media playback and waveform generation from `.aero` project bundles. |
| Shared recording foundations | Rust session state machine, monotonic clock, bounded queues, segment writer, project manifest, journal, and recovery engine implemented. | Failure-injection, concurrency, actual media validation, and cross-platform durability acceptance. |
| macOS capture | Swift ScreenCaptureKit/AVFoundation bridge (`SCKitBridge.swift` & `src-tauri/src/capture/macos.rs`); AVAssetWriter prototype. | Shared clock mapping and durable segment commit integration with Rust `SegmentWriter`; real-device qualification. |
| Mouse telemetry | Event schema, coordinate normalization, and geometry tracking contracts specified. | Native macOS `CGEventTap` (`MouseHookMac.swift`) integration logging real click/cursor events to `telemetry/events.jsonl`. |
| Native preview and export | Architecture specified; timeline editor and canvas playback UI completed. | Phase 0 embedding/interoperability experiments, then Phase 4 parity tests. |
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
* **Existing bridge integration**: The current `AeroShootCapture.swift` uses `AVAssetWriter` for one output file per track, with a video fragment interval. Treat it as an integration prototype until it satisfies the common timestamp, segment, journal, and shutdown contracts. A hardware encoder probe alone does not prove the production path or concurrent encoder capacity.
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

* The next telemetry schema uses a tagged payload: `move`; `button_down`/`button_up` with a button identifier (including auxiliary buttons); `scroll` with signed X/Y deltas and explicit units; `cursor_changed` with an asset/shape reference and hotspot; and `gap` with reason and affected time interval. Preserve platform scroll phase/precision when available. Modifier state is optional and must distinguish unknown from an empty set.
* Button transitions are authoritative. Clicks and double-clicks are derived annotations with references to their originating transitions; consumers must not animate both a derived click and its source transition as separate clicks. Maintain held-button state and reset it to unknown across telemetry gaps until resynchronized.
* Version payload changes explicitly. Existing v1 `Click` records have unknown button/derivation provenance; do not invent it during migration. Validate geometry and cursor references, and bound cursor asset sizes and cache memory.
* macOS permission preflight distinguishes listening/Input Monitoring from event modification/Accessibility. Use the listening-access APIs where applicable and test the actual event mask, tap location, signed app, and supported OS versions. Handle timeout/user-input tap-disable notifications: record the gap, revalidate authorization, and re-enable only where appropriate; do not repeatedly prompt or restart after an explicit denial.
* `NSCursor.current` is application-local; `currentSystem` was documented as system-wide but is deprecated and is not a dependable cross-version foundation. Qualify public cursor extraction on supported versions. If reliable extraction is unavailable, use the baked-cursor mode already defined above. Do not assume standard global cursor-shape identification is guaranteed, and do not use private cursor APIs.
* Windows `MSLLHOOKSTRUCT.pt` is documented as per-monitor-aware screen coordinates. Define the conversion to source physical pixels once and test negative desktop origins, rotation, mixed DPI, and moved windows. The hook does not provide stationary cursor-shape notifications; use a bounded cursor polling adapter or another qualified mechanism. Keep callbacks immediate, account for silent timeout removal, and report telemetry-health uncertainty rather than promising reliable removal detection. Do not hardcode an assumed 200 ms timeout.

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

### Phase 1: Shared Recording Foundations & Studio Shell (Complete)

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
   - Connect the Swift `SCKitBridge.swift` / `AVAssetWriter` delegate buffers directly into Rust's `SegmentWriter` and `Journal`.
   - Ensure native ScreenCaptureKit screen frames, system audio loopback, webcam frames, and microphone PCM chunks write out verified, timestamped `.mp4` and `.wav` segments into the active `.aero/media/` project bundle.
   - Verify that clicking `Start Recording` in the UI records real media and `Stop Recording` commits the project bundle.

2. **Step 2: Global Mouse Telemetry Capture (`CGEventTap` on macOS)**:
   - Wire `MouseHookMac.swift` using a passive listen-only `CGEventTap` to record mouse position, clicks, and dwell times.
   - Stream normalized coordinates and button transitions into `telemetry/events.jsonl` aligned with the monotonic session clock.

3. **Step 3: Real Media Playback & Waveforms in Edit Studio (Phase 4)**:
   - Connect the `EditStudioScene` video preview to playback the recorded screen and webcam video tracks from the `.aero` project bundle.
   - Generate true audio waveforms (RMS/peak) on the `TimelineStudio` mic and system audio tracks instead of sample placeholder waveforms.

4. **Step 4: Smart Auto-Zoom Keyframing Algorithm (Phase 5)**:
   - Implement the telemetry parser that scans `events.jsonl` for high-interest clusters (clicks, typing, cursor dwell) and automatically generates smooth cubic Bézier zoom keyframe blocks on the timeline.
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
