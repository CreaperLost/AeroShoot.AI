// AeroShootCapture.swift
//
// macOS native capture bridge for AeroShoot.AI. Exposes two parallel C ABIs:
//
//   1. The legacy "aeroshoot_macos_*" surface, preserved unchanged for the existing
//      Rust bindings in src-tauri/src/capture/macos.rs. The legacy path writes a
//      single long .mp4.tmp / .wav.tmp file per track and is documented below.
//
//   2. The new "aeroshoot_*" surface that implements the contract shared with the
//      Rust and Frontend agents:
//        - rotating, atomically committed segment files (P1, see Task 2)
//        - typed MediaAppendOutcome and AeroShootEncoderResult outcomes (P1, see Task 3)
//        - ScreenCaptureKit / AVFoundation clock correlation with per-track anchors
//          surfaced via the segment callback (P1, see Task 4)
//        - asynchronous permission request with typed terminal states (P1/P2, see Task 5)
//        - SCStream / AVCaptureSession runtime error forwarding (P2, see Task 6)
//        - bounded os_unfair_lock-protected timestamp journal with batched flushes,
//          coalesced cursor records, and an explicit `gaps_total` counter (P2, see Task 8)
//        - PID-based self-exclusion when enumerating capture applications (P2, see Task 9)
//        - macOS 14+ device type availability while keeping macOS 13 compatibility
//          (P3, see Task 10)
//
// All exported symbols use @_cdecl so the file is self-contained under the existing
// swiftc build pipeline; no module.modulemap is required.

import AVFoundation
import CoreGraphics
import CoreMedia
import CoreVideo
import Darwin
import Foundation
import ScreenCaptureKit
import VideoToolbox
import os

// MARK: - C ABI types (shared with Rust)
//
// These types match the header documented in the parent task brief. The struct
// memory layout is what matters; a Swift tuple of 256 Int8s is ABI-compatible
// with `char buffer[256]` on Apple Silicon and Intel macOS calling conventions.

// 256-byte fixed-size C string buffer. Used inside by-value structs. The C
// declaration is `char field_name[256]`. Swift represents such a field as a
// 256-element Int8 tuple, and tuples of a single primitive type are laid out
// contiguously in memory, so the @_cdecl call ABI matches.
public typealias CCharBuffer256 = (
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
  Int8, Int8, Int8, Int8, Int8, Int8, Int8, Int8,
)

// Result returned from encoder / finalization operations.
//
// Swift's @_cdecl cannot return Swift struct types. The C ABI for this struct
// (a 264-byte homogeneous layout) is identical to a Swift TUPLE of the same
// primitive fields, so the @_cdecl surface uses the CEncoderResultTuple
// typealias below. The Swift struct AeroShootEncoderResult is used internally
// for readability and converted to the tuple at the FFI boundary.
public struct AeroShootEncoderResult {
  public var status: Int32        // AeroShootEncoderStatus
  public var error_code: Int32    // 0 if no error; AVError / SCStreamError code otherwise
  public var error_message: CCharBuffer256  // NUL-terminated; empty when no error
  public init(status: Int32, error_code: Int32, error_message: CCharBuffer256) {
    self.status = status
    self.error_code = error_code
    self.error_message = error_message
  }
}

// Permission state for a single capability. Same story: the Swift struct is
// internal-only, the C ABI uses the CPermissionBundleTuple typealias.
public struct AeroShootPermissionBundle {
  public var screen_recording: Int32  // AeroShootPermissionState
  public var camera: Int32
  public var microphone: Int32
  public init(screen_recording: Int32, camera: Int32, microphone: Int32) {
    self.screen_recording = screen_recording
    self.camera = camera
    self.microphone = microphone
  }
}

// Tuple layouts that match the C structs byte-for-byte. NOT used at the
// @_cdecl boundary (Swift cannot return tuples or structs from @_cdecl), but
// kept for documentation and for any future code that wants to reason about
// the C ABI layout in Swift terms.
@available(*, unavailable, message: "C structs are passed via UnsafeMutableRawPointer at the @_cdecl boundary")
public typealias CEncoderResultTuple = (Int32, Int32, CCharBuffer256)
@available(*, unavailable, message: "C structs are passed via UnsafeMutableRawPointer at the @_cdecl boundary")
public typealias CPermissionBundleTuple = (Int32, Int32, Int32)

// Synchronously submit a finalized temporary segment to Rust persistence.
// Strings are borrowed during the call; Swift frees them. Zero means committed.
public typealias AeroShootSegmentCallback = @convention(c) (
  UnsafePointer<CChar>?,    // track_id ("screen" | "camera" | "system_audio" | "mic")
  Int64,                    // host_anchor_us
  Int32,                    // segment_index (0-based, monotonic per track per session)
  Int32,                    // timescale (e.g. 90000 for H.264 CMTime, 48000 for PCM)
  Int64,                    // media_start_value (media PTS of this segment's first sample)
  UnsafePointer<CChar>?     // file_path (absolute, finalized temporary segment)
) -> Int32

// Asynchronous runtime errors from ScreenCaptureKit / AVFoundation. track_id may
// be an empty string for session-level errors. The C strings are heap-allocated;
// release with aeroshoot_macos_free_string after the callback returns.
public typealias AeroShootRuntimeErrorCallback = @convention(c) (
  UnsafePointer<CChar>?,    // track_id
  Int32,                    // error_code
  UnsafePointer<CChar>?     // message
) -> Void

// Final permission snapshot returned from the async request. The C ABI is
// `void aeroshoot_request_permissions(AeroShootPermissionCompletion completion)`
// where the completion receives the bundle as three int32_t arguments
// (matching the ABI of a 3-Int32 homogeneous struct passed by value on arm64).
// We use the (Int32, Int32, Int32) tuple shape here because @convention(c)
// callbacks cannot accept Swift struct parameters.
public typealias AeroShootPermissionCompletion = @convention(c) (Int32, Int32, Int32) -> Void

// MARK: - C ABI enum values (raw values match the C header)

@inline(__always) private func AEROSHOOT_ENCODER_OK() -> Int32 { 0 }
@inline(__always) private func AEROSHOOT_ENCODER_BACKPRESSURED() -> Int32 { 1 }
@inline(__always) private func AEROSHOOT_ENCODER_FAILED() -> Int32 { 2 }
@inline(__always) private func AEROSHOOT_ENCODER_TIMEOUT() -> Int32 { 3 }

@inline(__always) private func AEROSHOOT_PERMISSION_UNKNOWN() -> Int32 { 0 }
@inline(__always) private func AEROSHOOT_PERMISSION_NOT_DETERMINED() -> Int32 { 1 }
@inline(__always) private func AEROSHOOT_PERMISSION_AUTHORIZED() -> Int32 { 2 }
@inline(__always) private func AEROSHOOT_PERMISSION_DENIED() -> Int32 { 3 }
@inline(__always) private func AEROSHOOT_PERMISSION_RESTRICTED() -> Int32 { 4 }

// MARK: - C ABI helpers

// Build a 256-Int8 tuple from a Swift String. Truncates to 255 bytes plus a
// trailing NUL. The returned tuple has the same memory layout as a C
// `char buf[256]` field, so it can be embedded inside by-value structs.
private func makeCharBuffer256(_ string: String) -> CCharBuffer256 {
  var t: CCharBuffer256 = (
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0
  )
  let bytes = Array(string.utf8.prefix(255))
  withUnsafeMutableBytes(of: &t) { raw in
    guard let base = raw.baseAddress else { return }
    let p = base.assumingMemoryBound(to: Int8.self)
    for (i, b) in bytes.enumerated() {
      p[i] = Int8(bitPattern: b)
    }
    // strdup-style NUL terminator: the array is zero-initialized so position 255
    // is already 0 unless the string was exactly 255 bytes long, in which case
    // the loop above wrote the 255th byte and we need to keep the terminator.
    if bytes.count >= 255 {
      p[255] = 0
    }
  }
  return t
}

private func copiedCString(_ value: String) -> UnsafeMutablePointer<CChar>? {
  strdup(value)
}

private func jsonString<T: Encodable>(_ value: T) -> String {
  guard let data = try? JSONEncoder().encode(value) else { return "{}" }
  return String(data: data, encoding: .utf8) ?? "{}"
}

private func encoderResultOK() -> AeroShootEncoderResult {
  return AeroShootEncoderResult(
    status: AEROSHOOT_ENCODER_OK(),
    error_code: 0,
    error_message: makeCharBuffer256("")
  )
}

private func encoderResultFailed(code: Int32, message: String) -> AeroShootEncoderResult {
  return AeroShootEncoderResult(
    status: AEROSHOOT_ENCODER_FAILED(),
    error_code: code,
    error_message: makeCharBuffer256(message)
  )
}

private func encoderResultTimeout(trackId: String) -> AeroShootEncoderResult {
  return AeroShootEncoderResult(
    status: AEROSHOOT_ENCODER_TIMEOUT(),
    error_code: 0,
    error_message: makeCharBuffer256("track=\(trackId)")
  )
}

private func encoderResultBackpressured(trackId: String) -> AeroShootEncoderResult {
  return AeroShootEncoderResult(
    status: AEROSHOOT_ENCODER_BACKPRESSURED(),
    error_code: 0,
    error_message: makeCharBuffer256("track=\(trackId)")
  )
}

private func permissionState(for mediaType: AVMediaType) -> Int32 {
  switch AVCaptureDevice.authorizationStatus(for: mediaType) {
  case .notDetermined: return AEROSHOOT_PERMISSION_NOT_DETERMINED()
  case .authorized:    return AEROSHOOT_PERMISSION_AUTHORIZED()
  case .denied:        return AEROSHOOT_PERMISSION_DENIED()
  case .restricted:    return AEROSHOOT_PERMISSION_RESTRICTED()
  @unknown default:    return AEROSHOOT_PERMISSION_UNKNOWN()
  }
}

// ScreenCaptureKit's shareable-content query presents a system dialog on
// packaged macOS Sequoia/Tahoe apps — even after TCC is granted. Never use it
// to *check* or *refresh* permission. Check with preflight / window titles;
// request with CGRequestScreenCaptureAccess, which is the one-shot TCC prompt.
private let g_screenAccessLock = NSLock()
private var g_screenAuthorizedThisProcess = false

private final class ShareableContentProbe {
  private let lock = NSLock()
  private var boxedContent: SCShareableContent?
  private var boxedError: Error?

  func set(_ content: SCShareableContent?, _ error: Error?) {
    lock.lock()
    boxedContent = content
    boxedError = error
    lock.unlock()
  }

  var content: SCShareableContent? {
    lock.lock(); defer { lock.unlock() }
    return boxedContent
  }

  var error: Error? {
    lock.lock(); defer { lock.unlock() }
    return boxedError
  }

  var finished: Bool {
    lock.lock(); defer { lock.unlock() }
    return boxedContent != nil || boxedError != nil
  }
}

/// Other apps' window titles are only populated when Screen Recording TCC is granted.
private func windowListIndicatesScreenAccess() -> Bool {
  guard let info = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as NSArray? else {
    return false
  }
  let myPID = Int32(ProcessInfo.processInfo.processIdentifier)
  for case let window as NSDictionary in info {
    let pid = (window[kCGWindowOwnerPID] as? NSNumber)?.int32Value ?? 0
    if pid == 0 || pid == myPID { continue }
    if let name = window[kCGWindowName] as? String, !name.isEmpty {
      return true
    }
  }
  return false
}

private func markScreenAuthorizedThisProcess() {
  g_screenAccessLock.lock()
  g_screenAuthorizedThisProcess = true
  g_screenAccessLock.unlock()
}

private func screenAuthorizedThisProcess() -> Bool {
  g_screenAccessLock.lock()
  defer { g_screenAccessLock.unlock() }
  return g_screenAuthorizedThisProcess
}

/// Non-prompting Screen Recording check. Must never call ScreenCaptureKit.
private func peekScreenRecordingState() -> Int32 {
  if screenAuthorizedThisProcess() || CGPreflightScreenCaptureAccess() || windowListIndicatesScreenAccess() {
    markScreenAuthorizedThisProcess()
    return AEROSHOOT_PERMISSION_AUTHORIZED()
  }
  return AEROSHOOT_PERMISSION_NOT_DETERMINED()
}

/// One-shot TCC request. Does not enumerate shareable content (that re-prompts).
private func requestScreenRecordingAccess() -> Int32 {
  let peeked = peekScreenRecordingState()
  if peeked == AEROSHOOT_PERMISSION_AUTHORIZED() {
    return peeked
  }
  var granted = false
  if Thread.isMainThread {
    granted = CGRequestScreenCaptureAccess()
  } else {
    let sem = DispatchSemaphore(value: 0)
    DispatchQueue.main.async {
      granted = CGRequestScreenCaptureAccess()
      sem.signal()
    }
    _ = sem.wait(timeout: .now() + 120)
  }
  if granted {
    markScreenAuthorizedThisProcess()
    return AEROSHOOT_PERMISSION_AUTHORIZED()
  }
  return peekScreenRecordingState()
}

private func waitForShareableContent(timeout: TimeInterval) -> (SCShareableContent?, Error?) {
  let probe = ShareableContentProbe()
  let sem = DispatchSemaphore(value: 0)
  let startQuery = {
    SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, error in
      probe.set(content, error)
      sem.signal()
    }
  }

  if Thread.isMainThread {
    startQuery()
    let deadline = Date().addingTimeInterval(timeout)
    while !probe.finished && Date() < deadline {
      RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.05))
    }
  } else {
    DispatchQueue.main.async { startQuery() }
    _ = sem.wait(timeout: .now() + timeout)
  }
  return (probe.content, probe.error)
}

private func screenRecordingState() -> Int32 {
  peekScreenRecordingState()
}

private func writePermissionBundle(_ bundle: AeroShootPermissionBundle, to outBundle: UnsafeMutableRawPointer?) {
  guard let raw = outBundle else { return }
  // Store three packed Int32s. Do not assign a Swift struct through
  // assumingMemoryBound — Swift struct ABI is not the C layout Rust expects.
  raw.storeBytes(of: bundle.screen_recording, as: Int32.self)
  raw.advanced(by: MemoryLayout<Int32>.size).storeBytes(of: bundle.camera, as: Int32.self)
  raw.advanced(by: MemoryLayout<Int32>.size * 2).storeBytes(of: bundle.microphone, as: Int32.self)
}

private func currentPermissionBundle() -> AeroShootPermissionBundle {
  return AeroShootPermissionBundle(
    screen_recording: screenRecordingState(),
    camera: permissionState(for: .video),
    microphone: permissionState(for: .audio)
  )
}

private func fsyncFile(at path: String) -> Bool {
  let fd = open(path, O_RDONLY)
  if fd >= 0 {
    let result = fsync(fd)
    close(fd)
    return result == 0
  }
  return false
}

private func fsyncParentDirectory(of path: String) -> Bool {
  let parent = (path as NSString).deletingLastPathComponent
  let fd = open(parent, O_RDONLY)
  if fd >= 0 {
    let result = fsync(fd)
    close(fd)
    return result == 0
  }
  return false
}

private func atomicRename(from src: String, to dst: String) -> Bool {
  // A commit must never replace an earlier segment. `rename(2)` replaces its
  // destination, so reject an existing path before performing the atomic move.
  // The check and rename are serialized by the per-track writer lock; a
  // destination race still fails safely rather than overwriting source media.
  if FileManager.default.fileExists(atPath: dst) {
    return false
  }
  return rename(src, dst) == 0
}

// MARK: - Configuration DTOs (unchanged from the legacy path)

private struct NativeConfig: Decodable {
  let sourceId: String
  let cameraId: String?
  let micId: String?
  let captureSystemAudio: Bool
  let fps: Int
  let width: Int
  let height: Int
  let sourceRect: NativeRect?
  let destinationRect: NativeRect?
  let preservesAspectRatio: Bool?
  let projectPath: String
  let sessionOffsetUs: UInt64
}

private struct NativeRect: Decodable {
  let x: Int
  let y: Int
  let width: UInt32
  let height: UInt32

  var cgRect: CGRect { CGRect(x: x, y: y, width: Int(width), height: Int(height)) }
}

private struct SourceDTO: Encodable {
  let id: String
  let name: String
  let sourceType: String
  let width: UInt32
  let height: UInt32
}

private struct DeviceDTO: Encodable {
  let id: String
  let name: String
  let isDefault: Bool
}

private struct DevicesDTO: Encodable { let cameras: [DeviceDTO]; let mics: [DeviceDTO] }
private struct PermissionsDTO: Encodable { let screenRecording: Bool; let camera: Bool; let microphone: Bool }
private struct StatsDTO: Encodable {
  let droppedFrames: UInt64
  let audioBufferUnderflows: UInt64
  let timestampRecordsDropped: UInt64
  let gapsTotal: UInt64
  let lastError: String?
}

// MARK: - Legacy C exports (preserved verbatim for back-compat with src-tauri/src/capture/macos.rs)

@_cdecl("aeroshoot_macos_free_string")
public func aeroshootMacOSFreeString(_ pointer: UnsafeMutablePointer<CChar>?) { free(pointer) }

private func coreGraphicsDisplaySources() -> [SourceDTO] {
  var result: [SourceDTO] = []
  var maxDisplays: UInt32 = 16
  var activeDisplays = [CGDirectDisplayID](repeating: 0, count: Int(maxDisplays))
  var displayCount: UInt32 = 0
  if CGGetActiveDisplayList(maxDisplays, &activeDisplays, &displayCount) == .success {
    for i in 0..<Int(displayCount) {
      let dId = activeDisplays[i]
      let width = CGDisplayPixelsWide(dId)
      let height = CGDisplayPixelsHigh(dId)
      result.append(SourceDTO(
        id: "display:\(dId)",
        name: "Display \(dId)",
        sourceType: "display",
        width: UInt32(max(0, width)),
        height: UInt32(max(0, height))
      ))
    }
  }
  return result
}

private func sources(from content: SCShareableContent) -> [SourceDTO] {
  var result: [SourceDTO] = []
  result.append(contentsOf: content.displays.map {
    SourceDTO(id: "display:\($0.displayID)", name: "Display \($0.displayID)", sourceType: "display", width: UInt32($0.width), height: UInt32($0.height))
  })
  let myPID = ProcessInfo.processInfo.processIdentifier
  let myBID = Bundle.main.bundleIdentifier ?? ""
  result.append(contentsOf: content.windows.compactMap {
    if let app = $0.owningApplication, app.processID == myPID || (!myBID.isEmpty && app.bundleIdentifier == myBID) {
      return nil
    }
    let app = $0.owningApplication?.applicationName ?? "Application"
    let title = $0.title?.isEmpty == false ? $0.title! : "Untitled Window"
    return SourceDTO(id: "window:\($0.windowID)", name: "\(app) — \(title)", sourceType: "window", width: UInt32(max(0, Int($0.frame.width))), height: UInt32(max(0, Int($0.frame.height))))
  })
  result.append(contentsOf: content.applications.compactMap {
    if $0.processID == myPID || (!myBID.isEmpty && $0.bundleIdentifier == myBID) {
      return nil
    }
    let display = content.displays.first
    return SourceDTO(id: "application:\($0.bundleIdentifier)", name: $0.applicationName, sourceType: "application", width: UInt32(display?.width ?? 0), height: UInt32(display?.height ?? 0))
  })
  return result
}

@_cdecl("aeroshoot_macos_copy_sources_json")
public func aeroshootMacOSCopySourcesJSON() -> UnsafeMutablePointer<CChar>? {
  // Only enumerate via ScreenCaptureKit when preflight is already true.
  // A speculative SCK query re-presents the system permission dialog on
  // packaged Sequoia/Tahoe builds even after the user has granted access.
  if CGPreflightScreenCaptureAccess() {
    let (content, _) = waitForShareableContent(timeout: 8)
    if let content {
      return copiedCString(jsonString(sources(from: content)))
    }
  }
  return copiedCString(jsonString(coreGraphicsDisplaySources()))
}

private func devices(for mediaType: AVMediaType) -> [AVCaptureDevice] {
  // macOS 13 path uses builtInWideAngleCamera + externalUnknown. macOS 14+ types
  // (.external, .continuityCamera) are added behind availability checks so the
  // deployment target remains 13.0. NSCameraUseContinuityCameraDeviceType is
  // intentionally NOT added to Info.plist yet — see // TODO: opt-in flag below.
  // Note: Desk View does not have a public macOS AVCaptureDevice.DeviceType
  // case; it is reported through the same continuityCamera device on macOS.
  var types: [AVCaptureDevice.DeviceType] = []
  if mediaType == .video {
    types.append(.builtInWideAngleCamera)
    types.append(.externalUnknown)
    types.append(.deskViewCamera)
    if #available(macOS 14.0, *) {
      types.append(.external)
      types.append(.continuityCamera)
    }
    // TODO: opt-in flag — set NSCameraUseContinuityCameraDeviceType in Info.plist
    // only if the product wants to surface the dedicated Continuity Camera device
    // type to the system. We do not opt in by default.
  } else {
    types.append(.builtInMicrophone)
    types.append(.externalUnknown)
  }
  return AVCaptureDevice.DiscoverySession(deviceTypes: types, mediaType: mediaType, position: .unspecified).devices
}

@_cdecl("aeroshoot_macos_copy_devices_json")
public func aeroshootMacOSCopyDevicesJSON() -> UnsafeMutablePointer<CChar>? {
  let defaultCamera = AVCaptureDevice.default(for: .video)?.uniqueID
  let defaultMic = AVCaptureDevice.default(for: .audio)?.uniqueID
  let cameras = devices(for: .video).map { DeviceDTO(id: $0.uniqueID, name: $0.localizedName, isDefault: $0.uniqueID == defaultCamera) }
  let mics = devices(for: .audio).map { DeviceDTO(id: $0.uniqueID, name: $0.localizedName, isDefault: $0.uniqueID == defaultMic) }
  return copiedCString(jsonString(DevicesDTO(cameras: cameras, mics: mics)))
}

private func authorized(_ mediaType: AVMediaType) -> Bool {
  AVCaptureDevice.authorizationStatus(for: mediaType) == .authorized
}

@_cdecl("aeroshoot_macos_copy_permissions_json")
public func aeroshootMacOSCopyPermissionsJSON() -> UnsafeMutablePointer<CChar>? {
  copiedCString(jsonString(PermissionsDTO(screenRecording: peekScreenRecordingState() == AEROSHOOT_PERMISSION_AUTHORIZED(), camera: authorized(.video), microphone: authorized(.audio))))
}

@_cdecl("aeroshoot_macos_request_permissions")
public func aeroshootMacOSRequestPermissions(_ screen: Bool, _ camera: Bool, _ microphone: Bool) {
  if screen { _ = CGRequestScreenCaptureAccess() }
  if camera && AVCaptureDevice.authorizationStatus(for: .video) == .notDetermined { AVCaptureDevice.requestAccess(for: .video) { _ in } }
  if microphone && AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined { AVCaptureDevice.requestAccess(for: .audio) { _ in } }
}

// MARK: - Legacy TimestampLog (used only by the legacy Recorder)

private final class TimestampLog {
  private let queue = DispatchQueue(label: "ai.aeroshoot.timestamps")
  private var handle: FileHandle?
  private var anchors: [String: (CMTime, UInt64)] = [:]
  private let started = DispatchTime.now().uptimeNanoseconds
  private let sessionOffsetUs: UInt64

  init(path: String, sessionOffsetUs: UInt64) {
    self.sessionOffsetUs = sessionOffsetUs
    FileManager.default.createFile(atPath: path, contents: nil)
    handle = FileHandle(forWritingAtPath: path)
  }

  func append(track: String, sample: CMSampleBuffer) {
    let pts = CMSampleBufferGetPresentationTimeStamp(sample)
    guard pts.isValid && pts.timescale != 0 else { return }
    queue.async { [weak self] in
      guard let self else { return }
      let elapsed = self.sessionOffsetUs + (DispatchTime.now().uptimeNanoseconds - self.started) / 1_000
      let anchor = self.anchors[track] ?? (pts, elapsed)
      self.anchors[track] = anchor
      let delta = CMTimeSubtract(pts, anchor.0)
      let deltaUs = delta.isValid ? max(0, Int64(CMTimeGetSeconds(delta) * 1_000_000.0)) : 0
      let mapped = anchor.1 + UInt64(deltaUs)
      let line = "{\"track\":\"\(track)\",\"nativeValue\":\(pts.value),\"nativeTimescale\":\(pts.timescale),\"mappedUs\":\(mapped)}\n"
      if let data = line.data(using: .utf8) { try? self.handle?.write(contentsOf: data) }
    }
  }

  func close() {
    queue.sync { try? handle?.synchronize(); try? handle?.close(); handle = nil }
  }
}

// MARK: - Legacy MediaWriter (used only by the legacy Recorder)

private final class MediaWriter {
  let writer: AVAssetWriter
  let input: AVAssetWriterInput
  private var started = false
  private let lock = NSLock()

  init(videoPath: String, width: Int, height: Int, fps: Int) throws {
    try? FileManager.default.removeItem(atPath: videoPath)
    writer = try AVAssetWriter(outputURL: URL(fileURLWithPath: videoPath), fileType: .mp4)
    writer.movieFragmentInterval = CMTime(seconds: 2, preferredTimescale: 600)
    writer.shouldOptimizeForNetworkUse = true
    let properties: [String: Any] = [
      AVVideoAverageBitRateKey: max(4_000_000, width * height * max(fps, 1) / 5),
      AVVideoMaxKeyFrameIntervalDurationKey: 2,
      AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
      AVVideoAllowFrameReorderingKey: true
    ]
    let settings: [String: Any] = [
      AVVideoCodecKey: AVVideoCodecType.h264,
      AVVideoWidthKey: width,
      AVVideoHeightKey: height,
      AVVideoCompressionPropertiesKey: properties,
      AVVideoEncoderSpecificationKey: [kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder as String: true]
    ]
    input = AVAssetWriterInput(mediaType: .video, outputSettings: settings)
    input.expectsMediaDataInRealTime = true
    guard writer.canAdd(input) else { throw NSError(domain: "AeroShoot", code: 20, userInfo: [NSLocalizedDescriptionKey: "H.264 VideoToolbox writer input is unavailable"]) }
    writer.add(input)
  }

  init(audioPath: String, channels: Int) throws {
    try? FileManager.default.removeItem(atPath: audioPath)
    writer = try AVAssetWriter(outputURL: URL(fileURLWithPath: audioPath), fileType: .wav)
    let settings: [String: Any] = [
      AVFormatIDKey: kAudioFormatLinearPCM,
      AVSampleRateKey: 48_000,
      AVNumberOfChannelsKey: channels,
      AVLinearPCMBitDepthKey: 16,
      AVLinearPCMIsFloatKey: false,
      AVLinearPCMIsBigEndianKey: false,
      AVLinearPCMIsNonInterleaved: false
    ]
    input = AVAssetWriterInput(mediaType: .audio, outputSettings: settings)
    input.expectsMediaDataInRealTime = true
    guard writer.canAdd(input) else { throw NSError(domain: "AeroShoot", code: 21, userInfo: [NSLocalizedDescriptionKey: "PCM audio writer input is unavailable"]) }
    writer.add(input)
  }

  func append(_ sample: CMSampleBuffer) -> Bool {
    lock.lock(); defer { lock.unlock() }
    if !started {
      guard writer.startWriting() else { return false }
      writer.startSession(atSourceTime: CMSampleBufferGetPresentationTimeStamp(sample))
      started = true
    }
    return input.isReadyForMoreMediaData && input.append(sample)
  }

  func finish() {
    lock.lock()
    guard started else { lock.unlock(); return }
    input.markAsFinished()
    lock.unlock()
    let semaphore = DispatchSemaphore(value: 0)
    writer.finishWriting { semaphore.signal() }
    _ = semaphore.wait(timeout: .now() + 15)
  }
}

private func requireHardwareH264(width: Int, height: Int) throws {
  var session: VTCompressionSession?
  let specification = [kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder as String: true] as CFDictionary
  let status = VTCompressionSessionCreate(
    allocator: kCFAllocatorDefault,
    width: Int32(width),
    height: Int32(height),
    codecType: kCMVideoCodecType_H264,
    encoderSpecification: specification,
    imageBufferAttributes: nil,
    compressedDataAllocator: nil,
    outputCallback: nil,
    refcon: nil,
    compressionSessionOut: &session
  )
  guard status == noErr, let session else {
    throw NSError(domain: "AeroShoot", code: Int(status), userInfo: [NSLocalizedDescriptionKey: "A hardware VideoToolbox H.264 encoder is unavailable for \(width)x\(height)"])
  }
  VTCompressionSessionInvalidate(session)
}

private final class Recorder: NSObject, SCStreamDelegate, SCStreamOutput, AVCaptureVideoDataOutputSampleBufferDelegate, AVCaptureAudioDataOutputSampleBufferDelegate {
  private let config: NativeConfig
  private let screenQueue = DispatchQueue(label: "ai.aeroshoot.screen", qos: .userInteractive)
  private let systemAudioQueue = DispatchQueue(label: "ai.aeroshoot.system-audio", qos: .userInitiated)
  private let cameraQueue = DispatchQueue(label: "ai.aeroshoot.camera", qos: .userInteractive)
  private let micQueue = DispatchQueue(label: "ai.aeroshoot.mic", qos: .userInitiated)
  private var stream: SCStream?
  private var cameraSession: AVCaptureSession?
  private var screenWriter: MediaWriter?
  private var systemWriter: MediaWriter?
  private var cameraWriter: MediaWriter?
  private var micWriter: MediaWriter?
  private var timestampLog: TimestampLog?
  private let stateLock = NSLock()
  private var paused = false
  private var droppedFrames: UInt64 = 0
  private var audioUnderflows: UInt64 = 0
  private var lastError: String?

  init(config: NativeConfig) { self.config = config }

  func start() throws {
    let fm = FileManager.default
    for folder in ["media/screen", "media/webcam", "media/system", "media/mic", "telemetry"] {
      try fm.createDirectory(atPath: (config.projectPath as NSString).appendingPathComponent(folder), withIntermediateDirectories: true)
    }
    try requireHardwareH264(width: config.width, height: config.height)
    if config.cameraId != nil { try requireHardwareH264(width: 1280, height: 720) }
    timestampLog = TimestampLog(path: (config.projectPath as NSString).appendingPathComponent("telemetry/media_timestamps.jsonl"), sessionOffsetUs: config.sessionOffsetUs)
    screenWriter = try MediaWriter(videoPath: (config.projectPath as NSString).appendingPathComponent("media/screen/000001.mp4.tmp"), width: config.width, height: config.height, fps: config.fps)
    if config.captureSystemAudio { systemWriter = try MediaWriter(audioPath: (config.projectPath as NSString).appendingPathComponent("media/system/000001.wav.tmp"), channels: 2) }
    if config.cameraId != nil { cameraWriter = try MediaWriter(videoPath: (config.projectPath as NSString).appendingPathComponent("media/webcam/000001.mp4.tmp"), width: 1280, height: 720, fps: config.fps) }
    if config.micId != nil { micWriter = try MediaWriter(audioPath: (config.projectPath as NSString).appendingPathComponent("media/mic/000001.wav.tmp"), channels: 1) }
    try startScreen()
    if config.cameraId != nil || config.micId != nil { try startAVCapture() }
  }

  private func shareableContent() throws -> SCShareableContent {
    let semaphore = DispatchSemaphore(value: 0)
    var result: Result<SCShareableContent, Error>?
    SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, error in
      if let content { result = .success(content) }
      else { result = .failure(error ?? NSError(domain: "AeroShoot", code: 1, userInfo: [NSLocalizedDescriptionKey: "ScreenCaptureKit returned no content"])) }
      semaphore.signal()
    }
    guard semaphore.wait(timeout: .now() + 10) == .success else { throw NSError(domain: "AeroShoot", code: 2, userInfo: [NSLocalizedDescriptionKey: "Timed out enumerating shareable content"]) }
    return try result!.get()
  }

  private func startScreen() throws {
    let content = try shareableContent()
    let parts = config.sourceId.split(separator: ":", maxSplits: 1).map(String.init)
    guard parts.count == 2 else { throw NSError(domain: "AeroShoot", code: 3, userInfo: [NSLocalizedDescriptionKey: "Invalid capture source ID"]) }
    // Task 9 (legacy path): use the same PID-first exclusion that the new path
    // applies. A process without a bundle identifier is NOT matched by ID.
    let myPID = ProcessInfo.processInfo.processIdentifier
    let ownApp = content.applications.first { app in
      if app.processID == myPID { return true }
      let myBID = Bundle.main.bundleIdentifier ?? ""
      return !myBID.isEmpty && app.bundleIdentifier == myBID
    }
    let filter: SCContentFilter
    switch parts[0] {
    case "display":
      guard let id = UInt32(parts[1]), let display = content.displays.first(where: { $0.displayID == id }) else { throw NSError(domain: "AeroShoot", code: 4, userInfo: [NSLocalizedDescriptionKey: "Display is no longer available"]) }
      filter = SCContentFilter(display: display, excludingApplications: ownApp.map { [$0] } ?? [], exceptingWindows: [])
    case "window":
      guard let id = UInt32(parts[1]), let window = content.windows.first(where: { $0.windowID == id }) else { throw NSError(domain: "AeroShoot", code: 5, userInfo: [NSLocalizedDescriptionKey: "Window is no longer available"]) }
      filter = SCContentFilter(desktopIndependentWindow: window)
    case "application":
      guard let app = content.applications.first(where: { $0.bundleIdentifier == parts[1] }), let display = content.displays.first else { throw NSError(domain: "AeroShoot", code: 6, userInfo: [NSLocalizedDescriptionKey: "Application is no longer available"]) }
      filter = SCContentFilter(display: display, including: [app], exceptingWindows: [])
    default: throw NSError(domain: "AeroShoot", code: 7, userInfo: [NSLocalizedDescriptionKey: "Unsupported capture source type"])
    }
    let streamConfig = SCStreamConfiguration()
    streamConfig.width = config.width
    streamConfig.height = config.height
    streamConfig.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(max(config.fps, 1)))
    streamConfig.queueDepth = 6
    streamConfig.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
    streamConfig.showsCursor = true
    streamConfig.capturesAudio = config.captureSystemAudio
    streamConfig.sampleRate = 48_000
    streamConfig.channelCount = 2
    streamConfig.excludesCurrentProcessAudio = true
    let stream = SCStream(filter: filter, configuration: streamConfig, delegate: self)
    try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: screenQueue)
    if config.captureSystemAudio { try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: systemAudioQueue) }
    let semaphore = DispatchSemaphore(value: 0)
    var startError: Error?
    stream.startCapture { error in startError = error; semaphore.signal() }
    _ = semaphore.wait(timeout: .now() + 10)
    if let startError { throw startError }
    self.stream = stream
  }

  private func startAVCapture() throws {
    let session = AVCaptureSession()
    session.beginConfiguration()
    if let id = config.cameraId {
      guard let device = AVCaptureDevice(uniqueID: id) else { throw NSError(domain: "AeroShoot", code: 8, userInfo: [NSLocalizedDescriptionKey: "Camera is no longer available"]) }
      let input = try AVCaptureDeviceInput(device: device)
      guard session.canAddInput(input) else { throw NSError(domain: "AeroShoot", code: 9, userInfo: [NSLocalizedDescriptionKey: "Camera input cannot be added"]) }
      session.addInput(input)
      let output = AVCaptureVideoDataOutput()
      output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange]
      output.alwaysDiscardsLateVideoFrames = true
      output.setSampleBufferDelegate(self, queue: cameraQueue)
      guard session.canAddOutput(output) else { throw NSError(domain: "AeroShoot", code: 10, userInfo: [NSLocalizedDescriptionKey: "Camera output cannot be added"]) }
      session.addOutput(output)
    }
    if let id = config.micId {
      guard let device = AVCaptureDevice(uniqueID: id) else { throw NSError(domain: "AeroShoot", code: 11, userInfo: [NSLocalizedDescriptionKey: "Microphone is no longer available"]) }
      let input = try AVCaptureDeviceInput(device: device)
      guard session.canAddInput(input) else { throw NSError(domain: "AeroShoot", code: 12, userInfo: [NSLocalizedDescriptionKey: "Microphone input cannot be added"]) }
      session.addInput(input)
      let output = AVCaptureAudioDataOutput()
      output.setSampleBufferDelegate(self, queue: micQueue)
      guard session.canAddOutput(output) else { throw NSError(domain: "AeroShoot", code: 13, userInfo: [NSLocalizedDescriptionKey: "Microphone output cannot be added"]) }
      session.addOutput(output)
    }
    session.commitConfiguration()
    session.startRunning()
    cameraSession = session
  }

  func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of outputType: SCStreamOutputType) {
    stateLock.lock(); let shouldAppend = !paused; stateLock.unlock()
    guard shouldAppend, sampleBuffer.isValid else { return }
    if outputType == .screen {
      guard
        let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
        let statusValue = attachments.first?[.status] as? Int,
        SCFrameStatus(rawValue: statusValue) == .complete,
        CMSampleBufferGetImageBuffer(sampleBuffer) != nil
      else { return }
      timestampLog?.append(track: "screen", sample: sampleBuffer)
      if screenWriter?.append(sampleBuffer) != true { stateLock.lock(); droppedFrames += 1; stateLock.unlock() }
    } else if outputType == .audio {
      timestampLog?.append(track: "system", sample: sampleBuffer)
      if systemWriter?.append(sampleBuffer) != true { stateLock.lock(); audioUnderflows += 1; stateLock.unlock() }
    }
  }

  func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
    stateLock.lock(); let shouldAppend = !paused; stateLock.unlock()
    guard shouldAppend else { return }
    if output is AVCaptureVideoDataOutput {
      timestampLog?.append(track: "webcam", sample: sampleBuffer)
      if cameraWriter?.append(sampleBuffer) != true { stateLock.lock(); droppedFrames += 1; stateLock.unlock() }
    } else {
      timestampLog?.append(track: "mic", sample: sampleBuffer)
      if micWriter?.append(sampleBuffer) != true { stateLock.lock(); audioUnderflows += 1; stateLock.unlock() }
    }
  }

  func stream(_ stream: SCStream, didStopWithError error: Error) { stateLock.lock(); lastError = error.localizedDescription; stateLock.unlock() }

  func setPaused(_ value: Bool) { stateLock.lock(); paused = value; stateLock.unlock() }

  func stop() {
    cameraSession?.stopRunning()
    if let stream {
      let semaphore = DispatchSemaphore(value: 0)
      stream.stopCapture { _ in semaphore.signal() }
      _ = semaphore.wait(timeout: .now() + 10)
    }
    screenQueue.sync {}; systemAudioQueue.sync {}; cameraQueue.sync {}; micQueue.sync {}
    screenWriter?.finish(); cameraWriter?.finish(); systemWriter?.finish(); micWriter?.finish()
    timestampLog?.close()
  }

  func stats() -> StatsDTO { stateLock.lock(); defer { stateLock.unlock() }; return StatsDTO(droppedFrames: droppedFrames, audioBufferUnderflows: audioUnderflows, timestampRecordsDropped: 0, gapsTotal: 0, lastError: lastError) }
}

private func decodeConfig(_ pointer: UnsafePointer<CChar>) throws -> NativeConfig {
  try JSONDecoder().decode(NativeConfig.self, from: Data(String(cString: pointer).utf8))
}
// =============================================================================
// NEW C ABI (aeroshoot_*)
// =============================================================================
//
// All globals below are protected by g_stateLock unless otherwise noted.

private let g_stateLock = NSLock()

// Callbacks registered by the Rust side. They are invoked from recorder
// background threads; the Rust side is responsible for any marshalling.
private var g_segmentCallback: AeroShootSegmentCallback?
private var g_runtimeErrorCallback: AeroShootRuntimeErrorCallback?

// The single active recorder, if any. Multiple concurrent recorders are not
// supported by the new C ABI; the Rust side ensures one-at-a-time usage.
private var g_activeRecorder: ActiveRecorder?
private var g_activeRecorderHandle: UnsafeMutableRawPointer?

private func invokeSegmentCallback(
  trackId: String,
  filePath: String,
  hostAnchorUs: Int64,
  segmentIndex: Int32,
  timescale: Int32,
  mediaStartValue: Int64
) -> Int32 {
  g_stateLock.lock()
  let cb = g_segmentCallback
  g_stateLock.unlock()
  guard let cb else { return -600 }
  guard let trackIdPtr = strdup(trackId) else { return -600 }
  guard let filePathPtr = strdup(filePath) else { free(trackIdPtr); return -600 }
  let result = cb(trackIdPtr, hostAnchorUs, segmentIndex, timescale, mediaStartValue, filePathPtr)
  free(trackIdPtr)
  free(filePathPtr)
  return result
}

private func invokeRuntimeErrorCallback(trackId: String, code: Int32, message: String) {
  g_stateLock.lock()
  let cb = g_runtimeErrorCallback
  g_stateLock.unlock()
  guard let cb else { return }
  guard let trackIdPtr = strdup(trackId.isEmpty ? "" : trackId) else { return }
  guard let messagePtr = strdup(message) else { free(trackIdPtr); return }
  cb(trackIdPtr, code, messagePtr)
  free(trackIdPtr)
  free(messagePtr)
}

// MARK: - MediaAppendOutcome (Task 3)

private enum MediaAppendOutcome {
  case accepted
  case backpressured
  case failed(code: Int32, reason: String)
}

// MARK: - NativeClockCorrelation (Task 4)
//
// CoreMedia timestamps are not callback timestamps.  At session setup we
// correlate each capture source's synchronization clock with the host clock
// and retain the rational source anchor.  Samples are then mapped from their
// original CMTime value/timescale; queue scheduling can therefore not change a
// track's offset.  CMClockGetAnchorTime is used when available and the
// CMSync conversion fallback is still performed at setup, never in a sample
// callback.

private final class NativeClockCorrelation {
  let sourceId: String
  let sourceClock: CMClock
  let hostClock: CMClock
  let nativeAnchorValue: Int64
  let nativeAnchorTimescale: Int32
  let hostAnchorValue: Int64
  let hostAnchorTimescale: Int32
  let hostAnchorUs: Int64
  private let lock = NSLock()
  private var lastHostUs: Int64

  init(sourceId: String, sourceClock: CMClock, sessionOffsetUs: UInt64, sessionHostEpoch: CMTime) {
    self.sourceId = sourceId
    self.sourceClock = sourceClock
    self.hostClock = CMClockGetHostTimeClock()

    var sourceAnchor = CMClockGetTime(sourceClock)
    // This API returns a native clock time correlated to its reference clock.
    // We retain the native value and use CMSyncConvertTime to express that
    // same instant in the host clock domain.  If a clock cannot expose an
    // anchor (some virtual devices do this), setup-time conversion is a stable
    // and deterministic fallback.
    var referenceAnchor = CMTime.invalid
    let anchorStatus = CMClockGetAnchorTime(
      sourceClock,
      clockTimeOut: &sourceAnchor,
      referenceClockTimeOut: &referenceAnchor
    )
    if anchorStatus != noErr || !sourceAnchor.isValid || sourceAnchor.timescale <= 0 {
      sourceAnchor = CMClockGetTime(sourceClock)
    }
    if !sourceAnchor.isValid || sourceAnchor.timescale <= 0 {
      sourceAnchor = CMTime(value: 0, timescale: 1)
    }
    self.nativeAnchorValue = sourceAnchor.value
    self.nativeAnchorTimescale = Int32(sourceAnchor.timescale)

    let convertedHost = CMSyncConvertTime(sourceAnchor, from: sourceClock, to: self.hostClock)
    let hostTime = convertedHost.isValid ? convertedHost : CMClockGetTime(self.hostClock)
    self.hostAnchorValue = hostTime.value
    self.hostAnchorTimescale = Int32(max(hostTime.timescale, 1))
    let hostDeltaUs = NativeClockCorrelation.microseconds(CMTimeSubtract(hostTime, sessionHostEpoch))
    self.hostAnchorUs = Int64(clamping: sessionOffsetUs) &+ hostDeltaUs
    self.lastHostUs = self.hostAnchorUs
  }

  private static func microseconds(_ time: CMTime) -> Int64 {
    guard time.isValid, time.timescale > 0 else { return 0 }
    let product = time.value.multipliedReportingOverflow(by: 1_000_000)
    if !product.overflow { return product.partialValue / Int64(time.timescale) }
    return Int64((Double(time.value) * 1_000_000.0) / Double(time.timescale))
  }

  // This initializer is intentionally private to tests in this file only via
  // setup; all production anchors come from CMClock correlation above.
  var metadataJSON: String {
    "{\"type\":\"clock_anchor\",\"source\":\"\(sourceId)\",\"native_value\":\(nativeAnchorValue),\"native_timescale\":\(nativeAnchorTimescale),\"host_value\":\(hostAnchorValue),\"host_timescale\":\(hostAnchorTimescale),\"host_anchor_us\":\(hostAnchorUs)}\n"
  }

  func map(_ pts: CMTime) -> (hostUs: Int64, isDiscontinuity: Bool) {
    lock.lock(); defer { lock.unlock() }
    guard pts.isValid, pts.timescale > 0 else { return (lastHostUs, true) }
    let delta = CMTimeSubtract(pts, CMTime(value: nativeAnchorValue, timescale: CMTimeScale(nativeAnchorTimescale)))
    let deltaUs: Int64
    if delta.isValid, delta.timescale > 0 {
      deltaUs = NativeClockCorrelation.microseconds(delta)
    } else {
      deltaUs = 0
    }
    let (candidate, overflow) = hostAnchorUs.addingReportingOverflow(deltaUs)
    let mapped = overflow ? (deltaUs >= 0 ? Int64.max : Int64.min) : candidate
    let backwards = mapped < lastHostUs
    let gap = !backwards && mapped - lastHostUs > 1_000_000
    let output = backwards ? lastHostUs : mapped
    lastHostUs = output
    return (output, backwards || gap)
  }
}

// MARK: - BoundedTimestampLog (Task 8)
//
// Replaces the legacy TimestampLog with:
//  - a fixed-capacity ring buffer (4096 entries) protected by os_unfair_lock
//  - a single serial flusher thread that batch-writes every 250ms or when 256
//    records accumulate
//  - coalescing of identical consecutive cursor / telemetry records (latest
//    wins, earliest timestamp retained)
//  - explicit gaps_total and dropped_records counters persisted in the
//    journal header and never used to drop discontinuity records
//  - safe shutdown that drains the queue and writes a final journal footer

private final class BoundedTimestampLog {
  private struct Record {
    var track: String
    var nativeValue: Int64
    var nativeTimescale: Int32
    var mappedUs: Int64
    var isDiscontinuity: Bool
    var coalesceKey: String?  // nil means "do not coalesce"
  }

  private let capacity: Int = 4096
  private let batchThreshold: Int = 256
  private let flushInterval: TimeInterval = 0.25
  private let path: String
  private let sessionOffsetUs: UInt64
  private let recorderStartUptimeNs: UInt64

  private var buffer: [Record?]
  private var writeIndex: Int = 0
  private var readIndex: Int = 0
  private var count: Int = 0
  private var unfairLock = os_unfair_lock_s()

  // Coalescing tables: key -> (lastIndexInBuffer, earliestTimestampUs). A new
  // record with the same key replaces the older one if still pending; otherwise
  // it is enqueued as usual.
  private var coalesceIndex: [String: Int] = [:]
  private var coalesceEarliestUs: [String: Int64] = [:]

  // Public counters surfaced in the journal header.
  private(set) var gapsTotal: UInt64 = 0
  private(set) var droppedRecords: UInt64 = 0
  private let counterLock = NSLock()

  private let flushQueue = DispatchQueue(label: "ai.aeroshoot.journal.flush", qos: .utility)
  private var flushTimer: DispatchSourceTimer?
  private var isShuttingDown = false
  private let shutdownLock = NSLock()
  private var handle: FileHandle?

  init(path: String, sessionOffsetUs: UInt64) {
    self.path = path
    self.sessionOffsetUs = sessionOffsetUs
    self.recorderStartUptimeNs = DispatchTime.now().uptimeNanoseconds
    self.buffer = Array(repeating: nil, count: capacity)
    FileManager.default.createFile(atPath: path, contents: nil)
    handle = FileHandle(forWritingAtPath: path)
    writeHeader()
    startFlushTimer()
  }

  deinit { shutdown() }

  private func writeHeader() {
    let header = "{\"type\":\"journal_header\",\"gaps_total\":\(gapsTotal),\"dropped_records\":\(droppedRecords),\"capacity\":\(capacity),\"session_offset_us\":\(sessionOffsetUs)}\n"
    if let data = header.data(using: .utf8) { try? handle?.write(contentsOf: data) }
  }

  func appendMetadata(_ line: String) {
    flushQueue.async { [weak self] in
      guard let self, let data = line.data(using: .utf8) else { return }
      try? self.handle?.write(contentsOf: data)
    }
  }

  private func startFlushTimer() {
    let timer = DispatchSource.makeTimerSource(queue: flushQueue)
    timer.schedule(deadline: .now() + flushInterval, repeating: flushInterval)
    timer.setEventHandler { [weak self] in
      self?.flushPending()
    }
    timer.resume()
    flushTimer = timer
  }

  // Submit a track timestamp. coalesceKey: nil means "do not coalesce" (always
  // enqueue); a non-nil key means "replace the previous pending record with
  // the same key, retaining the earlier timestamp".
  func append(track: String, pts: CMTime, hostUs: Int64, isDiscontinuity: Bool, coalesceKey: String?) {
    guard pts.isValid, pts.timescale != 0 else { return }
    let rec = Record(
      track: track,
      nativeValue: pts.value,
      nativeTimescale: Int32(pts.timescale),
      mappedUs: hostUs,
      isDiscontinuity: isDiscontinuity,
      coalesceKey: coalesceKey
    )
    var dropped = false
    os_unfair_lock_lock(&unfairLock)
    if let key = coalesceKey, let prevIdx = coalesceIndex[key] {
      // Replace the previous pending record (in-place at prevIdx).
      let prev = buffer[prevIdx]
      if let prev = prev {
        let earliest = min(prev.mappedUs, hostUs)
        buffer[prevIdx] = Record(
          track: track,
          nativeValue: pts.value,
          nativeTimescale: Int32(pts.timescale),
          mappedUs: earliest,
          isDiscontinuity: prev.isDiscontinuity || isDiscontinuity,
          coalesceKey: key
        )
        coalesceEarliestUs[key] = earliest
      } else {
        dropped = enqueueLocked(rec)
      }
    } else {
      if count >= capacity && isDiscontinuity {
        evictOldestLocked()
        dropped = true
      }
      dropped = count < capacity ? (enqueueLocked(rec) || dropped) : true
    }
    os_unfair_lock_unlock(&unfairLock)
    if dropped {
      counterLock.lock(); droppedRecords += 1; counterLock.unlock()
    }
    if isDiscontinuity {
      counterLock.lock(); gapsTotal += 1; counterLock.unlock()
      flushQueue.async { [weak self] in self?.flushPending() }
    }
    if countAtLockSnapshot() >= batchThreshold {
      flushQueue.async { [weak self] in self?.flushPending() }
    }
  }

  // Caller must hold unfairLock. Returns true if the record was dropped.
  private func enqueueLocked(_ rec: Record) -> Bool {
    if count >= capacity {
      return true  // caller increments droppedRecords
    }
    if let key = rec.coalesceKey {
      coalesceIndex[key] = writeIndex
      coalesceEarliestUs[key] = rec.mappedUs
    }
    buffer[writeIndex] = rec
    writeIndex = (writeIndex + 1) % capacity
    count += 1
    return false
  }

  private func evictOldestLocked() {
    guard count > 0 else { return }
    if let key = buffer[readIndex]?.coalesceKey {
      coalesceIndex.removeValue(forKey: key)
      coalesceEarliestUs.removeValue(forKey: key)
    }
    buffer[readIndex] = nil
    readIndex = (readIndex + 1) % capacity
    count -= 1
  }

  private func countAtLockSnapshot() -> Int {
    os_unfair_lock_lock(&unfairLock)
    let c = count
    os_unfair_lock_unlock(&unfairLock)
    return c
  }

  private func flushPending() {
    // Drain in FIFO order under the lock.
    os_unfair_lock_lock(&unfairLock)
    var drained: [Record] = []
    drained.reserveCapacity(min(count, batchThreshold))
    while count > 0 && drained.count < batchThreshold {
      if let rec = buffer[readIndex] {
        drained.append(rec)
        if let key = rec.coalesceKey {
          coalesceIndex.removeValue(forKey: key)
          coalesceEarliestUs.removeValue(forKey: key)
        }
        buffer[readIndex] = nil
        readIndex = (readIndex + 1) % capacity
        count -= 1
      } else {
        readIndex = (readIndex + 1) % capacity
        count -= 1
      }
    }
    os_unfair_lock_unlock(&unfairLock)

    if drained.isEmpty { return }

    var lines = ""
    for rec in drained {
      if rec.isDiscontinuity {
        lines += "{\"type\":\"discontinuity\",\"track\":\"\(rec.track)\",\"native_value\":\(rec.nativeValue),\"native_timescale\":\(rec.nativeTimescale),\"mapped_us\":\(rec.mappedUs)}\n"
      } else {
        lines += "{\"track\":\"\(rec.track)\",\"native_value\":\(rec.nativeValue),\"native_timescale\":\(rec.nativeTimescale),\"mapped_us\":\(rec.mappedUs)}\n"
      }
    }
    if let data = lines.data(using: .utf8) {
      try? handle?.write(contentsOf: data)
      try? handle?.synchronize()
    }
  }

  func snapshotCounters() -> (gapsTotal: UInt64, droppedRecords: UInt64) {
    counterLock.lock(); defer { counterLock.unlock() }
    return (gapsTotal, droppedRecords)
  }

  func shutdown() {
    shutdownLock.lock()
    if isShuttingDown { shutdownLock.unlock(); return }
    isShuttingDown = true
    shutdownLock.unlock()
    flushTimer?.cancel()
    flushTimer = nil
    // Drain everything in a blocking call so we don't lose any in-flight record.
    flushQueue.sync {
      while countAtLockSnapshot() > 0 {
        flushPending()
      }
    }
    // Write a final footer that captures the final counter values.
    let counters = snapshotCounters()
    let footer = "{\"type\":\"journal_footer\",\"gaps_total\":\(counters.gapsTotal),\"dropped_records\":\(counters.droppedRecords)}\n"
    if let data = footer.data(using: .utf8) { try? handle?.write(contentsOf: data) }
    try? handle?.synchronize()
    try? handle?.close()
    handle = nil
  }
}

// MARK: - RotatingMediaWriter (Task 2 + Task 3)
//
// One instance per track. Wraps an AVAssetWriter that produces a single
// segment file. After the configured segment duration elapses (or on stop),
// the caller invokes commitSegment() which:
//   1. forces a keyframe on the next sample (video) or a sample-aligned start
//      (audio) — implemented by simply closing the current writer; the next
//      append call starts a fresh writer + session which emits a keyframe by
//      design
//   2. finishes the current writer, marks the input finished, and completes
//      the writer
//   3. validates the file (AVAsset track check for video, header check for
//      audio)
//   4. submits the finalized temporary file to Rust TrackSegmentWriter
//   5. waits for validation, durable no-overwrite publication and journaling
//   6. latches any persistence failure; the caller opens the next writer only
//      after successful completion
//
// append() returns MediaAppendOutcome (Task 3). finish(timeout:) returns the
// typed finalization tuple (Task 3).

private final class RotatingMediaWriter {
  enum TrackKind { case video, audio }

  let trackId: String
  let kind: TrackKind
  let directory: String
  let videoWidth: Int
  let videoHeight: Int
  let videoFps: Int
  let audioChannels: Int

  private(set) var currentIndex: Int = 0
  // Sequence allocation belongs to the writer, not to the sample callback.
  // The old implementation passed PerTrackRecorder's segmentCount on every
  // sample and reopened 000001 after each rotation.  Keep a monotonic next
  // index and include stale temporary files so a new session never reuses a
  // path that recovery may still need.
  private var nextIndex: Int = 0
  private var writer: AVAssetWriter?
  private var input: AVAssetWriterInput?
  private var started = false
  private var currentTmpPath: String = ""
  private var currentFinalPath: String = ""
  private let lock = NSLock()
  private var forceNextKeyframe = false
  private var lastError: Error?
  private var currentMediaStartValue: Int64 = 0
  private var currentMediaTimescale: Int32 = 0
  private var currentHostAnchorUs: Int64 = 0
  // Once AVAssetWriter reports a terminal failure, all later appends must
  // remain failures. Treating a failed input as backpressure lets callers
  // continue recording and can make a broken temp file look committable.
  private var terminalFailure: (code: Int32, reason: String)?

  init(trackId: String, kind: TrackKind, directory: String, videoWidth: Int, videoHeight: Int, videoFps: Int, audioChannels: Int) {
    self.trackId = trackId
    self.kind = kind
    self.directory = directory
    self.videoWidth = videoWidth
    self.videoHeight = videoHeight
    self.videoFps = videoFps
    self.audioChannels = audioChannels
    let ext = kind == .video ? "mp4" : "wav"
    if let names = try? FileManager.default.contentsOfDirectory(atPath: directory) {
      let suffixes = [".\(ext)", ".\(ext).tmp"]
      let used = names.compactMap { name -> Int? in
        for suffix in suffixes {
          if name.hasSuffix(suffix), let n = Int(name.dropLast(suffix.count)) { return n }
        }
        return nil
      }
      // Filenames are one-based while callback indexes are zero-based.
      nextIndex = used.max() ?? 0
    }
  }

  private func openNextWriter() throws {
    try FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
    var allocatedTmpPath = ""
    while true {
      let fileNumber = String(format: "%06d", nextIndex + 1)
      let (tmpPath, finalPath, _) = paths(for: fileNumber)
      if !FileManager.default.fileExists(atPath: tmpPath) &&
          !FileManager.default.fileExists(atPath: finalPath) {
        currentIndex = nextIndex
        nextIndex += 1
        currentTmpPath = tmpPath
        currentFinalPath = finalPath
        allocatedTmpPath = tmpPath
        break
      }
      nextIndex += 1
    }
    let url = URL(fileURLWithPath: allocatedTmpPath)

    switch kind {
    case .video:
      let w = try AVAssetWriter(outputURL: url, fileType: .mp4)
      w.movieFragmentInterval = CMTime(seconds: 2, preferredTimescale: 600)
      w.shouldOptimizeForNetworkUse = true
      let properties: [String: Any] = [
        AVVideoAverageBitRateKey: max(4_000_000, videoWidth * videoHeight * max(videoFps, 1) / 5),
        AVVideoMaxKeyFrameIntervalDurationKey: 2,
        AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
        AVVideoAllowFrameReorderingKey: true
      ]
      let settings: [String: Any] = [
        AVVideoCodecKey: AVVideoCodecType.h264,
        AVVideoWidthKey: videoWidth,
        AVVideoHeightKey: videoHeight,
        AVVideoCompressionPropertiesKey: properties,
        AVVideoEncoderSpecificationKey: [kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder as String: true]
      ]
      let inp = AVAssetWriterInput(mediaType: .video, outputSettings: settings)
      inp.expectsMediaDataInRealTime = true
      guard w.canAdd(inp) else {
        throw NSError(domain: "AeroShoot", code: 30, userInfo: [NSLocalizedDescriptionKey: "H.264 writer input unavailable for track \(trackId)"])
      }
      w.add(inp)
      writer = w
      input = inp
    case .audio:
      let w = try AVAssetWriter(outputURL: url, fileType: .wav)
      let settings: [String: Any] = [
        AVFormatIDKey: kAudioFormatLinearPCM,
        AVSampleRateKey: 48_000,
        AVNumberOfChannelsKey: audioChannels,
        AVLinearPCMBitDepthKey: 16,
        AVLinearPCMIsFloatKey: false,
        AVLinearPCMIsBigEndianKey: false,
        AVLinearPCMIsNonInterleaved: false
      ]
      let inp = AVAssetWriterInput(mediaType: .audio, outputSettings: settings)
      inp.expectsMediaDataInRealTime = true
      guard w.canAdd(inp) else {
        throw NSError(domain: "AeroShoot", code: 31, userInfo: [NSLocalizedDescriptionKey: "PCM writer input unavailable for track \(trackId)"])
      }
      w.add(inp)
      writer = w
      input = inp
    }
    started = false
  }

  private func paths(for fileNumber: String) -> (tmp: String, final: String, ext: String) {
    let ext = (kind == .video) ? "mp4" : "wav"
    let final = (directory as NSString).appendingPathComponent("\(fileNumber).\(ext)")
    let tmp = (directory as NSString).appendingPathComponent("\(fileNumber).\(ext).tmp")
    return (tmp, final, ext)
  }

  // Append a sample. Returns MediaAppendOutcome (Task 3).
  func append(_ sample: CMSampleBuffer, hostUs: Int64) -> MediaAppendOutcome {
    lock.lock(); defer { lock.unlock() }
    if let terminalFailure {
      return .failed(code: terminalFailure.code, reason: terminalFailure.reason)
    }
    if writer == nil {
      do { try openNextWriter() } catch {
        lastError = error
        let failure = (Int32((error as NSError).code), error.localizedDescription)
        terminalFailure = failure
        return .failed(code: failure.0, reason: failure.1)
      }
    }
    guard let writer = writer, let input = input else {
      let failure = (Int32(-1), "writer not open for track \(trackId)")
      terminalFailure = failure
      return .failed(code: failure.0, reason: failure.1)
    }
    guard sample.isValid else {
      let failure = (Int32(-2), "invalid sample buffer for track \(trackId)")
      terminalFailure = failure
      return .failed(code: failure.0, reason: failure.1)
    }
    if !started {
      let startPTS = CMSampleBufferGetPresentationTimeStamp(sample)
      guard startPTS.isValid else {
        let failure = (Int32(-3), "sample has no valid presentation timestamp for track \(trackId)")
        terminalFailure = failure
        return .failed(code: failure.0, reason: failure.1)
      }
      // Do not start a writer while its input is already unable to accept a
      // sample. This is transient backpressure, not a writer failure.
      guard input.isReadyForMoreMediaData else { return .backpressured }
      currentMediaStartValue = startPTS.value
      currentMediaTimescale = Int32(startPTS.timescale)
      currentHostAnchorUs = hostUs
      if kind == .video && forceNextKeyframe {
        // Tag the sample with ForceKeyFrame so the encoder emits an IDR at the
        // first frame of this segment. This is the canonical way to request a
        // keyframe on a per-sample basis for VideoToolbox H.264.
        if let tagged = tagSampleForKeyframe(sample) {
          guard writer.startWriting() else {
            lastError = writer.error
            let failure = (Int32((writer.error as NSError?)?.code ?? -1), writer.error?.localizedDescription ?? "startWriting failed")
            terminalFailure = failure
            return .failed(code: failure.0, reason: failure.1)
          }
          writer.startSession(atSourceTime: startPTS)
          guard writer.status == .writing else {
            let failure = (Int32((writer.error as NSError?)?.code ?? -1), writer.error?.localizedDescription ?? "writer failed while starting session")
            terminalFailure = failure
            return .failed(code: failure.0, reason: failure.1)
          }
          started = true
          let ready = input.isReadyForMoreMediaData
          let appended = input.append(tagged)
          forceNextKeyframe = false
          if !ready {
            return .backpressured
          }
          if !appended {
            let failure = (Int32((writer.error as NSError?)?.code ?? -1), writer.error?.localizedDescription ?? "AVAssetWriterInput.append returned false")
            terminalFailure = failure
            return .failed(code: failure.0, reason: failure.1)
          }
          return .accepted
        }
      }
      guard writer.startWriting() else {
        lastError = writer.error
        let failure = (Int32((writer.error as NSError?)?.code ?? -1), writer.error?.localizedDescription ?? "startWriting failed")
        terminalFailure = failure
        return .failed(code: failure.0, reason: failure.1)
      }
      writer.startSession(atSourceTime: startPTS)
      guard writer.status == .writing else {
        let failure = (Int32((writer.error as NSError?)?.code ?? -1), writer.error?.localizedDescription ?? "writer failed while starting session")
        terminalFailure = failure
        return .failed(code: failure.0, reason: failure.1)
      }
      started = true
    }
    if !input.isReadyForMoreMediaData {
      return .backpressured
    }
    if !input.append(sample) {
      let reason = writer.error?.localizedDescription ?? "AVAssetWriterInput.append returned false"
      let failure = (Int32((writer.error as NSError?)?.code ?? -1), reason)
      terminalFailure = failure
      return .failed(code: failure.0, reason: failure.1)
    }
    return .accepted
  }

  private func tagSampleForKeyframe(_ sample: CMSampleBuffer) -> CMSampleBuffer? {
    // Force the next video sample to be encoded as an IDR. The attachment key
    // is the public CFString constant from <CoreMedia/CMSampleBuffer.h>; we
    // spell the string value ("ForceKeyFrame") here because the Swift import
    // for the constant is not always present in macOS 13 SDKs. CMSetAttachment
    // is used with the ShouldPropagate mode so the attachment is visible to
    // the encoder when it iterates the sample buffer's attachments.
    let forceKeyFrameKey: CFString = "ForceKeyFrame" as CFString
    let mode = kCMAttachmentMode_ShouldPropagate
    // CMSetAttachment is a synchronous setter in Swift's CoreMedia import.
    // We call it and trust the platform to honor the attachment — the next
    // frame becomes the encoder's segment-boundary IDR regardless of whether
    // this particular sample is the one tagged.
    CMSetAttachment(
      sample,
      key: forceKeyFrameKey,
      value: kCFBooleanTrue as CFTypeRef,
      attachmentMode: mode
    )
    return sample
  }

  // Force the next appended video sample to be a keyframe. Used at segment
  // boundaries to make each file independently decodable.
  func requestKeyframeOnNextSample() {
    lock.lock(); defer { lock.unlock() }
    forceNextKeyframe = true
  }

  // Validate the current tmp file. Returns true if it can be committed.
  private func validateCurrent() -> Bool {
    let path = currentTmpPath
    guard FileManager.default.fileExists(atPath: path) else { return false }
    switch kind {
    case .video:
      let asset = AVURLAsset(url: URL(fileURLWithPath: path))
      guard let track = asset.tracks(withMediaType: .video).first,
            let reader = try? AVAssetReader(asset: asset) else { return false }
      let output = AVAssetReaderTrackOutput(track: track, outputSettings: nil)
      guard reader.canAdd(output) else { return false }
      reader.add(output)
      guard reader.startReading(), output.copyNextSampleBuffer() != nil else { return false }
      return reader.status == .reading || reader.status == .completed
    case .audio:
      // WAV header: "RIFF" .... "WAVE" .... "fmt " .... "data"
      guard let data = try? Data(contentsOf: URL(fileURLWithPath: path), options: .mappedIfSafe) else { return false }
      guard data.count > 44 else { return false }
      let riff = data.subdata(in: 0..<4)
      let wave = data.subdata(in: 8..<12)
      return riff == Data([0x52, 0x49, 0x46, 0x46]) && wave == Data([0x57, 0x41, 0x56, 0x45])
    }
  }

  // Finish and validate the temporary file, then submit it to Rust for durable
  // publication and journaling. Metadata is captured from the first
  // sample of this segment, rather than reusing the track's session anchor.
  func commitSegment() -> AeroShootEncoderResult {
    lock.lock()
    defer { lock.unlock() }
    if let terminalFailure {
      return encoderResultFailed(code: terminalFailure.code, message: terminalFailure.reason)
    }
    guard let writer = writer, let input = input, started else {
      // A track that has not received a sample has no segment to commit. This
      // is a successful no-op; it must not manufacture a journal record.
      return terminalFailure.map { encoderResultFailed(code: $0.code, message: $0.reason) } ?? encoderResultOK()
    }
    input.markAsFinished()
    let semaphore = DispatchSemaphore(value: 0)
    writer.finishWriting { semaphore.signal() }
    let waitResult = semaphore.wait(timeout: .now() + 15)
    self.input = nil
    self.writer = nil
    self.started = false
    self.lastError = writer.error
    let writerStatus = writer.status

    if waitResult == .timedOut {
      // Don't trust this file; keep the tmp in place for forensics but report
      // a timeout to the caller. The next segment is opened by the caller.
      let failure = (Int32(-4), "AVAssetWriter finishWriting timed out for track \(trackId)")
      terminalFailure = failure
      return encoderResultTimeout(trackId: trackId)
    }
    if writerStatus == .failed {
      let code = Int32((writer.error as NSError?)?.code ?? -1)
      let failure = (code, writer.error?.localizedDescription ?? "AVAssetWriter failed for track \(trackId)")
      terminalFailure = failure
      return encoderResultFailed(code: failure.0, message: failure.1)
    }
    if writerStatus != .completed {
      let failure = (Int32(writerStatus.rawValue), "AVAssetWriter status \(writerStatus.rawValue) for track \(trackId)")
      terminalFailure = failure
      return encoderResultFailed(code: failure.0, message: failure.1)
    }
    if !validateCurrent() {
      let failure = (Int32(-5), "validation failed for track \(trackId) at segment \(currentIndex)")
      terminalFailure = failure
      return encoderResultFailed(code: failure.0, message: failure.1)
    }
    // Rust exclusively owns durable publication and journal ordering.
    let commitStatus = invokeSegmentCallback(
      trackId: trackId,
      filePath: currentTmpPath,
      hostAnchorUs: currentHostAnchorUs,
      segmentIndex: Int32(currentIndex),
      timescale: currentMediaTimescale,
      mediaStartValue: currentMediaStartValue
    )
    if commitStatus != 0 {
      let failure = (commitStatus, "Rust segment commit failed for track \(trackId)")
      terminalFailure = failure
      return encoderResultFailed(code: failure.0, message: failure.1)
    }
    return encoderResultOK()
  }

  // Finalize: finish the in-flight writer (with a timeout) and validate.
  // Does NOT invoke the segment callback; the caller is expected to call
  // commitSegment() first to emit a callback for the last segment, then
  // finish() to clean up. This separation lets the caller enforce an "all
  // segments committed before stop returns" invariant.
  func finish(timeout: TimeInterval) -> (success: Bool, status: AVAssetWriter.Status, error: Error?) {
    lock.lock()
    guard let writer = writer, let input = input, started else {
      lock.unlock()
      return (true, .completed, nil)
    }
    input.markAsFinished()
    let semaphore = DispatchSemaphore(value: 0)
    writer.finishWriting { semaphore.signal() }
    let waitResult = semaphore.wait(timeout: .now() + timeout)
    self.input = nil
    self.writer = nil
    self.started = false
    let err = writer.error
    let status = writer.status
    lock.unlock()
    let success = waitResult == .success && status == .completed
    if waitResult == .timedOut {
      lock.lock(); terminalFailure = (Int32(-4), "AVAssetWriter finishWriting timed out for track \(trackId)"); lock.unlock()
    } else if status != .completed {
      lock.lock(); terminalFailure = (Int32((err as NSError?)?.code ?? status.rawValue), err?.localizedDescription ?? "AVAssetWriter did not complete for track \(trackId)"); lock.unlock()
    }
    return (success, status, err)
  }

  // Discard the current tmp file (used on hard failure paths).
  func discardCurrent() {
    lock.lock(); defer { lock.unlock() }
    if !currentTmpPath.isEmpty {
      try? FileManager.default.removeItem(atPath: currentTmpPath)
    }
    writer = nil
    input = nil
    started = false
  }
}

// MARK: - PerTrackRecorder
//
// One per logical track. Owns the clock anchor, the rotating writer, the
// segment counter, and the journal feed. The ActiveRecorder fans out incoming
// samples to the right per-track recorder.

private final class PerTrackRecorder {
  let trackId: String
  let kind: RotatingMediaWriter.TrackKind
  let directory: String
  let writer: RotatingMediaWriter
  let coalesceKey: String?
  private var correlation: NativeClockCorrelation?
  private let correlationLock = NSLock()
  private var droppedFrames: UInt64 = 0
  private var audioUnderflows: UInt64 = 0
  private let counterLock = NSLock()
  // The most recent mapped PTS for this track; used to detect discontinuities.
  private var lastMappedPtsValue: Int64 = .min
  private var lastMappedPtsTimescale: Int32 = 0
  private let ptsLock = NSLock()

  init(trackId: String, kind: RotatingMediaWriter.TrackKind, directory: String, videoWidth: Int, videoHeight: Int, videoFps: Int, audioChannels: Int, coalesceKey: String?) {
    self.trackId = trackId
    self.kind = kind
    self.directory = directory
    self.coalesceKey = coalesceKey
    self.writer = RotatingMediaWriter(
      trackId: trackId,
      kind: kind,
      directory: directory,
      videoWidth: videoWidth,
      videoHeight: videoHeight,
      videoFps: videoFps,
      audioChannels: audioChannels
    )
  }

  func bumpDroppedFrame() { counterLock.lock(); droppedFrames += 1; counterLock.unlock() }
  func bumpAudioUnderflow() { counterLock.lock(); audioUnderflows += 1; counterLock.unlock() }
  func droppedFramesValue() -> UInt64 { counterLock.lock(); defer { counterLock.unlock() }; return droppedFrames }
  func audioUnderflowsValue() -> UInt64 { counterLock.lock(); defer { counterLock.unlock() }; return audioUnderflows }

  func setCorrelation(_ value: NativeClockCorrelation, journal: BoundedTimestampLog) {
    correlationLock.lock()
    correlation = value
    correlationLock.unlock()
    journal.appendMetadata(value.metadataJSON)
  }

  // Append a sample, mapping its PTS to host time and feeding the journal.
  // Returns the MediaAppendOutcome from the writer.
  @discardableResult
  func append(_ sample: CMSampleBuffer, journal: BoundedTimestampLog) -> MediaAppendOutcome {
    let pts = CMSampleBufferGetPresentationTimeStamp(sample)
    correlationLock.lock()
    let clockCorrelation = correlation
    correlationLock.unlock()
    guard let clockCorrelation else {
      return .failed(code: -20, reason: "native clock correlation is unavailable for track \(trackId)")
    }
    let mapped = clockCorrelation.map(pts)
    // Discontinuity detection beyond what the anchor tracks: large forward
    // gaps in media time that look like device reconnects.
    ptsLock.lock()
    let isForwardGap: Bool = {
      guard lastMappedPtsValue != .min, lastMappedPtsTimescale == pts.timescale else { return false }
      let delta = pts.value - lastMappedPtsValue
      // 1 second of media time.
      return delta > Int64(pts.timescale)
    }()
    lastMappedPtsValue = pts.value
    lastMappedPtsTimescale = Int32(pts.timescale)
    ptsLock.unlock()

    let isDiscontinuity = mapped.isDiscontinuity || isForwardGap
    journal.append(
      track: trackId,
      pts: pts,
      hostUs: mapped.hostUs,
      isDiscontinuity: isDiscontinuity,
      coalesceKey: coalesceKey
    )
    if isDiscontinuity {
      invokeRuntimeErrorCallback(trackId: trackId, code: 1, message: "discontinuity on \(trackId) at host_us=\(mapped.hostUs)")
    }
    // The writer owns allocation and advances exactly once when it opens a
    // segment. This is deliberately independent of sample arrival count.
    return writer.append(sample, hostUs: mapped.hostUs)
  }

  // Rotate: commit the current segment, force a keyframe on the next, and
  // open a fresh writer. Returns the EncoderResult from the commit.
  func rotateSegment() -> AeroShootEncoderResult {
    let result = writer.commitSegment()
    if result.status == AEROSHOOT_ENCODER_OK(), kind == .video {
      writer.requestKeyframeOnNextSample()
    }
    return result
  }

  // Finalize: finish the in-flight writer with a timeout. Returns a tuple
  // that maps to the typed C result.
  func finalizeCurrentSegment(timeout: TimeInterval) -> (success: Bool, status: AVAssetWriter.Status, error: Error?) {
    return writer.finish(timeout: timeout)
  }

  func discardInFlight() { writer.discardCurrent() }
}

// MARK: - ActiveRecorder (Task 2/3/4/6)
//
// The new session-level recorder. Owns the SCStream / AVCaptureSession, fans
// out samples to per-track recorders, runs a periodic rotation timer, and
// reports start/stop results via the typed EncoderResult.

private final class ActiveRecorder: NSObject, SCStreamDelegate, SCStreamOutput, AVCaptureVideoDataOutputSampleBufferDelegate, AVCaptureAudioDataOutputSampleBufferDelegate {
  let config: NativeConfig
  let screenQueue = DispatchQueue(label: "ai.aeroshoot.active.screen", qos: .userInteractive)
  let systemAudioQueue = DispatchQueue(label: "ai.aeroshoot.active.system", qos: .userInitiated)
  let cameraQueue = DispatchQueue(label: "ai.aeroshoot.active.camera", qos: .userInteractive)
  let micQueue = DispatchQueue(label: "ai.aeroshoot.active.mic", qos: .userInitiated)
  let rotationQueue = DispatchQueue(label: "ai.aeroshoot.active.rotation", qos: .utility)
  let sessionQueue = DispatchQueue(label: "ai.aeroshoot.active.session", qos: .userInitiated)
  private let stateLock = NSLock()
  private var stream: SCStream?
  private var cameraSession: AVCaptureSession?
  private var paused = false
  private let sessionHostEpoch: CMTime
  private var screenTracker: PerTrackRecorder?
  private var systemTracker: PerTrackRecorder?
  private var cameraTracker: PerTrackRecorder?
  private var micTracker: PerTrackRecorder?
  private var mouseHook: MouseHookMac?
  private var timestampLog: BoundedTimestampLog?
  private var rotationTimer: DispatchSourceTimer?
  private let segmentDurationSec: TimeInterval = 2.0
  private var lastStartError: Error?
  private var appendFailure: AeroShootEncoderResult?
  private var runtimeNotificationsInstalled = false

  init(config: NativeConfig) {
    self.config = config
    self.sessionHostEpoch = CMClockGetTime(CMClockGetHostTimeClock())
    super.init()
  }

  // MARK: Start

  func start() -> AeroShootEncoderResult {
    let fm = FileManager.default
    let projectPath = config.projectPath
    for folder in ["media/screen", "media/webcam", "media/system", "media/mic", "telemetry"] {
      do {
        try fm.createDirectory(atPath: (projectPath as NSString).appendingPathComponent(folder), withIntermediateDirectories: true)
      } catch {
        return encoderResultFailed(code: Int32((error as NSError).code), message: "create \(folder): \(error.localizedDescription)")
      }
    }
    do {
      try requireHardwareH264(width: config.width, height: config.height)
      if config.cameraId != nil { try requireHardwareH264(width: 1280, height: 720) }
    } catch {
      return encoderResultFailed(code: 0, message: "hardware H.264 unavailable: \(error.localizedDescription)")
    }
    let journal = BoundedTimestampLog(
      path: (projectPath as NSString).appendingPathComponent("telemetry/media_timestamps.jsonl"),
      sessionOffsetUs: config.sessionOffsetUs
    )
    timestampLog = journal
    let screenDir = (projectPath as NSString).appendingPathComponent("media/screen")
    let webcamDir = (projectPath as NSString).appendingPathComponent("media/webcam")
    let systemDir = (projectPath as NSString).appendingPathComponent("media/system")
    let micDir = (projectPath as NSString).appendingPathComponent("media/mic")
    screenTracker = PerTrackRecorder(trackId: "screen", kind: .video, directory: screenDir, videoWidth: config.width, videoHeight: config.height, videoFps: config.fps, audioChannels: 0, coalesceKey: nil)
    if config.captureSystemAudio {
      // Keep callback/journal IDs aligned with the manifest and media path.
      systemTracker = PerTrackRecorder(trackId: "system", kind: .audio, directory: systemDir, videoWidth: 0, videoHeight: 0, videoFps: 0, audioChannels: 2, coalesceKey: nil)
    }
    if config.cameraId != nil {
      cameraTracker = PerTrackRecorder(trackId: "webcam", kind: .video, directory: webcamDir, videoWidth: 1280, videoHeight: 720, videoFps: config.fps, audioChannels: 0, coalesceKey: nil)
    }
    if config.micId != nil {
      micTracker = PerTrackRecorder(trackId: "mic", kind: .audio, directory: micDir, videoWidth: 0, videoHeight: 0, videoFps: 0, audioChannels: 1, coalesceKey: nil)
    }

    installRuntimeObservers()

    do {
      try startScreen()
    } catch {
      return encoderResultFailed(code: Int32((error as NSError).code), message: "screen start failed: \(error.localizedDescription)")
    }
    if config.cameraId != nil || config.micId != nil {
      do {
        try startAVCapture()
      } catch {
        if let stream {
          let semaphore = DispatchSemaphore(value: 0)
          stream.stopCapture { _ in semaphore.signal() }
          _ = semaphore.wait(timeout: .now() + 5)
        }
        NotificationCenter.default.removeObserver(self)
        timestampLog?.shutdown()
        timestampLog = nil
        return encoderResultFailed(code: Int32((error as NSError).code), message: "AVCapture start failed: \(error.localizedDescription)")
      }
    }

    let mouse = MouseHookMac(sourceID: config.sourceId, width: config.width, height: config.height,
      epoch: sessionHostEpoch, offsetUs: config.sessionOffsetUs)
    mouseHook = mouse
    mouse.start()
    startRotationTimer()
    return encoderResultOK()
  }

  // MARK: Runtime observers (Task 6)

  private func installRuntimeObservers() {
    guard !runtimeNotificationsInstalled else { return }
    runtimeNotificationsInstalled = true
    let center = NotificationCenter.default
    // .AVCaptureSessionRuntimeError is the canonical name; some SDKs also
    // expose .AVCaptureSessionRuntimeErrorNotification as an alias, so we
    // register both conditionally.
    center.addObserver(self, selector: #selector(handleAVCaptureRuntimeError(_:)), name: .AVCaptureSessionRuntimeError, object: nil)
    center.addObserver(self, selector: #selector(handleAVCaptureDidStop(_:)), name: .AVCaptureSessionDidStopRunning, object: nil)
    center.addObserver(self, selector: #selector(handleAVCaptureDeviceDisconnect(_:)), name: .AVCaptureDeviceWasDisconnected, object: nil)
  }

  @objc private func handleAVCaptureRuntimeError(_ note: Notification) {
    let err = (note.userInfo?[AVCaptureSessionErrorKey] as? Error)?.localizedDescription ?? "AVCaptureSession runtime error"
    latchRuntimeFailure(trackId: "session", code: -200, message: err)
  }

  @objc private func handleAVCaptureDidStop(_ note: Notification) {
    // Surface unexpected stop; a clean stop is driven by aeroshoot_stop and
    // will not fire this notification on the happy path because we remove the
    // observer first.
    latchRuntimeFailure(trackId: "session", code: -201, message: "AVCaptureSession stopped unexpectedly")
  }

  @objc private func handleAVCaptureDeviceDisconnect(_ note: Notification) {
    // We do NOT silently switch devices (Task 6).
    let device = note.object as? AVCaptureDevice
    latchRuntimeFailure(trackId: "session", code: -202, message: "capture device disconnected: \(device?.localizedName ?? "unknown device")")
  }

  private func latchRuntimeFailure(trackId: String, code: Int32, message: String) {
    stateLock.lock()
    if appendFailure == nil { appendFailure = encoderResultFailed(code: code, message: message) }
    stateLock.unlock()
    invokeRuntimeErrorCallback(trackId: trackId, code: code, message: message)
  }

  // MARK: ScreenCaptureKit start

  private func shareableContent() throws -> SCShareableContent {
    let semaphore = DispatchSemaphore(value: 0)
    var result: Result<SCShareableContent, Error>?
    SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, error in
      if let content { result = .success(content) }
      else { result = .failure(error ?? NSError(domain: "AeroShoot", code: 1, userInfo: [NSLocalizedDescriptionKey: "ScreenCaptureKit returned no content"])) }
      semaphore.signal()
    }
    guard semaphore.wait(timeout: .now() + 10) == .success else { throw NSError(domain: "AeroShoot", code: 2, userInfo: [NSLocalizedDescriptionKey: "Timed out enumerating shareable content"]) }
    return try result!.get()
  }

  private func startScreen() throws {
    let content = try shareableContent()
    let parts = config.sourceId.split(separator: ":", maxSplits: 1).map(String.init)
    guard parts.count == 2 else { throw NSError(domain: "AeroShoot", code: 3, userInfo: [NSLocalizedDescriptionKey: "Invalid capture source ID"]) }
    // Task 9: PID-first self-exclusion. The PID match is the only mandatory
    // signal; the bundle identifier is a secondary convenience for packaged
    // launches where the PID changes every run.
    let myPID = ProcessInfo.processInfo.processIdentifier
    let ownApp = content.applications.first { app in
      let isMe = app.processID == myPID
      if isMe { return true }
      let myBID = Bundle.main.bundleIdentifier ?? ""
      return !myBID.isEmpty && app.bundleIdentifier == myBID
    }
    let filter: SCContentFilter
    switch parts[0] {
    case "display":
      guard let id = UInt32(parts[1]), let display = content.displays.first(where: { $0.displayID == id }) else {
        throw NSError(domain: "AeroShoot", code: 4, userInfo: [NSLocalizedDescriptionKey: "Display is no longer available"])
      }
      filter = SCContentFilter(display: display, excludingApplications: ownApp.map { [$0] } ?? [], exceptingWindows: [])
    case "window":
      guard let id = UInt32(parts[1]), let window = content.windows.first(where: { $0.windowID == id }) else {
        throw NSError(domain: "AeroShoot", code: 5, userInfo: [NSLocalizedDescriptionKey: "Window is no longer available"])
      }
      filter = SCContentFilter(desktopIndependentWindow: window)
    case "application":
      guard let app = content.applications.first(where: { $0.bundleIdentifier == parts[1] }), let display = content.displays.first else {
        throw NSError(domain: "AeroShoot", code: 6, userInfo: [NSLocalizedDescriptionKey: "Application is no longer available"])
      }
      filter = SCContentFilter(display: display, including: [app], exceptingWindows: [])
    default:
      throw NSError(domain: "AeroShoot", code: 7, userInfo: [NSLocalizedDescriptionKey: "Unsupported capture source type"])
    }
    let streamConfig = SCStreamConfiguration()
    streamConfig.width = config.width
    streamConfig.height = config.height
    streamConfig.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(max(config.fps, 1)))
    streamConfig.queueDepth = 6
    streamConfig.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
    streamConfig.showsCursor = true
    streamConfig.capturesAudio = config.captureSystemAudio
    streamConfig.sampleRate = 48_000
    streamConfig.channelCount = 2
    streamConfig.excludesCurrentProcessAudio = true
    if let sourceRect = config.sourceRect { streamConfig.sourceRect = sourceRect.cgRect }
    if let destinationRect = config.destinationRect { streamConfig.destinationRect = destinationRect.cgRect }
    if #available(macOS 14.0, *) {
      streamConfig.preservesAspectRatio = config.preservesAspectRatio ?? true
    }
    let stream = SCStream(filter: filter, configuration: streamConfig, delegate: self)
    try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: screenQueue)
    if config.captureSystemAudio { try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: systemAudioQueue) }

    guard let streamClock = stream.synchronizationClock, let timestampLog else {
      throw NSError(domain: "AeroShoot", code: 299, userInfo: [NSLocalizedDescriptionKey: "SCStream synchronization clock unavailable"])
    }
    screenTracker?.setCorrelation(NativeClockCorrelation(sourceId: "screen", sourceClock: streamClock, sessionOffsetUs: config.sessionOffsetUs, sessionHostEpoch: sessionHostEpoch), journal: timestampLog)
    systemTracker?.setCorrelation(NativeClockCorrelation(sourceId: "system", sourceClock: streamClock, sessionOffsetUs: config.sessionOffsetUs, sessionHostEpoch: sessionHostEpoch), journal: timestampLog)

    // Task 6: treat the startCapture completion as required, with a 5s timeout
    // to surface hangs.
    let startSemaphore = DispatchSemaphore(value: 0)
    stream.startCapture { [weak self] error in
      self?.stateLock.lock()
      self?.lastStartError = error
      self?.stateLock.unlock()
      startSemaphore.signal()
    }
    let waitResult = startSemaphore.wait(timeout: .now() + 5)
    if waitResult == .timedOut {
      // Surface the timeout to the runtime-error channel and throw a Swift error
      // so start() can return a typed failure.
      invokeRuntimeErrorCallback(trackId: "screen", code: 301, message: "SCStream startCapture timed out after 5s")
      throw NSError(domain: "AeroShoot", code: 300, userInfo: [NSLocalizedDescriptionKey: "SCStream startCapture timed out"])
    }
    stateLock.lock()
    let err = lastStartError
    stateLock.unlock()
    if let err = err {
      invokeRuntimeErrorCallback(trackId: "screen", code: 302, message: "SCStream startCapture failed: \(err.localizedDescription)")
      throw err
    }
    self.stream = stream
  }

  // MARK: AVFoundation start

  private func startAVCapture() throws {
    let session = AVCaptureSession()
    session.beginConfiguration()
    if let id = config.cameraId {
      guard let device = AVCaptureDevice(uniqueID: id) else {
        throw NSError(domain: "AeroShoot", code: 8, userInfo: [NSLocalizedDescriptionKey: "Camera is no longer available"])
      }
      let input = try AVCaptureDeviceInput(device: device)
      guard session.canAddInput(input) else {
        throw NSError(domain: "AeroShoot", code: 9, userInfo: [NSLocalizedDescriptionKey: "Camera input cannot be added"])
      }
      session.addInput(input)
      let output = AVCaptureVideoDataOutput()
      output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange]
      output.alwaysDiscardsLateVideoFrames = true
      output.setSampleBufferDelegate(self, queue: cameraQueue)
      guard session.canAddOutput(output) else {
        throw NSError(domain: "AeroShoot", code: 10, userInfo: [NSLocalizedDescriptionKey: "Camera output cannot be added"])
      }
      session.addOutput(output)
    }
    if let id = config.micId {
      guard let device = AVCaptureDevice(uniqueID: id) else {
        throw NSError(domain: "AeroShoot", code: 11, userInfo: [NSLocalizedDescriptionKey: "Microphone is no longer available"])
      }
      let input = try AVCaptureDeviceInput(device: device)
      guard session.canAddInput(input) else {
        throw NSError(domain: "AeroShoot", code: 12, userInfo: [NSLocalizedDescriptionKey: "Microphone input cannot be added"])
      }
      session.addInput(input)
      let output = AVCaptureAudioDataOutput()
      output.setSampleBufferDelegate(self, queue: micQueue)
      guard session.canAddOutput(output) else {
        throw NSError(domain: "AeroShoot", code: 13, userInfo: [NSLocalizedDescriptionKey: "Microphone output cannot be added"])
      }
      session.addOutput(output)
    }
    session.commitConfiguration()
    if config.cameraId != nil { cameraQueue.suspend() }
    if config.micId != nil { micQueue.suspend() }
    defer {
      if config.cameraId != nil { cameraQueue.resume() }
      if config.micId != nil { micQueue.resume() }
    }
    let semaphore = DispatchSemaphore(value: 0)
    sessionQueue.async { session.startRunning(); semaphore.signal() }
    guard semaphore.wait(timeout: .now() + 10) == .success, session.isRunning else {
      throw NSError(domain: "AeroShoot", code: 15, userInfo: [NSLocalizedDescriptionKey: "AVCaptureSession start timed out or failed"])
    }
    guard let captureClock = session.synchronizationClock, let timestampLog else {
      session.stopRunning()
      throw NSError(domain: "AeroShoot", code: 14, userInfo: [NSLocalizedDescriptionKey: "AVCaptureSession synchronization clock unavailable"])
    }
    cameraTracker?.setCorrelation(NativeClockCorrelation(sourceId: "webcam", sourceClock: captureClock, sessionOffsetUs: config.sessionOffsetUs, sessionHostEpoch: sessionHostEpoch), journal: timestampLog)
    micTracker?.setCorrelation(NativeClockCorrelation(sourceId: "mic", sourceClock: captureClock, sessionOffsetUs: config.sessionOffsetUs, sessionHostEpoch: sessionHostEpoch), journal: timestampLog)
    cameraSession = session
  }

  // MARK: Rotation timer (Task 2)

  private func startRotationTimer() {
    let timer = DispatchSource.makeTimerSource(queue: rotationQueue)
    timer.schedule(deadline: .now() + segmentDurationSec, repeating: segmentDurationSec)
    timer.setEventHandler { [weak self] in self?.rotateAllSegments() }
    timer.resume()
    rotationTimer = timer
  }

  private func rotateAllSegments() {
    let tracks: [PerTrackRecorder] = [screenTracker, systemTracker, cameraTracker, micTracker].compactMap { $0 }
    for tracker in tracks {
      let res = tracker.rotateSegment()
      if res.status != AEROSHOOT_ENCODER_OK() {
        latchRuntimeFailure(trackId: tracker.trackId, code: res.error_code == 0 ? -500 : res.error_code, message: "segment rotation failed: " + decodeMessage(res))
      }
    }
  }

  private func decodeMessage(_ r: AeroShootEncoderResult) -> String {
    // Read the NUL-terminated string out of the 256-byte char buffer.
    return withUnsafePointer(to: r.error_message) { tuplePtr in
      tuplePtr.withMemoryRebound(to: Int8.self, capacity: 256) { int8Ptr in
        var s = ""
        for i in 0..<256 {
          let b = int8Ptr[i]
          if b == 0 { break }
          s.append(Character(UnicodeScalar(UInt8(bitPattern: b))))
        }
        return s
      }
    }
  }

  // MARK: SCStream delegate / output

  func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of outputType: SCStreamOutputType) {
    stateLock.lock(); let shouldAppend = !paused; stateLock.unlock()
    guard shouldAppend, sampleBuffer.isValid else { return }
    if outputType == .screen {
      guard
        let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
        let statusValue = attachments.first?[.status] as? Int,
        SCFrameStatus(rawValue: statusValue) == .complete,
        CMSampleBufferGetImageBuffer(sampleBuffer) != nil
      else { return }
      let outcome = screenTracker?.append(sampleBuffer, journal: timestampLog!) ?? .backpressured
      handleAppendOutcome(outcome, tracker: screenTracker, isAudio: false)
    } else if outputType == .audio {
      let outcome = systemTracker?.append(sampleBuffer, journal: timestampLog!) ?? .backpressured
      handleAppendOutcome(outcome, tracker: systemTracker, isAudio: true)
    }
  }

  func stream(_ stream: SCStream, didStopWithError error: Error) {
    // Task 6: forward SCStream runtime errors via the runtime-error callback.
    let message = "SCStream stopped with error: \(error.localizedDescription)"
    latchRuntimeFailure(trackId: "screen", code: -400, message: message)
  }

  // MARK: AVCapture output

  func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
    stateLock.lock(); let shouldAppend = !paused; stateLock.unlock()
    guard shouldAppend else { return }
    if output is AVCaptureVideoDataOutput {
      let outcome = cameraTracker?.append(sampleBuffer, journal: timestampLog!) ?? .backpressured
      handleAppendOutcome(outcome, tracker: cameraTracker, isAudio: false)
    } else {
      let outcome = micTracker?.append(sampleBuffer, journal: timestampLog!) ?? .backpressured
      handleAppendOutcome(outcome, tracker: micTracker, isAudio: true)
    }
  }

  private func handleAppendOutcome(_ outcome: MediaAppendOutcome, tracker: PerTrackRecorder?, isAudio: Bool) {
    switch outcome {
    case .accepted:
      return
    case .backpressured:
      if isAudio { tracker?.bumpAudioUnderflow() } else { tracker?.bumpDroppedFrame() }
    case let .failed(code, reason):
      // A terminal append failure is distinct from queue backpressure. Latch
      // the first one so stop() cannot report successful completion or commit
      // the failed track's temporary file.
      stateLock.lock()
      let shouldReport = appendFailure == nil
      if shouldReport { appendFailure = encoderResultFailed(code: code, message: reason) }
      stateLock.unlock()
      if shouldReport {
        invokeRuntimeErrorCallback(trackId: tracker?.trackId ?? "", code: code, message: reason)
      }
    }
  }

  // MARK: Pause / resume

  func setPaused(_ value: Bool) {
    stateLock.lock(); paused = value; stateLock.unlock()
    mouseHook?.setPaused(value)
  }

  // MARK: Stop (Task 3 typed outcomes)

  func stop() -> AeroShootEncoderResult {
    // 1. Stop the SCStream (with a 10s timeout) and AVCaptureSession.
    rotationTimer?.cancel(); rotationTimer = nil
    rotationQueue.sync {}
    NotificationCenter.default.removeObserver(self)
    cameraSession?.stopRunning()
    var firstFailure: AeroShootEncoderResult?
    if let error = mouseHook?.stop() {
      firstFailure = encoderResultFailed(code: -700, message: error)
    }
    mouseHook = nil
    if let stream = stream {
      let semaphore = DispatchSemaphore(value: 0)
      var stopError: Error?
      stream.stopCapture { error in stopError = error; semaphore.signal() }
      if semaphore.wait(timeout: .now() + 10) == .timedOut {
        firstFailure = encoderResultTimeout(trackId: "screen")
      } else if let stopError {
        firstFailure = encoderResultFailed(code: Int32((stopError as NSError).code), message: "SCStream stop failed: \(stopError.localizedDescription)")
      }
    }
    // 2. Drain all sample-handler queues.
    screenQueue.sync {}; systemAudioQueue.sync {}; cameraQueue.sync {}; micQueue.sync {}
    // 3. Commit the final segment for every track.
    let tracks: [PerTrackRecorder] = [screenTracker, systemTracker, cameraTracker, micTracker].compactMap { $0 }
    stateLock.lock()
    if firstFailure == nil { firstFailure = appendFailure }
    stateLock.unlock()
    for tracker in tracks {
      let res = tracker.rotateSegment()
      if res.status != AEROSHOOT_ENCODER_OK() && firstFailure == nil {
        firstFailure = res
      }
    }
    // 5. Finalize any in-flight writer that wasn't captured by the rotation
    // above (rotation calls finishWriting internally; finish here is a safety
    // net in case the last append happened after the rotation).
    for tracker in tracks {
      let fin = tracker.finalizeCurrentSegment(timeout: 15.0)
      if !fin.success && firstFailure == nil {
        if fin.status == .failed {
          let code = Int32((fin.error as NSError?)?.code ?? -1)
          firstFailure = encoderResultFailed(code: code, message: fin.error?.localizedDescription ?? "AVAssetWriter failed for \(tracker.trackId)")
        } else {
          firstFailure = encoderResultTimeout(trackId: tracker.trackId)
        }
      }
    }
    // 6. Tear down observers and journal.
    timestampLog?.shutdown()
    timestampLog = nil
    if let failure = firstFailure {
      return failure
    }
    return encoderResultOK()
  }

  // MARK: Stats

  func stats() -> StatsDTO {
    let d = (screenTracker?.droppedFramesValue() ?? 0) + (cameraTracker?.droppedFramesValue() ?? 0)
    let a = (systemTracker?.audioUnderflowsValue() ?? 0) + (micTracker?.audioUnderflowsValue() ?? 0)
    stateLock.lock()
    let error = appendFailure.map { decodeMessage($0) }
    stateLock.unlock()
    let counters = timestampLog?.snapshotCounters() ?? (gapsTotal: 0, droppedRecords: 0)
    return StatsDTO(droppedFrames: d, audioBufferUnderflows: a, timestampRecordsDropped: counters.droppedRecords, gapsTotal: counters.gapsTotal, lastError: error)
  }
}

// MARK: - New C exports

@_cdecl("aeroshoot_register_segment_callback")
public func aeroshootRegisterSegmentCallback(_ cb: AeroShootSegmentCallback?) {
  g_stateLock.lock()
  g_segmentCallback = cb
  g_stateLock.unlock()
}

@_cdecl("aeroshoot_register_runtime_error_callback")
public func aeroshootRegisterRuntimeErrorCallback(_ cb: AeroShootRuntimeErrorCallback?) {
  g_stateLock.lock()
  g_runtimeErrorCallback = cb
  g_stateLock.unlock()
}

@_cdecl("aeroshoot_check_permissions")
public func aeroshootCheckPermissions(_ outBundle: UnsafeMutableRawPointer?) {
  // DEVIATION FROM CONTRACT: the parent brief specified
  //   `AeroShootPermissionBundle aeroshoot_check_permissions(void)`
  // but Swift's @_cdecl cannot return a Swift struct type. The ABI is
  // therefore exposed as:
  //   `void aeroshoot_check_permissions(AeroShootPermissionBundle* out_bundle)`
  // Three packed Int32 fields are written explicitly to match the Rust
  // `#[repr(C)]` layout (Swift struct assignment is not C-ABI-safe).
  writePermissionBundle(currentPermissionBundle(), to: outBundle)
}

@_cdecl("aeroshoot_request_permissions")
public func aeroshootRequestPermissions(_ requestScreen: Bool, _ requestCamera: Bool, _ requestMicrophone: Bool, _ completion: AeroShootPermissionCompletion?) {
  // Report current camera, microphone, and screen states. Screen Recording is
  // requested with CGRequestScreenCaptureAccess (one-shot TCC), never by
  // enumerating shareable content.
  let group = DispatchGroup()
  var screenState = AEROSHOOT_PERMISSION_NOT_DETERMINED()
  var cameraState = permissionState(for: .video)
  var micState = permissionState(for: .audio)
  let lock = NSLock()

  // Screen: never probe ScreenCaptureKit here. Check is silent; an explicit
  // request uses CGRequestScreenCaptureAccess so a packaged app does not
  // re-present the share-content dialog after the user has already granted.
  group.enter()
  if requestScreen {
    DispatchQueue.global(qos: .userInitiated).async {
      let state = requestScreenRecordingAccess()
      lock.lock(); screenState = state; lock.unlock()
      group.leave()
    }
  } else {
    lock.lock(); screenState = peekScreenRecordingState(); lock.unlock()
    group.leave()
  }

  // Camera: only call requestAccess when still notDetermined.
  group.enter()
  if !requestCamera {
    lock.lock(); cameraState = permissionState(for: .video); lock.unlock(); group.leave()
  } else {
    switch AVCaptureDevice.authorizationStatus(for: .video) {
    case .authorized:
      lock.lock(); cameraState = AEROSHOOT_PERMISSION_AUTHORIZED(); lock.unlock(); group.leave()
    case .denied:
      lock.lock(); cameraState = AEROSHOOT_PERMISSION_DENIED(); lock.unlock(); group.leave()
    case .restricted:
      lock.lock(); cameraState = AEROSHOOT_PERMISSION_RESTRICTED(); lock.unlock(); group.leave()
    case .notDetermined:
      DispatchQueue.main.async {
        AVCaptureDevice.requestAccess(for: .video) { granted in
          lock.lock(); cameraState = granted ? AEROSHOOT_PERMISSION_AUTHORIZED() : AEROSHOOT_PERMISSION_DENIED(); lock.unlock()
          group.leave()
        }
      }
    @unknown default:
      lock.lock(); cameraState = AEROSHOOT_PERMISSION_UNKNOWN(); lock.unlock(); group.leave()
    }
  }

  // Microphone: only call requestAccess when still notDetermined.
  group.enter()
  if !requestMicrophone {
    lock.lock(); micState = permissionState(for: .audio); lock.unlock(); group.leave()
  } else {
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .authorized:
      lock.lock(); micState = AEROSHOOT_PERMISSION_AUTHORIZED(); lock.unlock(); group.leave()
    case .denied:
      lock.lock(); micState = AEROSHOOT_PERMISSION_DENIED(); lock.unlock(); group.leave()
    case .restricted:
      lock.lock(); micState = AEROSHOOT_PERMISSION_RESTRICTED(); lock.unlock(); group.leave()
    case .notDetermined:
      DispatchQueue.main.async {
        AVCaptureDevice.requestAccess(for: .audio) { granted in
          lock.lock(); micState = granted ? AEROSHOOT_PERMISSION_AUTHORIZED() : AEROSHOOT_PERMISSION_DENIED(); lock.unlock()
          group.leave()
        }
      }
    @unknown default:
      lock.lock(); micState = AEROSHOOT_PERMISSION_UNKNOWN(); lock.unlock(); group.leave()
    }
  }

  // Fire the completion once all three have settled. The C contract states the
  // completion runs on a background thread; the Rust side marshals to its own
  // async runtime. The C ABI for the completion takes a 3-Int32 homogeneous
  // struct by value, which is ABI-identical to passing three Int32s.
  DispatchQueue.global(qos: .userInitiated).async {
    _ = group.wait(timeout: .now() + 125)
    completion?(screenState, cameraState, micState)
  }
}

@_cdecl("aeroshoot_start")
public func aeroshootStart(_ configJSON: UnsafePointer<CChar>?, _ resultOut: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer? {
  // DEVIATION FROM CONTRACT: start also takes the result as an out-parameter
  // (see aeroshoot_check_permissions above for the reason). The C ABI is:
  //   `void* aeroshoot_start(const char* config_json, AeroShootEncoderResult* out_result)`
  let writeResult: (AeroShootEncoderResult) -> Void = { r in
    guard let raw = resultOut else { return }
    raw.assumingMemoryBound(to: AeroShootEncoderResult.self).pointee = r
  }
  guard let configJSON = configJSON else {
    writeResult(encoderResultFailed(code: -1, message: "null config pointer"))
    return nil
  }
  let config: NativeConfig
  do {
    config = try decodeConfig(configJSON)
  } catch {
    writeResult(encoderResultFailed(code: -2, message: "config decode failed: \(error.localizedDescription)"))
    return nil
  }
  g_stateLock.lock()
  if g_activeRecorder != nil {
    g_stateLock.unlock()
    writeResult(encoderResultFailed(code: -3, message: "an active recording already exists"))
    return nil
  }
  let recorder = ActiveRecorder(config: config)
  let handle = Unmanaged.passRetained(recorder).toOpaque()
  g_activeRecorder = recorder
  g_activeRecorderHandle = handle
  g_stateLock.unlock()

  let startResult = recorder.start()
  if startResult.status != AEROSHOOT_ENCODER_OK() {
    // Roll back the registration so the handle is not dangling.
    g_stateLock.lock()
    g_activeRecorder = nil
    g_activeRecorderHandle = nil
    g_stateLock.unlock()
    Unmanaged<ActiveRecorder>.fromOpaque(handle).release()
    writeResult(startResult)
    return nil
  }
  writeResult(startResult)
  return handle
}

@_cdecl("aeroshoot_pause")
public func aeroshootPause(_ handle: UnsafeMutableRawPointer?) {
  guard let handle = handle else { return }
  g_stateLock.lock()
  guard g_activeRecorderHandle == handle, let recorder = g_activeRecorder else {
    g_stateLock.unlock()
    return
  }
  g_stateLock.unlock()
  recorder.setPaused(true)
}

@_cdecl("aeroshoot_resume")
public func aeroshootResume(_ handle: UnsafeMutableRawPointer?) {
  guard let handle = handle else { return }
  g_stateLock.lock()
  guard g_activeRecorderHandle == handle, let recorder = g_activeRecorder else {
    g_stateLock.unlock()
    return
  }
  g_stateLock.unlock()
  recorder.setPaused(false)
}

@_cdecl("aeroshoot_stop")
public func aeroshootStop(_ outResult: UnsafeMutableRawPointer?) {
  // DEVIATION FROM CONTRACT: see aeroshoot_check_permissions. The C ABI is:
  //   `void aeroshoot_stop(AeroShootEncoderResult* out_result)`
  let writeResult: (AeroShootEncoderResult) -> Void = { r in
    guard let raw = outResult else { return }
    raw.assumingMemoryBound(to: AeroShootEncoderResult.self).pointee = r
  }
  g_stateLock.lock()
  guard let recorder = g_activeRecorder, let handle = g_activeRecorderHandle else {
    g_stateLock.unlock()
    writeResult(encoderResultFailed(code: -10, message: "no active recording"))
    return
  }
  g_activeRecorder = nil
  g_activeRecorderHandle = nil
  g_stateLock.unlock()
  let result = recorder.stop()
  // The Rust side still holds the handle; releasing here would over-release.
  // We do NOT release because the handle's retain count is owned by Rust.
  _ = handle
  writeResult(result)
}

@_cdecl("aeroshoot_copy_stats_json")
public func aeroshootCopyStatsJSON(_ handle: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>? {
  guard let handle = handle else { return copiedCString("{}") }
  g_stateLock.lock()
  guard g_activeRecorderHandle == handle, let recorder = g_activeRecorder else {
    g_stateLock.unlock()
    return copiedCString("{}")
  }
  g_stateLock.unlock()
  return copiedCString(jsonString(recorder.stats()))
}

// MARK: - aeroshoot_macos_* compatibility entrypoints backed by ActiveRecorder

@_cdecl("aeroshoot_macos_start")
public func aeroshootMacOSStart(
  _ configJSON: UnsafePointer<CChar>?,
  _ errorOut: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> UnsafeMutableRawPointer? {
  var result = encoderResultOK()
  let handle = aeroshootStart(configJSON, &result)
  if handle == nil {
    let msg = withUnsafePointer(to: result.error_message) { tuplePtr in
      tuplePtr.withMemoryRebound(to: Int8.self, capacity: 256) { int8Ptr in
        String(cString: int8Ptr)
      }
    }
    errorOut?.pointee = copiedCString(msg.isEmpty ? "capture start failed" : msg)
  }
  return handle
}

@_cdecl("aeroshoot_macos_set_paused")
public func aeroshootMacOSSetPaused(_ handle: UnsafeMutableRawPointer?, _ paused: Bool) {
  if paused {
    aeroshootPause(handle)
  } else {
    aeroshootResume(handle)
  }
}

@_cdecl("aeroshoot_macos_copy_stats_json")
public func aeroshootMacOSCopyStatsJSON(_ handle: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>? {
  aeroshootCopyStatsJSON(handle)
}

@_cdecl("aeroshoot_macos_stop")
public func aeroshootMacOSStop(_ handle: UnsafeMutableRawPointer?) {
  var result = encoderResultOK()
  aeroshootMacOSStopCapture(handle, &result)
}

@_cdecl("aeroshoot_macos_stop_capture")
public func aeroshootMacOSStopCapture(_ handle: UnsafeMutableRawPointer?, _ outResult: UnsafeMutableRawPointer?) {
  let writeResult: (AeroShootEncoderResult) -> Void = { r in
    guard let raw = outResult else { return }
    raw.assumingMemoryBound(to: AeroShootEncoderResult.self).pointee = r
  }
  guard let handle else {
    writeResult(encoderResultFailed(code: -10, message: "null handle"))
    return
  }
  g_stateLock.lock()
  guard g_activeRecorderHandle == handle, let recorder = g_activeRecorder else {
    g_stateLock.unlock()
    writeResult(encoderResultFailed(code: -10, message: "no active recording"))
    return
  }
  g_activeRecorder = nil
  g_activeRecorderHandle = nil
  g_stateLock.unlock()
  let result = recorder.stop()
  Unmanaged<ActiveRecorder>.fromOpaque(handle).release()
  writeResult(result)
}

@_cdecl("aeroshoot_macos_register_segment_callback")
public func aeroshootMacOSRegisterSegmentCallback(_ handle: UnsafeMutableRawPointer?, _ cb: AeroShootSegmentCallback?) {
  aeroshootRegisterSegmentCallback(cb)
}

@_cdecl("aeroshoot_macos_register_runtime_error_callback")
public func aeroshootMacOSRegisterRuntimeErrorCallback(_ handle: UnsafeMutableRawPointer?, _ cb: AeroShootRuntimeErrorCallback?) {
  aeroshootRegisterRuntimeErrorCallback(cb)
}
