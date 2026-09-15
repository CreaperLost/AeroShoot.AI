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
  private let capacity = 2048
  /// Live capture always uses a 100 ms geometry poll. Do not tighten this
  /// without a measured adapter; H2 keeps the interval as uncertainty.
  static let geometrySamplingIntervalUs: UInt64 = 100_000
#if MOUSE_CONTRACT_TESTS
  var listenAccessOverride: Bool?
#endif

  init(sourceID: String, width: Int, height: Int, epoch: CMTime, offsetUs: UInt64) {
    self.sourceID = sourceID; outputWidth = width; outputHeight = height
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
      self?.refreshGeometry(); self?.drain()
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
    let time = sessionTime(event.timestamp)
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
      enqueue(gap("event_tap_disabled", time, time), at: time)
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
    guard let bounds else { return }
    // Don't reference a geometry revision that could not enter the queue.
    guard records.count < capacity - 2 else { geometry = nil; return }
    geometryRevision += 1
    let id = "mouse-g\(geometryRevision)"
    geometryID = id
    var revision: [String: Any] = ["record": "geometry", "version": 2, "geometry_id": id, "t_us": time,
      "coordinate_space": "quartz_global", "source_id": sourceID,
      "bounds": ["x": bounds.minX, "y": bounds.minY, "width": bounds.width, "height": bounds.height],
      "output_width": outputWidth, "output_height": outputHeight,
      "sampling_interval_us": MouseHookMac.geometrySamplingIntervalUs, "cursor_mode": "baked"]
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
        lock.lock(); failure = "Mouse telemetry persistence failed"; lock.unlock(); return
      }
    }
    if let sink, "{\"record\":\"flush\"}".withCString({ sink($0) }) != 0 {
      lock.lock(); failure = "Mouse telemetry flush failed"; lock.unlock()
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
    print("Mouse telemetry contracts passed (no event tap installed)")
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
