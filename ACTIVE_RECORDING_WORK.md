# AeroShoot recording recovery plan

Updated: 13 September 2026  
Active platform: macOS 13+ on Apple Silicon  
Deferred platform: Windows 11, after macOS qualification

This file is the short, executable companion to `AEROSHOOT_MASTER_PLAN.md`. The master plan defines the target architecture; this file records the current defects, ordering, and proof required before a feature is called working.

## Product contract

The recorder produces independent screen video, webcam video, system-audio, microphone-audio, and mouse-telemetry streams. Every media source can be disabled independently. Screen-off camera/mic/audio-only sessions are implemented across the frontend, Rust manifest/lifecycle, and macOS bridge; they remain hardware-unqualified. A session with no selected media tracks is rejected.

Media is committed in independently recoverable segments no longer than 60 seconds. The native writers rotate every 60 seconds so screen, webcam, system audio, and microphone tracks use the same segment cadence.

Mouse telemetry is append-only JSONL beside the media. It records session-relative timestamps, source geometry revisions, unclamped normalized positions, button transitions, scrolls, and explicit gaps. Missing Input Monitoring permission must never stop media recording.

## Current audit

| Capability | Current state | Next proof or fix |
| --- | --- | --- |
| Screen recording | Implemented with ScreenCaptureKit and can be disabled; system audio still uses a ScreenCaptureKit source | Fresh 2-minute and 15-minute hardware recordings plus camera-only, mic-only, and system-only combinations; decode every segment and compare media end times |
| Webcam recording | Optional isolated H.264 track; explicitly choosing a camera requests only Camera permission so its native preview is available before Record | Test camera enabled, disabled, busy, unplugged, and reconnect on Apple Silicon |
| System audio | Optional isolated PCM track; native preflight and recording-time meters implemented; missing CoreMedia durations preserve the full buffer frame count | Hardware-test silence, playback, headphones, and output-device changes |
| Microphone | Optional isolated PCM track; explicitly choosing a mic requests only Microphone permission so its native meter is available before Record. `No Microphone` is supported; production gain covers −24…+24 dB continuously. Missing CoreMedia durations use buffer frame count and native sample rate. | Test no-mic persistence, gain, hotplug, and permission denial in the signed app |
| Screen/webcam preview | Native AppKit surface | Device menus update occlusion on one continuously attached native surface; they no longer detach/reattach and race AppKit above the dropdown. Menus expose keyboard/ARIA behavior, selectors collapse at narrow widths, and the non-functional aspect-ratio selector is removed. User hardware/UI qualification remains. |
| Audio previews | Idle mic and system-audio meters now come from the native preview session; the selected AVFoundation mic ID and configured gain are shared with recording. Recording-time native meters and sample counters remain visible. | Hardware-qualify silence, gain changes, and device switching in the signed app |
| Source enumeration | The native app accepts only ScreenCaptureKit/AVFoundation IDs returned by its Rust/Swift bridge; browser probes and synthetic display fallbacks are limited to browser-mode development. Native enumeration failures are surfaced inline and remain distinct from genuinely empty device lists. | Hardware-qualify hotplug and permission changes |
| Mouse telemetry | Native v2 stream normalizes against live Quartz source bounds and records physical display transforms. `qualification.json` now summarizes usable geometry, moves, button transitions, scrolls, gaps, dropped events, and parser diagnostics. | User signed-app grant/deny/revoke checks plus click-to-frame and mixed-DPI validation |
| Segments | Native rotation is 60 seconds for every enabled track; startup rejects intervals outside `(0, 60]`; each native commit is structurally validated; Stop now rejects missing media for any manifest track and any journaled segment over 60 seconds | Validate duration and playback of every segment in long hardware sessions; run crash/kill and recovery tests |
| Capture health | Start remains `Preparing` until selected screen/webcam/mic sources deliver their first native sample; system-audio-only uses successful ScreenCaptureKit startup because silence yields no buffers. The recording row reports freshness, peaks, committed segments, and preserves the first terminal native error. | Hardware-qualify the two-second live stall threshold |
| Qualification artifact | Stop writes schema-v2 `qualification.json`. `passed` is false for missing media, segments over 60 seconds, discontinuities/two-second final-tail stalls, any native runtime error, or cross-track end skew over two seconds. Machine-readable failure codes explain every rejection. | User hardware sessions should retain these reports with the relevant OS/device notes |
| Automated gates | Frontend build and Rust unit tests pass | Add frontend component tests and make integration-test stubs link without the Swift bridge |

## Ordered work

### P0 — Make failures visible and reproducible

- Keep the implemented recording health row visible through terminal session errors. It preserves the first non-recoverable native error beside enabled tracks, native audio peaks, two-second freshness detection, and committed segment counts.
- Review the schema-v2 `qualification.json` after hardware sessions. Its pass/fail result covers missing media, segment duration, stalls/discontinuities, runtime errors, and a documented two-second cross-track end-skew limit.
- Hardware-qualify the implemented start-ready gate: selected screen/webcam/mic sources must produce a first native sample within five seconds before the state becomes `Recording`; system-audio-only relies on successful ScreenCaptureKit startup so silence remains valid.
- Keep one documented hardware smoke procedure and save its generated report with OS, hardware, commit, permissions, and selected devices.

Acceptance: a stalled screen/camera track is visible within two seconds and Stop never reports a clean completion when an enabled track has no valid media.

### P1 — Finish source semantics and previews

- Hardware-qualify the native mic/system-audio preflight meters and compact four-source recording status row. The browser `getUserMedia` mic probe has been removed so preview and recording cannot disagree about or contend for the selected device.
- Keep the implemented explicit Off choices and stable persistence for every optional source.
- Keep Record readiness based on effective sources: denied/restricted camera or mic selections do not count as recordable media, while `notDetermined` remains actionable through the explicit device-selection permission flow.
- Hardware-qualify the implemented screen-off sessions (camera-only/audio-only). The bridge keeps a display identifier only as ScreenCaptureKit context for system audio; it does not create a screen track, geometry, or mouse log when screen is off. Reject only when all media sources are off.
- Keep native preview visibility tied to modal/menu occlusion; native AppKit views always sit above web content, so CSS stacking is not a solution.

Acceptance: every valid source combination records exactly the selected tracks, previews/meters only the selected sources, and permission denial removes only the affected optional track.

### P2 — Long-session and recovery qualification

- Run 2-, 15-, and 60-minute sessions at 1080p30, then 1080p60; gate 4K and 60 fps on measured encoder/thermal capacity.
- Validate all MP4/WAV segments, monotonic timestamps, independent keyframe starts, A/V end skew, pause/resume boundaries, and recovery after forced termination.
- Exercise static-screen, camera motion, silence, loud audio, device loss, display sleep, low disk, and app quit while stopping.

Acceptance: no enabled track stalls; each segment is independently readable and at most 60 seconds; final per-track duration differs from the session by only a documented bounded tail.

### P3 — UI consolidation

- Keep the four-source strip, predictable preview stage, and recording/status bar compact. Recording-time aspect-ratio controls are removed because they did not alter captured media.
- Permission problems are shown inside the affected source card and no longer move the entire canvas vertically.
- Device menus provide focus rings, accessible names/state, Escape and arrow-key navigation, viewport-bounded panels, and two/one-column narrow-window layouts.
- Capture visual regression screenshots for menu-open and permission states during user UI qualification.

Acceptance: controls never overlap the native preview at supported window sizes and recording state is understandable without reading diagnostic prose.

### P4 — Windows final iteration

Only begin after P0–P3 pass on macOS ARM. Preserve the Rust session/project/telemetry contracts; add Windows Graphics Capture, WASAPI loopback/input, Media Foundation camera/encoding, and a Direct3D preview adapter. Qualify Windows x64 first and ARM64 separately.

## Verification commands

```sh
cd front-end && npm run build
cd src-tauri && cargo test --lib --no-default-features
sh script/test-native-preview.sh
sh script/test-recording-writer.sh
sh script/test-mouse-telemetry.sh
sh script/test-mic-gain.sh
```

Native preview/writer tests use AVFoundation and macOS graphics services. A sandbox abort is an environment limitation, not a pass; rerun on the signed local app/tooling host and record the result.

Use `./script/codex.sh install` for the deterministic production flow: local frontend build, direct Cargo/Swift release build, checked-in app assembly, stable signing, signature verification, and replacement of `/Applications/AeroShoot.app`. It never downloads an unpinned Tauri CLI or makes DMG creation a prerequisite; pass `--dmg` explicitly when an installer image is needed.

The user owns interactive hardware/UI qualification for this iteration.
