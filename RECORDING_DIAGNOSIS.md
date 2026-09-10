# Recording failure investigation — 10 September 2026

The saved projects in `~/Documents/AeroShootRec` confirm that recording continued while video stopped arriving. These are native capture/preview and timestamp bugs, not a missing browser recording API.

## Evidence from existing recordings

| Project | Take length | Screen media length | Webcam problem |
| --- | ---: | ---: | --- |
| Untitled 9 Sep 2026 | 10.53 s | 0.37 s | Ends around 5.5 s; first segment has an uptime-sized duration |
| Untitled 10 Sep 2026 | 4.99 s | 0.55 s | First timestamp is zero; next timestamp jumps to system uptime |
| Untitled 10 Sep 2026 2 | 10.94 s | 0.37 s | Stops around 5.2 s |
| Untitled 10 Sep 2026 3 | 16.05 s | 0.40 s | First segment claims 215,575 seconds, despite containing only 70 video frames |

The screen timestamp journals contain only 11–16 samples. Microphone and system-audio samples continue to the end of the takes. `ffprobe` independently confirms the malformed webcam media duration in the last project. Existing projects were inspected without modifying their files.

## Causes and corrections

1. **Preview rendering retains capture buffers.** The Rust playback thread repeatedly calls `livePreviewRead` and `livePreviewReadCamera`. Core Image creates autoreleased render objects that retain the source IOSurfaces. Rust's worker has no AppKit event loop to drain those objects. The small ScreenCaptureKit buffer pool runs out first; camera capture runs out later. The regression test reproduces this with synthetic frames: the original implementation exhausts a six-buffer pool at frame index 6, returning `kCVReturnWouldExceedAllocationThreshold` (-6689). Both native preview read functions now drain an autorelease pool on every call. Capture callback queues also drain per work item.

2. **A zero-time webcam startup sample poisons the recording clock.** The previous mapper anchored each track to its first callback, accepting a timestamp of zero. The next real camera sample uses host uptime, so the first segment becomes tens of hours long. Sample timestamps now convert from the documented `SCStream.synchronizationClock` or `AVCaptureSession.synchronizationClock` into the host clock, then subtract the shared recording epoch. Samples before the session epoch, invalid timestamps, and backward timestamps are rejected before reaching the writer. Callback scheduling no longer determines audio/video offsets.

3. **Segment finalization permits overlapping appends.** `commitSegment` marked its input finished and then unlocked the writer while waiting. Capture callbacks could access that finished input. Finalization now keeps the same lock through writer completion and publication. The completion handler only signals a semaphore, so it does not need that lock.

4. **Unrelated camera session notifications can fail a take.** The recorder subscribed to process-wide AVCapture notifications. The handlers now check that the stopped/failed session or disconnected device belongs to the recording session.

## Verification

`sh script/test-recording-writer.sh` **passed** with macOS graphics-service access. All 180 screen frames and 180 camera frames reused a six-buffer pool; native H.264 and PCM writing committed two segments for each of the four tracks; startup, invalid, and backward timestamp checks passed. Before the preview fix, the same test failed at frame index 6. These are synthetic-media tests and do not use the screen, camera, or microphone.

The backend checks also **passed: 25 tests** (11 recording lifecycle, 5 media interoperability, 9 project reader). Native media tests require macOS media-service access; the sandboxed run aborted in AVFoundation, and the unrestricted fixture-based run passed all tests.

The frontend and optimized desktop release built successfully. The rebuilt app is at `src-tauri/target/release/bundle/macos/AeroShoot.app`. Its signature was verified after applying the same stable designated requirement used by the project's build script. The installed copy in `/Applications` has not been replaced.

A fresh recording in the rebuilt desktop app is required to verify actual device capture end to end. Existing files cannot supply frames that were never recorded.

## Apple API references

- [Autorelease pools on secondary threads](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/MemoryMgmt/Articles/mmAutoreleasePools.html)
- [Capture frame drops and retained sample buffers](https://developer.apple.com/library/archive/technotes/tn2445/_index.html)
- [ScreenCaptureKit synchronization clock](https://developer.apple.com/documentation/screencapturekit/scstream/synchronizationclock)
- [AVCaptureSession synchronization clock](https://developer.apple.com/documentation/avfoundation/avcapturesession/synchronizationclock)
