# macOS Native Capture Acceptance Review

## Verdict

**Request changes.** The macOS bridge compiles and establishes the intended framework integration,
but the vertical slice is not acceptance-ready. The principal blockers are invalid initial UI
selections, lack of recoverable segment commits, unchecked encoder finalization, and timestamps that
are not mapped from the native media clocks.

This document consolidates the two reviews, removes overlapping findings, and corrects claims that
no longer match the repository.

## Acceptance-blocking findings

### 1. [P1] Initial UI selections are not valid native identifiers

The settings store initializes `selectedSourceId`, `selectedCameraId`, and `selectedMicId` with the
synthetic identifiers `screen-main`, `cam-facetime`, and `mic-macbook`
([`settingsStore.ts:25`](front-end/src/stores/settingsStore.ts#L25)). The native bridge accepts source
IDs in `display:<CGDirectDisplayID>`, `window:<SCWindowID>`, or `application:<bundle-id>` form and
camera/microphone IDs from `AVCaptureDevice.uniqueID`
([`AeroShootCapture.swift:283`](src-tauri/native/macos/AeroShootCapture.swift#L283)).

The HUD loads real sources and devices but only stores them in component state; it does not replace
missing or invalid selections ([`RecordingHUD.tsx:44`](front-end/src/components/recording-hud/RecordingHUD.tsx#L44)).
The first native recording therefore fails unless the user manually selects each input.

**Required change:** After enumeration, preserve a selection only if its ID still exists. Otherwise,
select the main/first display, the device marked `isDefault`, or `undefined` when no optional device
exists. Disable recording until source enumeration and selection reconciliation have completed.

### 2. [P1] Native output is neither independently segmented nor recoverable after a crash

Each native track writes one long temporary file
([`AeroShootCapture.swift:263`](src-tauri/native/macos/AeroShootCapture.swift#L263)). The two-second
`movieFragmentInterval` creates fragments inside that file; it does not create independently
committed two-second project segments. Rust renames and journals the file only during clean shutdown
([`commands/mod.rs:650`](src-tauri/src/commands/mod.rs#L650)).

Recovery scans only files whose extension is `.mp4` or `.wav`, so `*.mp4.tmp` and `*.wav.tmp` are
ignored ([`recovery.rs:228`](src-tauri/src/project/recovery.rs#L228)). A crash before `stop` therefore
leaves no committed native media in the journal, and the current recovery path cannot salvage the
temporary output. Fragmentation alone is not a crash-durability guarantee.

**Required change:** Rotate every track into short, independently decodable segment files beginning
with a keyframe. For each segment, finish the writer, validate the container, sync the file, atomically
rename it, sync the parent directory, and then append the journal record. Recovery must separately
recognize temporary files and salvage only fragments that pass demux/container validation.

### 3. [P1] Failed or timed-out encoders can be committed as successful media

`MediaWriter.append` returns the same `false` for temporary backpressure, `startWriting` failure, and
terminal writer failure ([`AeroShootCapture.swift:192`](src-tauri/native/macos/AeroShootCapture.swift#L192)).
`finish` ignores its 15-second timeout as well as `AVAssetWriter.status` and `AVAssetWriter.error`
([`AeroShootCapture.swift:202`](src-tauri/native/macos/AeroShootCapture.swift#L202)). The C stop function
cannot return an error, and Rust commits every non-empty output without calling `MediaValidator`
([`commands/mod.rs:667`](src-tauri/src/commands/mod.rs#L667)). See Apple's
[`AVAssetWriterInput.append` documentation](https://developer.apple.com/documentation/avfoundation/avassetwriterinput/append%28_%3A?changes=_9_5&language=objc).

**Required change:** Give append/finalization typed outcomes such as `accepted`, `backpressured`, and
`failed`. Propagate writer error and timeout information through the C ABI. A stop operation must fail
or produce an explicit recoverable-session result unless every required writer has completed and each
output passes media validation. Never rename or journal a failed file as committed media.

### 4. [P1] Timestamp mapping is based on queue execution rather than native clock correlation

The first timestamp for each track is paired with `DispatchTime.now()` inside an asynchronously
queued logging block ([`AeroShootCapture.swift:123`](src-tauri/native/macos/AeroShootCapture.swift#L123)).
Logger backlog therefore changes the track offset. Moving that call before `queue.async` would remove
one source of delay but would still anchor independent tracks to callback arrival rather than their
media clocks.

ScreenCaptureKit exposes `SCStream.synchronizationClock` for synchronizing its output with other
media sources. AVFoundation sample timestamps likewise need conversion from the capture session's
clock into the same host-clock domain. The current implementation uses neither. See Apple's
[`SCStream.synchronizationClock` documentation](https://developer.apple.com/documentation/screencapturekit/scstream/synchronizationclock?changes=_7_2&language=objc).

The recovery validator compounds the problem by converting `tfdt` ticks using a hard-coded 90 kHz
timebase ([`media_validator.rs:344`](src-tauri/src/project/media_validator.rs#L344)), although an
AVFoundation-authored track may use a different timescale.

**Required change:** Establish and retain rational clock-to-host anchors at session start. Convert
ScreenCaptureKit and AVFoundation PTS into that common host domain, preserve original value/timescale,
and persist mapping and discontinuity metadata. Parse the MP4 track timescale from `mdhd` rather than
assuming 90 kHz.

## Additional required hardening

### 5. [P1/P2] The permission API exists, but the user flow is fire-and-forget and unused

The backend command is exposed through Tauri and the frontend API, so the earlier claim that it is
missing is stale. However, camera and microphone authorization completions are discarded
([`AeroShootCapture.swift:103`](src-tauri/native/macos/AeroShootCapture.swift#L103)), Rust immediately
returns another preflight result ([`commands/mod.rs:206`](src-tauri/src/commands/mod.rs#L206)), and no
frontend component calls `requestCapturePermissions`.

**Required change:** Make authorization an asynchronous operation that resolves after all requested
permissions reach a terminal status. Represent at least `notDetermined`, `authorized`, `denied`, and
`restricted`; display actionable UI and refresh it after prompts. Screen Recording may require the
user to visit System Settings or restart capture, which should be represented explicitly.

### 6. [P2] Capture startup and runtime failures do not reach the session state machine

The ScreenCaptureKit start wait result is ignored, so a timeout with no completion error is treated as
success ([`AeroShootCapture.swift:315`](src-tauri/native/macos/AeroShootCapture.swift#L315)). Runtime
ScreenCaptureKit errors only update `lastError`
([`AeroShootCapture.swift:383`](src-tauri/native/macos/AeroShootCapture.swift#L383)); Rust deserializes
that field but omits it from `SessionStatusResult`. AVFoundation runtime errors, device disconnection,
and media-service resets are not observed.

**Required change:** Treat start/stop timeouts as errors, surface native runtime failures to Rust, and
transition the session to the appropriate failed or recoverable state. Observe AVFoundation runtime
notifications and device connection changes. Do not silently switch devices.

### 7. [P2] Fixed output dimensions lack an explicit source-geometry contract

Rust selects a fixed 16:9 output size from the quality preset
([`commands/mod.rs:285`](src-tauri/src/commands/mod.rs#L285)), even for 16:10 displays and arbitrary
window shapes. It does not calculate or persist an active content rectangle. The earlier assertion
that ScreenCaptureKit necessarily stretches the image is too strong—current APIs preserve aspect
ratio by default—but implicit framework behavior is insufficient for telemetry and later rendering.

**Required change:** Explicitly calculate source and destination rectangles, define fit/crop behavior,
and persist the active content rectangle with the geometry revision. Gate newer
`preservesAspectRatio` behavior by OS availability while retaining a correct macOS 13 path.

### 8. [P2] Timestamp logging has unbounded queueing and excessive small writes

Every video or audio sample submits another asynchronous block and normally writes one JSON line
([`AeroShootCapture.swift:126`](src-tauri/native/macos/AeroShootCapture.swift#L126)). The queue is
unbounded, so slow storage can cause growing memory consumption while also generating frequent small
I/O operations.

**Required change:** Use a bounded record queue and batched writes. Coalesce optional high-frequency
records where safe, never discard essential discontinuities silently, and persist an explicit gap or
overflow counter when the queue saturates.

### 9. [P2] Development self-exclusion is unreliable when the main bundle ID is absent

Display filtering identifies AeroShoot only by `Bundle.main.bundleIdentifier`
([`AeroShootCapture.swift:287`](src-tauri/native/macos/AeroShootCapture.swift#L287)). In development or
test launches that value can be `nil`; comparing optionals can also match an unrelated application
whose bundle identifier is absent.

**Required change:** Match `SCRunningApplication.processID` against
`ProcessInfo.processInfo.processIdentifier`, using the bundle identifier only as an additional packaged
application check.

### 10. [P3] Device discovery can adopt newer camera types without dropping macOS 13

The current discovery list already includes `.externalUnknown`, so the assertion that external
webcams are invisible is incorrect. Continuity Cameras can also report as built-in wide-angle cameras
when an application has not opted into the dedicated macOS 14 type. Nevertheless, discovery can be
more complete for Continuity Camera and Desk View.

**Recommended change:** Keep the macOS 13-compatible types, add Desk View where supported, and on
macOS 14+ use `.external` and `.continuityCamera` behind availability checks. Add
`NSCameraUseContinuityCameraDeviceType` only if the product intends to opt into the dedicated
Continuity Camera type.

## Acceptance criteria

The native slice can be accepted when:

1. A fresh launch can start recording without manually replacing synthetic IDs.
2. A forced termination leaves at least the previously committed segment interval recoverable.
3. Writer failure, device removal, and capture start/stop timeout tests cannot produce a successful
   committed journal record.
4. Screen, webcam, system audio, and microphone timestamps map reproducibly to one host-clock epoch,
   including late starts and discontinuities.
5. Permission prompts complete through the UI and return terminal, typed authorization states.
6. Non-16:9 sources retain correct geometry and telemetry mapping.
7. Sustained logger or encoder backpressure remains bounded and is visible in session diagnostics.

