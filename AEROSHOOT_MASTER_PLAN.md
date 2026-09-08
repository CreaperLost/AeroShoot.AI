# AeroShoot.AI — Master Architectural & Implementation Plan

> **Persistent Blueprint & Development Roadmap**
> Target Platforms: macOS (Apple Silicon ARM64, macOS 13+) & Windows 11 (x86_64 / ARM64)
> Architecture review: 2026-09-07. The repository currently contains the Phase 1 shared recording foundations and a synthetic studio shell. Native capture, preview, export, and performance targets below remain planned and unbenchmarked.
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

---

## 2. Technical Architecture & Component Breakdown

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                             FRONTEND (TAURI V2)                             │
│       React + TypeScript       + Vite + Tailwind CSS + Lucide Icons         │
│  ┌───────────────────────┬─────────────────────────┬─────────────────────┐  │
│  │   Recording Overlay   │   Floating Webcam HUD   │   Timeline Studio   │  │
│  │   & Source Selector   │   (Frameless Window)    │   & Video Editor    │  │
│  └───────────────────────┴─────────────────────────┴─────────────────────┘  │
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
│  (Swift / Objective-C++)     │              │     (C++20 / WinRT)     │
│  - ScreenCaptureKit (SCKit)  │              │  - Windows.Graphics.Capture  │
│    (Display/Window/App/Audio)│              │    (Direct3D11 / DXGI)       │
│  - AVFoundation (Webcam/Mic) │              │  - Media Foundation           
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

#### B. Windows 11 Engine (x86_64 / ARM64)

* **Screen Capture**: `Windows.Graphics.Capture` (WGC) backed by Direct3D11 GPU textures. Avoid CPU staging readback on the normal recording path; retain/copy GPU resources with explicit synchronization as required by the frame pool and encoder.
  * Native window/display capture. Border suppression requires supported APIs, applicable packaging/capability configuration, and successful user consent via `GraphicsCaptureAccess.RequestAccessAsync(Borderless)`; retain the system border if unavailable or denied.
  * Handle resize, minimized/closed windows, display removal, rotation, and per-monitor DPI explicitly. Desktop Duplication captures a display and is not an equivalent fallback for isolated window capture; never silently broaden the capture scope. Stop or ask for source reselection when the selected source becomes unavailable.
* **System Audio**: `WASAPI` loopback mode (`AUDCLNT_STREAMFLAGS_LOOPBACK`) on the default playback endpoint.
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

---

### 3.5 Timeline Studio & Visual Editor

The editor uses React and Canvas for timeline controls, with a native media preview surface. One Rust timeline evaluator and WGPU scene compositor drive both preview and export; do not implement independent WebGL and native renderers with divergent styling or keyframe behavior. Native decoders feed bounded frame queues; one audio playback clock drives preview, and seeking cancels stale decode work. Proxies, thumbnails, and waveforms are background, cancelable caches.

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
  * Implemented in Rust via SIMD-accelerated sample scanning.
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

### 3.8 Session Clock, Timeline, and Cut Semantics

* Establish one monotonic session epoch **before** starting capture. Map each native source timestamp/timebase to that epoch, preserving original timestamps and mapping metadata. Never substitute callback arrival time for media PTS. For macOS bridge media clocks to host time; for Windows map WGC timestamps and WASAPI device/QPC positions to the session clock.
* Persist integer microseconds for project/telemetry times and retain rational codec timebases and audio sample counts. Detect discontinuities and estimate audio clock drift over long sessions; use bounded adaptive resampling for playback/export alignment, preserving original source data and corrections in metadata.
* Choose fixed encoded dimensions per recording track; fit resized source content into that canvas and persist its active content rectangle for telemetry transforms. If codec parameters must change, open a new segment and record its format revision. Variable-rate capture retains actual timestamps; constant-rate export samples the timeline explicitly.
* Late-starting or disconnected tracks have explicit gaps, not an invented zero offset. Screen gaps hold the last valid frame (black if none); webcam gaps hide the bubble; missing audio becomes silence. Source closure stops screen capture recoverably; camera/mic loss can continue with a visible warning. Do not silently switch audio endpoints or capture sources.
* State machine: `Idle → Preparing → Recording → Paused → Recording → Stopping → Completed`, with explicit failed/recoverable outcomes. Commands are serialized and start/stop are idempotent. Pause retains the monotonic source clock and records a common excluded interval; resume opens new segments. Sleep/lock interrupts capture and requires explicit resume after revalidation. Stop drains callbacks/encoders before closing muxers.
* Timeline uses sorted, non-overlapping, half-open retained source intervals `[start_us, end_us)`. Edited time maps to source time through their cumulative lengths. Every video, audio, telemetry event, and source-anchored zoom keyframe uses that same mapping. Cuts never rewrite source files. Define interpolation at cuts to avoid panning through removed material; keep edits reversible with undo/redo.

### 3.9 Ownership, Backpressure, and IPC

* Rust owns sessions, project revisions, jobs, and persistence. React holds view state and submits revision-checked commands; multiple windows subscribe to snapshots/events and cannot independently mutate a recording session.
* Native FFI uses opaque handles, explicit retain/release ownership, versioned structures, error codes, and a shutdown handshake. No Rust panic or native exception crosses the C ABI. Callbacks must stop before resources are freed.
* Capture callbacks enqueue references into bounded queues and never wait for disk, encoding, rendering, or UI. Saturated video queues drop frames with timestamp/gap counters; audio overflow creates an explicit discontinuity and error status. Sustained overload triggers a controlled stop, not unbounded allocation.
* IPC carries commands, metadata, throttled meters, and job status. All raw media stays in native/Rust processing. Scope Tauri capabilities per window; validate project paths and resource IDs, restrict local media access to the opened project, and disallow arbitrary shell execution or remote navigation in privileged windows.

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
│       │   ├── recording-hud/    # Source selector, resolution/fps picker
│       │   ├── camera-overlay/   # Floating webcam bubble UI
│       │   ├── timeline/         # Multi-track timeline & playhead
│       │   ├── waveform/         # Audio waveform renderer
│       │   ├── inspector/        # Zoom settings, background styling, shapes
│       │   └── silence-modal/    # Silence detection threshold & jump-cut controls
│       ├── hooks/
│       │   ├── useRecording.ts
│       │   └── useTimeline.ts
│       └── stores/
│           ├── projectStore.ts   # Zustand state for timeline & keyframes
│           └── settingsStore.ts  # Device & quality preferences
└── AEROSHOOT_MASTER_PLAN.md      # This persistent master blueprint
```

---

## 5. Phased Implementation Roadmap

### Phase 0: Architecture Feasibility Gates

* Pin toolchains and one React major; define platform capability reports and a device/OS qualification matrix. macOS 13+ ARM64 and Windows 11 x64 are first targets; Windows ARM64 remains a planned target until independently qualified.
* Build minimal native spikes on **both** platforms: screen + webcam hardware encoding concurrently, system/mic timestamps, cursor exclusion, and HUD exclusion.
* Prove native preview surface embedding and WGPU-to-encoder interoperability. Select and document bounded-copy fallbacks where necessary.
* Exit gate: short synchronized recordings and a styled preview/export on both systems, with measured queue memory, copy cost, encoder availability, and dependency packaging feasibility. Resolve failed gates before building the full editor.

### Phase 1: Shared Recording Foundations & Minimal Shell

* Initialize Tauri v2, React/TypeScript/Vite, Tailwind and Zustand, with least-privilege window capabilities.
* Implement clock/timebase contracts, session state machine, native ownership interfaces, bounded queues, manifest schema, segment writer, journal, and recovery **before** full native integrations.
* Add synthetic timestamp/packet fixtures covering offsets, drift, gaps, pause/resume and truncated storage. Build permission/capability status and a minimal source selector.

### Phase 2: macOS Vertical Slice

* Integrate ScreenCaptureKit screen/system audio, AVFoundation camera/mic, VideoToolbox encoding and the shared muxer.
* Wire native HUD preview, permission handling, cursor events and changing source geometry into the common session clock and bundle writer.
* Exit gate: a 60-minute 1080p30 recording with both audio tracks and webcam; target audio/video skew ≤ 20 ms, bounded queue memory, and reported frame drops. Test forced termination, disk-full, denial/revocation, resize, pause/resume, and device removal. These are acceptance targets, not measured results.

### Phase 3: Windows Vertical Slice & Platform Parity

* Integrate WGC/D3D11, WASAPI loopback and mic, Media Foundation camera/encoder, and cursor/HUD handling using the same contracts.
* Repeat Phase 2 gates on qualified x64 hardware; test mixed DPI, GPU/device loss, endpoint changes, border consent, and minimize/close behavior. Qualify ARM64 separately before shipping that binary.

### Phase 4: Shared Preview, Timeline & Basic Export

* Implement native decode/playback, the source-to-edited-time evaluator, WGPU compositor, and MP4 H.264/AAC export before advanced styling.
* Add cached thumbnails/waveforms, multi-track timeline, reversible trims, webcam layout, cursor rendering, and basic background styling.
* Exit gate: seek/cut fixtures align all tracks; representative preview frames match exported frames within a documented codec/color tolerance; export cancellation preserves sources and previous outputs.

### Phase 5: Smart Editing & Advanced Styling

* Add telemetry-generated zoom trajectories, editable keyframes, canvas and camera styling, deterministic silence suggestions and reviewed ripple cuts.
* Test cuts through zooms, source geometry changes, silent/missing mic tracks, and undo/redo. Silence padding is user-tunable and suggestions remain reversible.

### Phase 6: Performance Qualification & Distribution

* Benchmark 1080p60/4K modes, encoder contention, long sessions, thermal behavior, memory and disk throughput; enable only supported combinations.
* Add signed/notarized macOS packages and signed Windows installers, dependency notices, CI for shared tests and platform builds, plus hardware smoke tests. OS capture/permission/GPU behavior requires real-device checks beyond hosted CI.
* Defer HDR, ProRes, speech/filler recognition, and vendor-specific encoder SDKs until the baseline is stable and their capability/licensing requirements are resolved.

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
4. **Performance Rule**: Never copy raw video frames across the Tauri IPC boundary (between Rust and Webview). The webview is strictly for controls, preview playback, and editing UI. All recording, compositing, and encoding must remain in native/Rust threads.

## 8. Architecture Review References

Reviewed 2026-09-07. These sources establish platform behavior; performance, native surface embedding, and recovery durability still require the Phase 0–3 experiments.

* [Apple: SCStreamOutput](https://developer.apple.com/documentation/screencapturekit/scstreamoutput) — output samples are separate from stream lifecycle callbacks.
* [Apple: ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit) — screen capture and its permission flow.
* [Microsoft: IsBorderRequired](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired) — border suppression is consent/capability dependent.
* [Microsoft: SetWindowDisplayAffinity](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity) — owned-window capture exclusion and its limits.
* [Microsoft: IAudioClock2::GetDevicePosition](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclock2-getdeviceposition) — device position and QPC correlation.
* [FFmpeg: format/muxer documentation](https://ffmpeg.org/ffmpeg-formats.html) — fragmented MOV/MP4 behavior; durability and project recovery need additional application logic.
