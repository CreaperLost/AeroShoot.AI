import AppKit
import Foundation
import CoreGraphics
import CoreMedia
import IOKit.hid

/// Live Input Monitoring state. `CGPreflightListenEventAccess` can report a
/// value cached for the running process, so also read the TCC decision
/// through IOKit, which reflects a grant made in System Settings.
func listenEventAccessGranted() -> Bool {
  IOHIDCheckAccess(kIOHIDRequestTypeListenEvent) == kIOHIDAccessTypeGranted || CGPreflightListenEventAccess()
}

/// Consecutive pointer moves closer together than this are merged, giving a
/// ~60 Hz path: smooth enough for editor cursor smoothing and zoom following,
/// without persisting every raw event from high-rate mice.
private let moveSampleIntervalUs: UInt64 = 16_667

/// One observation of the system cursor. Sizes and the hotspot are in points;
/// the hotspot uses a top-left origin, like `NSCursor.hotSpot`.
struct CursorSample {
  /// Stable 16-hex-digit id of the normalized image plus hotspot.
  let id: String
  /// Standard AppKit shape name (for example `arrow`, `i_beam`), when recognized.
  let name: String?
  let hotspotX: Double
  let hotspotY: Double
  let width: Double
  let height: Double
  /// PNG image, base64-encoded; nil when it cannot be encoded within limits.
  let pngBase64: String?
}

/// Samples the system cursor shape with public AppKit API. The same shape
/// always gets the same id, and standard shapes are named by comparing their
/// silhouette against AppKit's cursors (the system copy is not byte-identical).
enum CursorSampler {
  static let maxAssetBytes = 64_000
  private static let side = 32

  /// Reads the current system cursor. Call on the main thread.
  static func sample() -> CursorSample? {
    guard let cursor = NSCursor.currentSystem else { return nil }
    return describe(cursor)
  }

  static func describe(_ cursor: NSCursor) -> CursorSample? {
    let image = cursor.image
    let size = image.size
    let hotspot = cursor.hotSpot
    guard size.width > 0, size.height > 0, size.width <= 256, size.height <= 256,
      hotspot.x >= 0, hotspot.y >= 0, hotspot.x <= size.width, hotspot.y <= size.height,
      let pixels = normalizedPixels(image) else { return nil }
    var hash = fnv1a(pixels)
    for value in [Double(hotspot.x), Double(hotspot.y)] {
      hash = fnv1a(withUnsafeBytes(of: value.bitPattern) { Array($0) }, seed: hash)
    }
    // Apps hide the pointer (for example while typing) with a fully transparent
    // cursor; name it so the editor draws nothing rather than a blank shape.
    let transparent = !stride(from: 3, to: pixels.count, by: 4).contains { pixels[$0] > 8 }
    return CursorSample(
      id: String(format: "%016llx", hash),
      name: transparent ? "hidden" : standardName(pixels: pixels, hotspot: hotspot, size: size),
      hotspotX: Double(hotspot.x), hotspotY: Double(hotspot.y),
      width: Double(size.width), height: Double(size.height),
      pngBase64: png(image))
  }

  private static func normalizedPixels(_ image: NSImage) -> [UInt8]? {
    var rect = NSRect(origin: .zero, size: image.size)
    guard let cgImage = image.cgImage(forProposedRect: &rect, context: nil, hints: nil) else { return nil }
    var pixels = [UInt8](repeating: 0, count: side * side * 4)
    let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
      guard let context = CGContext(data: buffer.baseAddress, width: side, height: side,
        bitsPerComponent: 8, bytesPerRow: side * 4, space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
      context.interpolationQuality = .medium
      context.draw(cgImage, in: CGRect(x: 0, y: 0, width: side, height: side))
      return true
    }
    return drawn ? pixels : nil
  }

  private static func png(_ image: NSImage) -> String? {
    // Prefer the Retina representation so the editor can draw a crisp cursor.
    var rect = NSRect(x: 0, y: 0, width: image.size.width * 2, height: image.size.height * 2)
    guard let cgImage = image.cgImage(forProposedRect: &rect, context: nil, hints: nil),
      let data = NSBitmapImageRep(cgImage: cgImage).representation(using: .png, properties: [:]),
      data.count <= maxAssetBytes else { return nil }
    return data.base64EncodedString()
  }

  private static func fnv1a(_ bytes: [UInt8], seed: UInt64 = 0xcbf2_9ce4_8422_2325) -> UInt64 {
    var hash = seed
    for byte in bytes {
      hash ^= UInt64(byte)
      hash = hash &* 0x0000_0100_0000_01b3
    }
    return hash
  }

  private static let standards: [(name: String, pixels: [UInt8], hotspot: NSPoint, size: NSSize)] = {
    var named: [(String, NSCursor)] = []
    // macOS 15 cursors come first so an identical legacy image resolves to the
    // modern name. Two-way frame resizes look the same from opposite edges, so
    // they are named by axis rather than by edge.
    if #available(macOS 15.0, *) {
      named += [
        ("column_resize", .columnResize), ("row_resize", .rowResize),
        ("column_resize_left", .columnResize(directions: .left)),
        ("column_resize_right", .columnResize(directions: .right)),
        ("row_resize_up", .rowResize(directions: .up)), ("row_resize_down", .rowResize(directions: .down)),
        ("zoom_in", .zoomIn), ("zoom_out", .zoomOut),
        ("frame_resize_vertical", .frameResize(position: .top, directions: .all)),
        ("frame_resize_horizontal", .frameResize(position: .left, directions: .all)),
        ("frame_resize_diagonal_nwse", .frameResize(position: .topLeft, directions: .all)),
        ("frame_resize_diagonal_nesw", .frameResize(position: .topRight, directions: .all)),
      ]
    }
    named += [
      ("arrow", .arrow), ("i_beam", .iBeam), ("i_beam_vertical", .iBeamCursorForVerticalLayout),
      ("pointing_hand", .pointingHand), ("open_hand", .openHand), ("closed_hand", .closedHand),
      ("crosshair", .crosshair), ("resize_left_right", .resizeLeftRight), ("resize_up_down", .resizeUpDown),
      ("operation_not_allowed", .operationNotAllowed), ("drag_copy", .dragCopy), ("drag_link", .dragLink),
      ("contextual_menu", .contextualMenu), ("disappearing_item", .disappearingItem),
    ]
    return named.compactMap { name, cursor in
      normalizedPixels(cursor.image).map { (name, $0, cursor.hotSpot, cursor.image.size) }
    }
  }()

  /// Closest standard cursor by silhouette (alpha) and relative hotspot.
  private static func standardName(pixels: [UInt8], hotspot: NSPoint, size: NSSize) -> String? {
    var best: (name: String, score: Double)?
    for standard in standards {
      guard abs(standard.size.width / standard.size.height - size.width / size.height) < 0.1 else { continue }
      var difference = 0
      for index in stride(from: 3, to: pixels.count, by: 4) {
        difference += abs(Int(pixels[index]) - Int(standard.pixels[index]))
      }
      let score = Double(difference) / Double(side * side) / 255
      // The system copy of a standard cursor can use a different hotspot than
      // AppKit's (the I-beam does), so an essentially identical silhouette is
      // enough; looser matches must also agree on the relative hotspot.
      let hotspotAgrees = abs(standard.hotspot.x / standard.size.width - hotspot.x / size.width) < 0.1
        && abs(standard.hotspot.y / standard.size.height - hotspot.y / size.height) < 0.1
      guard score < 0.02 || (score < 0.08 && hotspotAgrees) else { continue }
      if score < (best?.score ?? .infinity) { best = (standard.name, score) }
    }
    return best?.name
  }
}

// Borrowed UTF-8 JSON; Rust owns append-only persistence. Runs on the drain
// worker, never on the event tap. Nonzero means persistence failed.
typealias MouseTelemetrySink = @convention(c) (UnsafePointer<CChar>?) -> Int32
private let mouseSinkLock = NSLock()
private var mouseSink: MouseTelemetrySink?
@_cdecl("aeroshoot_macos_register_mouse_sink")
func registerMouseSink(_ sink: MouseTelemetrySink?) {
  mouseSinkLock.lock(); mouseSink = sink; mouseSinkLock.unlock()
}

final class MouseHookMac {
  private let lock = NSLock()
  private let worker = DispatchQueue(label: "ai.aeroshoot.mouse.persistence", qos: .utility)
  private let exited = DispatchGroup()
  private var timer: DispatchSourceTimer?
  private var tap: CFMachPort?
  private var loop: CFRunLoop?
  private var stopping = false
  private var paused = false
  private var pauseStart: UInt64?
  private var records: [[String: Any]] = []
  private var lost: (start: UInt64, end: UInt64, count: UInt64)?
  private var geometry: CGRect?
  private var geometryID: String?
  private var geometryRevision = 0
  private var displayMetrics: [Double] = []
  private var sequence: UInt64 = 0
  private var failure: String?
  private let sourceID: String
  private let outputWidth: Int
  private let outputHeight: Int
  private let epochUs: Int64
  private let offsetUs: UInt64
  private var permissionLost = false
  private var userDisabled = false
  /// `replace` when the cursor is hidden from the video, otherwise `baked`.
  private let cursorMode: String
  private var trackingLostReported = false
  /// Called once, off the event tap, when pointer data stops being recorded.
  var onTrackingLost: ((String) -> Void)?
  private var lastCursorId: String?
  private var cursorAssetIds = Set<String>()
  private var cursorPollPending = false
  /// Distinct cursor images stored per recording.
  static let maxCursorAssets = 64
  private let capacity = 2048
  /// Live capture always uses a 100 ms geometry poll. Do not tighten this
  /// without a measured adapter; H2 keeps the interval as uncertainty.
  static let geometrySamplingIntervalUs: UInt64 = 100_000
#if MOUSE_CONTRACT_TESTS
  var listenAccessOverride: Bool?
#endif

  init(sourceID: String, width: Int, height: Int, epoch: CMTime, offsetUs: UInt64, cursorMode: String = "baked") {
    self.sourceID = sourceID; outputWidth = width; outputHeight = height
    self.cursorMode = cursorMode
    epochUs = CMTimeConvertScale(epoch, timescale: 1_000_000, method: .roundTowardZero).value
    self.offsetUs = offsetUs
  }

  // CGEvent timestamps are host uptime nanoseconds. Use the same CoreMedia
  // host epoch as the media recorder, not callback arrival or drain time.
  func sessionTime(_ nanoseconds: UInt64) -> UInt64 {
    let delta = Int64(clamping: nanoseconds / 1000) - epochUs
    return UInt64(max(0, Int64(clamping: offsetUs) + delta))
  }
  private func now() -> UInt64 {
    let host = CMClockGetTime(CMClockGetHostTimeClock())
    let us = CMTimeConvertScale(host, timescale: 1_000_000, method: .roundTowardZero).value
    return UInt64(max(0, Int64(clamping: offsetUs) + us - epochUs))
  }

  private func hasListenAccess() -> Bool {
#if MOUSE_CONTRACT_TESTS
    if let override = listenAccessOverride { return override }
#endif
    return listenEventAccessGranted()
  }

  private func gap(_ reason: String, _ start: UInt64, _ end: UInt64, count: UInt64 = 0) -> [String: Any] {
    return ["record": "event", "version": 2, "t_us": end,
      "payload": ["kind": "gap", "reason": reason, "start_us": start,
        "end_us": end, "dropped_events": count]]
  }

  // Called with lock held. Reserve capacity for an overflow marker. If any
  // event is lost, held-button state is unknown until a future transition.
  private func enqueue(_ record: [String: Any], at time: UInt64) {
    if let payload = record["payload"] as? [String: Any], payload["kind"] as? String == "gap",
       let reason = payload["reason"] as? String, MouseHookMac.trackingLostReasons.contains(reason) {
      reportTrackingLost(reason)
    }
    if records.count >= capacity - 1 {
      if let previous = lost { lost = (previous.start, time, previous.count + 1) }
      else { lost = (time, time, 1) }
      return
    }
    if let lost {
      records.append(gap("queue_overflow", lost.start, lost.end, count: lost.count))
      self.lost = nil
    }
    records.append(record)
  }

  /// Samples the cursor shape on the main thread (AppKit), then records any
  /// change on the persistence worker. At most one sample is in flight.
  private func pollCursor() {
    lock.lock()
    let skip = cursorPollPending || stopping || paused || userDisabled || geometryID == nil
    if !skip { cursorPollPending = true }
    lock.unlock()
    guard !skip else { return }
    DispatchQueue.main.async { [weak self] in
      let sample = CursorSampler.sample()
      self?.worker.async { self?.recordCursor(sample) }
    }
  }

  /// Records a `cursor_changed` event when the shape differs from the last one,
  /// preceded by the shape's image the first time it is seen.
  func recordCursor(_ sample: CursorSample?) {
    lock.lock(); defer { lock.unlock() }
    cursorPollPending = false
    guard let sample, !stopping, !paused, sample.id != lastCursorId, let geometryID else { return }
    lastCursorId = sample.id
    let time = now()
    if !cursorAssetIds.contains(sample.id), cursorAssetIds.count < MouseHookMac.maxCursorAssets,
       let png = sample.pngBase64 {
      cursorAssetIds.insert(sample.id)
      enqueue(["record": "cursor_asset", "version": 2, "cursor_id": sample.id, "png_base64": png,
        "width": sample.width, "height": sample.height], at: time)
    }
    var payload: [String: Any] = ["kind": "cursor_changed", "cursor_id": sample.id,
      "hotspot_x": sample.hotspotX, "hotspot_y": sample.hotspotY,
      "width": sample.width, "height": sample.height]
    if let name = sample.name { payload["name"] = name }
    enqueue(["record": "event", "version": 2, "t_us": time, "geometry_id": geometryID,
      "payload": payload], at: time)
  }

  /// Gaps after which pointer data is no longer being recorded.
  static let trackingLostReasons: Set<String> = [
    "input_monitoring_unavailable", "input_monitoring_revoked", "event_tap_unavailable",
    "event_tap_disabled", "unsupported_source_geometry",
  ]

  // Called with lock held. Notifies at most once per hook.
  private func reportTrackingLost(_ reason: String) {
    guard !trackingLostReported else { return }
    trackingLostReported = true
    guard let callback = onTrackingLost else { return }
    DispatchQueue.global(qos: .userInitiated).async { callback(reason) }
  }

  /// Log that the cursor is baked into the video again from this moment on.
  func markCursorShownInVideo() {
    lock.lock(); defer { lock.unlock() }
    guard !stopping else { return }
    let time = now()
    enqueue(gap("cursor_shown_in_video", time, time), at: time)
  }

  func start() {
    refreshGeometry()
    lock.lock()
    enqueue(gap("initial_button_state_unknown", now(), now()), at: now())
    lock.unlock()
    guard hasListenAccess() else {
      lock.lock(); enqueue(gap("input_monitoring_unavailable", now(), now()), at: now()); lock.unlock()
      drain(); return
    }
    guard geometryID != nil else {
      lock.lock(); enqueue(gap("unsupported_source_geometry", now(), now()), at: now()); lock.unlock()
      drain(); return
    }
    let timer = DispatchSource.makeTimerSource(queue: worker)
    timer.schedule(deadline: .now(), repeating: .milliseconds(100))
    timer.setEventHandler { [weak self] in
      self?.refreshGeometry(); self?.pollCursor(); self?.drain()
    }
    self.timer = timer; timer.resume()
    exited.enter()
    Thread.detachNewThread { [self] in
      defer { exited.leave() }
      let types: [CGEventType] = [.mouseMoved, .leftMouseDragged, .rightMouseDragged,
        .otherMouseDragged, .leftMouseDown, .leftMouseUp, .rightMouseDown,
        .rightMouseUp, .otherMouseDown, .otherMouseUp, .scrollWheel]
      let mask = types.reduce(CGEventMask(0)) { $0 | (CGEventMask(1) << $1.rawValue) }
      let port = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap,
        options: .listenOnly, eventsOfInterest: mask, callback: { _, type, event, context in
          if let context {
            Unmanaged<MouseHookMac>.fromOpaque(context).takeUnretainedValue().receive(type, event)
          }
          return Unmanaged.passUnretained(event)
        }, userInfo: Unmanaged.passUnretained(self).toOpaque())
      guard let port, let source = CFMachPortCreateRunLoopSource(nil, port, 0) else {
        lock.lock(); enqueue(gap("event_tap_unavailable", now(), now()), at: now()); lock.unlock()
        return
      }
      let current = CFRunLoopGetCurrent()!
      CFRunLoopAddSource(current, source, .commonModes)
      lock.lock(); tap = port; loop = current; let stopNow = stopping; lock.unlock()
      if !stopNow {
        while true {
          lock.lock(); let done = stopping; lock.unlock()
          if done { break }
          CFRunLoopRunInMode(.defaultMode, 0.1, false)
        }
      }
      CGEvent.tapEnable(tap: port, enable: false)
      CFMachPortInvalidate(port)
      CFRunLoopRemoveSource(current, source, .commonModes)
      lock.lock(); tap = nil; loop = nil; lock.unlock()
    }
  }

  private func receive(_ type: CGEventType, _ event: CGEvent) {
    // Tap-disabled notifications carry no event timestamp; use the session clock.
    let disabled = type == .tapDisabledByTimeout || type == .tapDisabledByUserInput
    let time = disabled ? now() : sessionTime(event.timestamp)
    lock.lock(); defer { lock.unlock() }
    guard !stopping, failure == nil else { return }
    if type == .tapDisabledByUserInput {
      enqueue(gap("event_tap_disabled", time, time), at: time)
      // Explicit user disable: record the gap, do not re-enable or prompt.
      userDisabled = true
      permissionLost = false
      return
    }
    if type == .tapDisabledByTimeout {
      // Transient: the worker re-enables the tap within 100 ms, so pointer data
      // resumes and this must not end cursor replacement. If Input Monitoring is
      // really gone, the re-enable path records `input_monitoring_revoked`.
      enqueue(gap("event_tap_timeout", time, time), at: time)
      // Authorization is rechecked on the worker, not inside the tap callback.
      permissionLost = true
      return
    }
    guard !userDisabled, !paused, let bounds = geometry, let id = geometryID else { return }
    let point = event.location
    let x = (point.x - bounds.minX) / bounds.width
    let y = (point.y - bounds.minY) / bounds.height
    var payload: [String: Any]
    switch type {
    case .leftMouseDown, .rightMouseDown, .otherMouseDown:
      payload = ["kind": "button_down", "button": event.getIntegerValueField(.mouseEventButtonNumber)]
    case .leftMouseUp, .rightMouseUp, .otherMouseUp:
      payload = ["kind": "button_up", "button": event.getIntegerValueField(.mouseEventButtonNumber)]
    case .scrollWheel:
      let precise = event.getIntegerValueField(.scrollWheelEventIsContinuous) != 0
      payload = ["kind": "scroll", "delta_x": event.getDoubleValueField(precise ? .scrollWheelEventPointDeltaAxis2 : .scrollWheelEventDeltaAxis2),
        "delta_y": event.getDoubleValueField(precise ? .scrollWheelEventPointDeltaAxis1 : .scrollWheelEventDeltaAxis1),
        "units": precise ? "pixels" : "lines", "precise": precise,
        "phase": event.getIntegerValueField(.scrollWheelEventScrollPhase),
        "momentum_phase": event.getIntegerValueField(.scrollWheelEventMomentumPhase)]
    default: payload = ["kind": "move"]
    }
    let record: [String: Any] = ["record": "event", "version": 2, "t_us": time,
      "geometry_id": id, "norm_x": x, "norm_y": y,
      "inside_source": x >= 0 && x < 1 && y >= 0 && y < 1,
      "payload": payload]
    // Replace only an immediately preceding movement within the sampling
    // interval. Button/scroll ordering is preserved; movement while dwelling
    // remains derivable from timestamps.
    if payload["kind"] as? String == "move", let last = records.last,
       let prior = last["payload"] as? [String: Any], prior["kind"] as? String == "move",
       last["geometry_id"] as? String == id, lost == nil,
       let priorTime = last["t_us"] as? UInt64, time >= priorTime, time - priorTime < moveSampleIntervalUs {
      records[records.count - 1] = record
    } else { enqueue(record, at: time) }
  }

  private func refreshGeometry() {
    var bounds: CGRect?
    var metrics: [Double] = []
    if sourceID.hasPrefix("display:"), let id = UInt32(sourceID.dropFirst(8)), CGDisplayIsActive(id) != 0 {
      bounds = CGDisplayBounds(id)
      metrics = [Double(CGDisplayPixelsWide(id)), Double(CGDisplayPixelsHigh(id)), CGDisplayRotation(id)]
    } else if sourceID.hasPrefix("window:"), let id = UInt32(sourceID.dropFirst(7)),
      let windows = CGWindowListCopyWindowInfo(.optionIncludingWindow, id) as? [[String: Any]],
      let window = windows.first, let dictionary = window[kCGWindowBounds as String] as? [String: Any] {
      bounds = CGRect(dictionaryRepresentation: dictionary as CFDictionary)
    }
    if let rect = bounds, rect.width <= 0 || rect.height <= 0 { bounds = nil }
    lock.lock(); defer { lock.unlock() }
    let time = now()
    if permissionLost {
      permissionLost = false
      if hasListenAccess() {
        if let tap { CGEvent.tapEnable(tap: tap, enable: true) }
      } else {
        enqueue(gap("input_monitoring_revoked", time, time), at: time)
      }
    }
    guard bounds != geometry || metrics != displayMetrics else { return }
    displayMetrics = metrics
    // A geometry change is a discontinuity at polling resolution. Do not
    // imply frame-exact window tracking between these 100 ms observations.
    let uncertainty = MouseHookMac.geometrySamplingIntervalUs
    enqueue(gap("geometry_changed", time > uncertainty ? time - uncertainty : 0, time), at: time)
    geometry = bounds
    geometryID = nil
    guard let bounds else {
      // The source vanished (display unplugged, window closed): no more pointer data.
      reportTrackingLost("geometry_unavailable")
      return
    }
    // Don't reference a geometry revision that could not enter the queue.
    guard records.count < capacity - 2 else { geometry = nil; return }
    geometryRevision += 1
    let id = "mouse-g\(geometryRevision)"
    geometryID = id
    var revision: [String: Any] = ["record": "geometry", "version": 2, "geometry_id": id, "t_us": time,
      "coordinate_space": "quartz_global", "source_id": sourceID,
      "bounds": ["x": bounds.minX, "y": bounds.minY, "width": bounds.width, "height": bounds.height],
      "output_width": outputWidth, "output_height": outputHeight,
      "sampling_interval_us": MouseHookMac.geometrySamplingIntervalUs, "cursor_mode": cursorMode]
    if metrics.count == 3 {
      revision["physical_width"] = metrics[0]
      revision["physical_height"] = metrics[1]
      revision["rotation_degrees"] = metrics[2]
      revision["logical_to_physical_scale_x"] = metrics[0] / bounds.width
      revision["logical_to_physical_scale_y"] = metrics[1] / bounds.height
    }
    enqueue(revision, at: time)
  }

  private func drain() {
    lock.lock()
    var batch = records; records.removeAll(keepingCapacity: true)
    if let lost { batch.append(gap("queue_overflow", lost.start, lost.end, count: lost.count)); self.lost = nil }
    let failed = failure != nil
    lock.unlock()
    guard !failed else { return }
    mouseSinkLock.lock(); let sink = mouseSink; mouseSinkLock.unlock()
    for var record in batch {
      if record["record"] as? String == "event" { record["seq"] = sequence; sequence += 1 }
      guard let sink, let data = try? JSONSerialization.data(withJSONObject: record),
        let json = String(data: data, encoding: .utf8), json.withCString({ sink($0) }) == 0 else {
        lock.lock(); failure = "Mouse telemetry persistence failed"; reportTrackingLost("persistence_failed"); lock.unlock(); return
      }
    }
    if let sink, "{\"record\":\"flush\"}".withCString({ sink($0) }) != 0 {
      lock.lock(); failure = "Mouse telemetry flush failed"; reportTrackingLost("persistence_failed"); lock.unlock()
    }
  }

  func setPaused(_ value: Bool) {
    lock.lock(); defer { lock.unlock() }
    let time = now()
    if value && !paused { pauseStart = time }
    if !value, let start = pauseStart {
      enqueue(gap("recording_paused", start, time), at: time); pauseStart = nil
    }
    paused = value
  }

  func stop() -> String? {
    lock.lock(); stopping = true; let runLoop = loop
    if let start = pauseStart { enqueue(gap("recording_paused", start, now()), at: now()); pauseStart = nil }
    lock.unlock()
    if let runLoop { CFRunLoopStop(runLoop) }
    exited.wait()
    if let timer {
      timer.setEventHandler {}
      timer.cancel()
    }
    self.timer = nil
    worker.sync { drain() }
    lock.lock(); defer { lock.unlock() }
    return failure
  }
}

// This API is called only by the explicit UI permission action. Startup uses
// preflight, so denying telemetry never creates a repeated prompt loop.
@_cdecl("aeroshoot_macos_mouse_permission")
func mousePermission(_ request: Bool) -> Bool {
  if request && !listenEventAccessGranted() {
    return IOHIDRequestAccess(kIOHIDRequestTypeListenEvent) || CGRequestListenEventAccess()
  }
  return listenEventAccessGranted()
}

#if MOUSE_CONTRACT_TESTS
private let capturedLock = NSLock()
private var capturedJSON: [String] = []
private func capturingSink(_ ptr: UnsafePointer<CChar>?) -> Int32 {
  guard let ptr else { return -1 }
  capturedLock.lock(); capturedJSON.append(String(cString: ptr)); capturedLock.unlock()
  return 0
}

extension MouseHookMac {
  private func gapReasons() -> [String] {
    records.compactMap { record in
      (record["payload"] as? [String: Any])?["reason"] as? String
    }
  }

  private func eventKinds() -> [String] {
    records.compactMap { record in
      (record["payload"] as? [String: Any])?["kind"] as? String
    }
  }

  static func runContractTests() {
    testSessionClockAndUnclampedGeometry()
    testOverflowEmitsGapNotFabricatedClicks()
    testPauseUserDisableTimeoutAndStop()
    testDeniedAndRevokedPermissionGaps()
    testUnsupportedApplicationGeometry()
    testGeometryUncertaintyInterval()
    testNoCallbacksAfterStop()
    testTrackingLostRestoresCursorOnce()
    testCursorShapesRecordedOncePerShape()
    testTapTimeoutIsTransient()
    print("Mouse telemetry contracts passed (no event tap installed)")
  }

  private static func testTapTimeoutIsTransient() {
    let hook = makeHook()
    hook.geometry = CGRect(x: 0, y: 0, width: 1920, height: 1080)
    hook.geometryID = "g1"
    let lostLock = NSLock()
    var lost = 0
    hook.onTrackingLost = { _ in lostLock.lock(); lost += 1; lostLock.unlock() }
    hook.receive(.tapDisabledByTimeout, makeEvent())
    assert(hook.gapReasons() == ["event_tap_timeout"])
    assert(hook.permissionLost && !hook.trackingLostReported)
    assert((hook.records[0]["t_us"] as? UInt64 ?? 0) > 0)
    Thread.sleep(forTimeInterval: 0.05)
    lostLock.lock(); assert(lost == 0); lostLock.unlock()
    // A deliberate user disable does stop pointer data.
    hook.receive(.tapDisabledByUserInput, makeEvent())
    assert(hook.trackingLostReported)
  }

  private static func testCursorShapesRecordedOncePerShape() {
    // AppKit loads standard cursor images only once an application exists;
    // the recorder always has one, a command-line test does not.
    _ = NSApplication.shared
    guard let arrow = CursorSampler.describe(NSCursor.arrow),
      let again = CursorSampler.describe(NSCursor.arrow),
      let beam = CursorSampler.describe(NSCursor.iBeam) else {
      fatalError("standard cursors must be describable")
    }
    assert(arrow.id.count == 16 && arrow.id == again.id)
    assert(arrow.name == "arrow" && beam.name == "i_beam")
    assert(beam.id != arrow.id)
    assert(arrow.pngBase64 != nil && Data(base64Encoded: arrow.pngBase64!)!.starts(with: [0x89, 0x50, 0x4E, 0x47]))
    assert(arrow.hotspotX <= arrow.width && arrow.hotspotY <= arrow.height)
    // The system I-beam uses AppKit's image with a different hotspot.
    let systemBeam = NSCursor(image: NSCursor.iBeam.image, hotSpot: NSPoint(x: 11.5, y: 11))
    assert(CursorSampler.describe(systemBeam)?.name == "i_beam")
    // A fully transparent cursor means the app hid the pointer.
    let blank = NSImage(size: NSSize(width: 16, height: 16), flipped: false) { _ in true }
    assert(CursorSampler.describe(NSCursor(image: blank, hotSpot: .zero))?.name == "hidden")
    if #available(macOS 15.0, *) {
      assert(CursorSampler.describe(.rowResize)?.name == "row_resize")
      assert(CursorSampler.describe(.frameResize(position: .bottomRight, directions: .all))?.name
        == "frame_resize_diagonal_nwse")
    }

    let hook = makeHook()
    hook.geometry = CGRect(x: 0, y: 0, width: 1920, height: 1080)
    hook.geometryID = "g1"
    hook.recordCursor(arrow)
    hook.recordCursor(again)   // unchanged shape: nothing new
    hook.recordCursor(beam)
    hook.recordCursor(arrow)   // image already stored: event only
    let assets = hook.records.filter { $0["record"] as? String == "cursor_asset" }
    assert(assets.count == 2)
    assert(hook.eventKinds().filter { $0 == "cursor_changed" }.count == 3)
    assert(hook.records.allSatisfy { JSONSerialization.isValidJSONObject($0) })
    hook.setPaused(true)
    hook.recordCursor(beam)    // paused: shape changes are not recorded
    assert(hook.eventKinds().filter { $0 == "cursor_changed" }.count == 3)
  }

  private static func testTrackingLostRestoresCursorOnce() {
    capturedLock.lock(); capturedJSON.removeAll(); capturedLock.unlock()
    registerMouseSink(capturingSink)
    let hook = MouseHookMac(sourceID: "display:\(CGMainDisplayID())", width: 1920, height: 1080,
      epoch: CMTime(value: 1, timescale: 1), offsetUs: 100, cursorMode: "replace")
    let reasonsLock = NSLock()
    var reasons: [String] = []
    let fired = DispatchSemaphore(value: 0)
    hook.onTrackingLost = { reason in
      reasonsLock.lock(); reasons.append(reason); reasonsLock.unlock()
      fired.signal()
    }
    hook.listenAccessOverride = false
    hook.start()
    assert(fired.wait(timeout: .now() + 2) == .success)
    // A later loss must not ask the recorder to restore the cursor again.
    hook.permissionLost = true
    hook.refreshGeometry()
    Thread.sleep(forTimeInterval: 0.1)
    reasonsLock.lock(); let observed = reasons; reasonsLock.unlock()
    assert(observed == ["input_monitoring_unavailable"])
    hook.markCursorShownInVideo()
    assert(hook.gapReasons().contains("cursor_shown_in_video"))
    capturedLock.lock(); let flushed = capturedJSON; capturedLock.unlock()
    assert(flushed.contains { $0.contains("\"cursor_mode\":\"replace\"") })
    _ = hook.stop()
  }

  private static func makeHook(sourceID: String = "display:0") -> MouseHookMac {
    MouseHookMac(sourceID: sourceID, width: 1920, height: 1080,
      epoch: CMTime(value: 1, timescale: 1), offsetUs: 100)
  }

  private static func makeEvent() -> CGEvent {
    let event = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved,
      mouseCursorPosition: CGPoint(x: -2400, y: 1520), mouseButton: .left)!
    event.timestamp = 1_500_000_000
    return event
  }

  private static func testSessionClockAndUnclampedGeometry() {
    let hook = makeHook()
    assert(hook.sessionTime(1_500_000_000) == 500_100)
    hook.geometry = CGRect(x: -1920, y: -100, width: 1920, height: 1080)
    hook.geometryID = "g1"
    let event = makeEvent()
    hook.receive(.mouseMoved, event)
    assert(hook.records[0]["t_us"] as? UInt64 == 500_100)
    assert(hook.records[0]["norm_x"] as? CGFloat == -0.25)
    assert(hook.records[0]["norm_y"] as? CGFloat == 1.5)
    assert(hook.records[0]["inside_source"] as? Bool == false)
    assert(JSONSerialization.isValidJSONObject(hook.records[0]))
    for _ in 0..<10_000 { hook.receive(.mouseMoved, event) }
    assert(hook.records.count == 1)
    event.setIntegerValueField(.mouseEventButtonNumber, value: 4)
    hook.receive(.otherMouseDown, event)
    hook.receive(.otherMouseUp, event)
    let down = hook.records[1]["payload"] as! [String: Any]
    assert(down["kind"] as? String == "button_down" && down["button"] as? Int64 == 4)
  }

  private static func testOverflowEmitsGapNotFabricatedClicks() {
    let hook = makeHook()
    hook.geometry = CGRect(x: 0, y: 0, width: 1920, height: 1080)
    hook.geometryID = "g1"
    let event = makeEvent()
    for _ in 0..<10_000 { hook.receive(.leftMouseDown, event) }
    assert(hook.records.count < hook.capacity)
    assert(hook.lost != nil)
    capturedLock.lock(); capturedJSON.removeAll(); capturedLock.unlock()
    registerMouseSink(capturingSink)
    hook.drain()
    capturedLock.lock(); let flushed = capturedJSON; capturedLock.unlock()
    assert(flushed.contains { $0.contains("\"queue_overflow\"") })
    assert(!flushed.contains { $0.contains("\"kind\":\"click\"") })
    assert(hook.records.isEmpty)
    assert(hook.lost == nil)
  }

  private static func testPauseUserDisableTimeoutAndStop() {
    let hook = makeHook()
    hook.geometry = CGRect(x: 0, y: 0, width: 1920, height: 1080)
    hook.geometryID = "g1"
    let event = makeEvent()
    hook.setPaused(true)
    hook.receive(.mouseMoved, event)
    assert(hook.records.isEmpty)
    hook.setPaused(false)
    assert((hook.records[0]["payload"] as? [String: Any])?["reason"] as? String == "recording_paused")
    hook.receive(.tapDisabledByUserInput, event)
    assert(!hook.permissionLost)
    assert(hook.userDisabled)
    let afterDisable = hook.records.count
    hook.receive(.leftMouseDown, event)
    assert(hook.records.count == afterDisable)
    hook.receive(.tapDisabledByTimeout, event)
    assert(hook.permissionLost)
    registerMouseSink(nil)
    hook.drain()
    assert(hook.stop() != nil)
  }

  private static func testDeniedAndRevokedPermissionGaps() {
    capturedLock.lock(); capturedJSON.removeAll(); capturedLock.unlock()
    registerMouseSink(capturingSink)
    let denied = makeHook()
    denied.listenAccessOverride = false
    denied.start()
    assert(denied.tap == nil)
    capturedLock.lock(); let flushed = capturedJSON; capturedLock.unlock()
    assert(flushed.contains { $0.contains("input_monitoring_unavailable") })
    assert(!flushed.contains { $0.contains("\"kind\":\"button_down\"") })
    assert(!flushed.contains { $0.contains("\"kind\":\"click\"") })
    _ = denied.stop()

    let revoked = makeHook()
    revoked.listenAccessOverride = false
    revoked.permissionLost = true
    revoked.refreshGeometry()
    assert(revoked.gapReasons().contains("input_monitoring_revoked"))
    assert(!revoked.eventKinds().contains("button_down"))
    let authorized = makeHook()
    authorized.listenAccessOverride = true
    authorized.permissionLost = true
    authorized.refreshGeometry()
    assert(!authorized.gapReasons().contains("input_monitoring_revoked"))
  }

  private static func testUnsupportedApplicationGeometry() {
    capturedLock.lock(); capturedJSON.removeAll(); capturedLock.unlock()
    registerMouseSink(capturingSink)
    let hook = makeHook(sourceID: "application:com.example")
    hook.listenAccessOverride = true
    hook.start()
    assert(hook.tap == nil)
    capturedLock.lock(); let flushed = capturedJSON; capturedLock.unlock()
    assert(flushed.contains { $0.contains("unsupported_source_geometry") })
    assert(!flushed.contains { $0.contains("\"kind\":\"button_down\"") })
    _ = hook.stop()
  }

  private static func testGeometryUncertaintyInterval() {
    let hook = makeHook()
    hook.geometry = CGRect(x: -1920, y: -100, width: 1920, height: 1080)
    hook.refreshGeometry()
    let changed = hook.records.first { record in
      (record["payload"] as? [String: Any])?["reason"] as? String == "geometry_changed"
    }
    assert(changed != nil)
    let payload = changed?["payload"] as! [String: Any]
    let start = payload["start_us"] as! UInt64
    let end = payload["end_us"] as! UInt64
    assert(end >= start)
    if end >= MouseHookMac.geometrySamplingIntervalUs {
      assert(end - start == MouseHookMac.geometrySamplingIntervalUs)
    }
  }

  private static func testNoCallbacksAfterStop() {
    let hook = makeHook()
    hook.geometry = CGRect(x: 0, y: 0, width: 1920, height: 1080)
    hook.geometryID = "g1"
    let event = makeEvent()
    capturedLock.lock(); capturedJSON.removeAll(); capturedLock.unlock()
    registerMouseSink(capturingSink)
    hook.receive(.leftMouseDown, event)
    assert(hook.stop() == nil)
    capturedLock.lock(); let afterStop = capturedJSON.count; capturedLock.unlock()
    hook.receive(.leftMouseDown, event)
    hook.receive(.mouseMoved, event)
    hook.drain()
    capturedLock.lock(); let later = capturedJSON; capturedLock.unlock()
    assert(hook.records.isEmpty)
    assert(later.filter { $0.contains("button_down") }.count
      == later.prefix(afterStop).filter { $0.contains("button_down") }.count)
  }
}
#endif
