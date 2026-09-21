import AVFoundation
import ScreenCaptureKit
import CoreImage
import CoreVideo
import AppKit
import AudioToolbox
import Metal

// Capture callbacks retain only the newest pixel buffer per video source.
// Recording and idle monitoring use the same mailbox; no frames go through JS.
enum LivePreviewFrames {
  static let lock = NSLock()
  static var screen: CVPixelBuffer?
  static var camera: CVPixelBuffer?
  static var showScreen = true
  /// Bumped on every change so the renderer can skip unchanged frames.
  static var sequence: UInt64 = 0
  static func offer(_ sample: CMSampleBuffer, camera isCamera: Bool) {
    guard let pixel = CMSampleBufferGetImageBuffer(sample) else { return }
    lock.lock(); defer { lock.unlock() }
    if isCamera { camera = pixel } else { screen = pixel }
    sequence &+= 1
  }
  static func clear() { lock.lock(); screen = nil; camera = nil; showScreen = true; sequence &+= 1; lock.unlock() }
}

private enum LivePreviewLevels {
  static let lock = NSLock()
  static var systemPeakDb: Double?
  static var micPeakDb: Double?
  static func clear() { lock.lock(); systemPeakDb = nil; micPeakDb = nil; lock.unlock() }
  private static func peakDb(_ sample: CMSampleBuffer) -> Double? {
    guard let description = CMSampleBufferGetFormatDescription(sample),
      let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(description)?.pointee else { return nil }
    var retained: CMBlockBuffer?
    var list = AudioBufferList(mNumberBuffers: 1,
      mBuffers: AudioBuffer(mNumberChannels: 0, mDataByteSize: 0, mData: nil))
    guard CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(sample,
      bufferListSizeNeededOut: nil, bufferListOut: &list,
      bufferListSize: MemoryLayout<AudioBufferList>.size,
      blockBufferAllocator: kCFAllocatorDefault,
      blockBufferMemoryAllocator: kCFAllocatorDefault,
      flags: kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment,
      blockBufferOut: &retained) == noErr else { return nil }
    var peak = 0.0
    let isFloat = (asbd.mFormatFlags & kAudioFormatFlagIsFloat) != 0
    for buffer in UnsafeMutableAudioBufferListPointer(&list) {
      guard let raw = buffer.mData else { continue }
      if isFloat && asbd.mBitsPerChannel == 32 {
        let count = Int(buffer.mDataByteSize) / MemoryLayout<Float32>.size
        let values = raw.bindMemory(to: Float32.self, capacity: count)
        for index in 0..<count { peak = max(peak, abs(Double(values[index]))) }
      } else if asbd.mBitsPerChannel == 16 {
        let count = Int(buffer.mDataByteSize) / MemoryLayout<Int16>.size
        let values = raw.bindMemory(to: Int16.self, capacity: count)
        for index in 0..<count { peak = max(peak, abs(Double(values[index])) / 32768.0) }
      } else { return nil }
    }
    return peak > 0 ? 20 * log10(min(peak, 1)) : -120
  }
  static func updateSystem(_ sample: CMSampleBuffer) {
    guard let peak = peakDb(sample) else { return }
    lock.lock(); systemPeakDb = peak; lock.unlock()
  }
  static func updateMic(_ sample: CMSampleBuffer, gainDb: Double) {
    guard let raw = peakDb(sample) else { return }
    let adjusted = min(0, max(-120, raw + min(24, max(-24, gainDb))))
    lock.lock(); micPeakDb = adjusted; lock.unlock()
  }
}

private final class LiveMonitor: NSObject, SCStreamOutput, SCStreamDelegate, AVCaptureVideoDataOutputSampleBufferDelegate, AVCaptureAudioDataOutputSampleBufferDelegate {
  let queue = DispatchQueue(label: "ai.aeroshoot.live-preview", autoreleaseFrequency: .workItem)
  var stream: SCStream?
  var camera: AVCaptureSession?
  var micGainDb = 0.0
  func start(source: String, captureScreen: Bool, captureSystemAudio: Bool, cameraID: String?, micID: String?, micGainDb: Double) throws {
    self.micGainDb = micGainDb
    if captureScreen || captureSystemAudio {
    let semaphore = DispatchSemaphore(value: 0)
    var content: SCShareableContent?
    var failure: Error?
    SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { value, error in
      content = value; failure = error; semaphore.signal()
    }
    guard semaphore.wait(timeout: .now() + 10) == .success, let content else {
      throw failure ?? NSError(domain: "AeroShoot", code: 1, userInfo: [NSLocalizedDescriptionKey: "Screen preview enumeration failed"])
    }
    let parts = source.split(separator: ":", maxSplits: 1).map(String.init)
    guard parts.count == 2 else { throw NSError(domain: "AeroShoot", code: 2) }
    let filter: SCContentFilter
    if parts[0] == "display", let id = UInt32(parts[1]), let display = content.displays.first(where: { $0.displayID == id }) {
      let own = content.applications.filter { $0.processID == ProcessInfo.processInfo.processIdentifier }
      filter = SCContentFilter(display: display, excludingApplications: own, exceptingWindows: [])
    } else if parts[0] == "window", let id = UInt32(parts[1]), let window = content.windows.first(where: { $0.windowID == id }) {
      filter = SCContentFilter(desktopIndependentWindow: window)
    } else if parts[0] == "application", let app = content.applications.first(where: { $0.bundleIdentifier == parts[1] }), let display = content.displays.first {
      filter = SCContentFilter(display: display, including: [app], exceptingWindows: [])
    } else { throw NSError(domain: "AeroShoot", code: 3, userInfo: [NSLocalizedDescriptionKey: "Selected preview source is unavailable"]) }
    let config = SCStreamConfiguration()
    config.width = 1280; config.height = 720
    config.minimumFrameInterval = CMTime(value: 1, timescale: 15)
    config.queueDepth = 3; config.pixelFormat = kCVPixelFormatType_32BGRA
    config.showsCursor = true; config.capturesAudio = captureSystemAudio
    config.sampleRate = 48_000; config.channelCount = 2
    if #available(macOS 14.0, *) {
      config.preservesAspectRatio = true
    }
    let next = SCStream(filter: filter, configuration: config, delegate: self)
    if captureScreen { try next.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue) }
    if captureSystemAudio { try next.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue) }
    stream = next
    let started = DispatchSemaphore(value: 0)
    next.startCapture { error in failure = error; started.signal() }
    guard started.wait(timeout: .now() + 10) == .success else { throw NSError(domain: "AeroShoot", code: 4, userInfo: [NSLocalizedDescriptionKey: "Screen preview startup timed out"]) }
    if let failure { throw failure }
    }
    if cameraID != nil || micID != nil {
      do {
        let session = AVCaptureSession()
        session.beginConfiguration()
        session.sessionPreset = .hd1280x720
        if let cameraID {
          guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else {
            throw NSError(domain: "AeroShoot", code: 5, userInfo: [NSLocalizedDescriptionKey: "Allow Camera access to preview the selected webcam"])
          }
          guard let device = AVCaptureDevice(uniqueID: cameraID) else { throw NSError(domain: "AeroShoot", code: 6, userInfo: [NSLocalizedDescriptionKey: "Selected webcam is unavailable"]) }
          let input = try AVCaptureDeviceInput(device: device)
          let output = AVCaptureVideoDataOutput()
          // The recorder's format, so a starting recording can take this running
          // session over (livePreviewHandOff). Core Image draws it either way.
          output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange]
          output.alwaysDiscardsLateVideoFrames = true
          output.setSampleBufferDelegate(self, queue: queue)
          guard session.canAddInput(input), session.canAddOutput(output) else { throw NSError(domain: "AeroShoot", code: 7, userInfo: [NSLocalizedDescriptionKey: "Cannot open webcam preview"]) }
          session.addInput(input); session.addOutput(output)
        }
        if let micID {
          guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
            throw NSError(domain: "AeroShoot", code: 9, userInfo: [NSLocalizedDescriptionKey: "Allow Microphone access to meter the selected input"])
          }
          guard let device = AVCaptureDevice(uniqueID: micID) else { throw NSError(domain: "AeroShoot", code: 10, userInfo: [NSLocalizedDescriptionKey: "Selected microphone is unavailable"]) }
          let input = try AVCaptureDeviceInput(device: device)
          let output = AVCaptureAudioDataOutput()
          output.setSampleBufferDelegate(self, queue: queue)
          guard session.canAddInput(input), session.canAddOutput(output) else { throw NSError(domain: "AeroShoot", code: 11, userInfo: [NSLocalizedDescriptionKey: "Cannot open microphone meter"]) }
          session.addInput(input); session.addOutput(output)
        }
        session.commitConfiguration()
        camera = session; session.startRunning()
        guard session.isRunning else { throw NSError(domain: "AeroShoot", code: 8, userInfo: [NSLocalizedDescriptionKey: "Camera/microphone preview did not start"]) }
      } catch {
        // Screen preview remains useful when the camera is busy or denied.
        camera = nil
      }
    }
  }
  func stop() {
    camera?.stopRunning()
    let deadline = Date().addingTimeInterval(2)
    while camera?.isRunning == true && Date() < deadline {
      Thread.sleep(forTimeInterval: 0.01)
    }
    camera = nil
    if let stream {
      let stopped = DispatchSemaphore(value: 0)
      stream.stopCapture { _ in stopped.signal() }
      _ = stopped.wait(timeout: .now() + 10)
    }
    stream = nil; queue.sync {}
  }
  func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
    guard sample.isValid else { return }
    if type == .audio { LivePreviewLevels.updateSystem(sample); return }
    guard type == .screen,
      let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
      let status = attachments.first?[.status] as? Int, let frameStatus = SCFrameStatus(rawValue: status) else { return }
    if frameStatus == .complete || frameStatus == .idle {
      LivePreviewFrames.offer(sample, camera: false)
    }
  }
  func captureOutput(_ output: AVCaptureOutput, didOutput sample: CMSampleBuffer, from connection: AVCaptureConnection) {
    if output is AVCaptureAudioDataOutput {
      LivePreviewLevels.updateMic(sample, gainDb: micGainDb)
    } else {
      LivePreviewFrames.offer(sample, camera: true)
    }
  }
}
// Configuration calls are serialized by Rust's command lock; reads use the mailbox lock.
private var liveMonitor: LiveMonitor?
private let liveContext = CIContext(options: [.cacheIntermediates: false])
@_cdecl("aeroshoot_live_preview_stop")
func livePreviewStop() { liveMonitor?.stop(); liveMonitor = nil; LivePreviewFrames.clear(); LivePreviewLevels.clear() }
@_cdecl("aeroshoot_live_preview_start")
func livePreviewStart(_ source: UnsafePointer<CChar>?, _ captureScreen: Bool, _ captureSystemAudio: Bool, _ camera: UnsafePointer<CChar>?, _ mic: UnsafePointer<CChar>?, _ micGainDb: Double) -> UnsafeMutablePointer<CChar>? {
  livePreviewStop()
  guard let source else { return strdup("No screen source selected") }
  let monitor = LiveMonitor()
  do {
    LivePreviewFrames.lock.lock(); LivePreviewFrames.showScreen = captureScreen; LivePreviewFrames.lock.unlock()
    try monitor.start(source: String(cString: source), captureScreen: captureScreen, captureSystemAudio: captureSystemAudio, cameraID: camera.map { String(cString: $0) }, micID: mic.map { String(cString: $0) }, micGainDb: micGainDb)
    liveMonitor = monitor
    return nil
  } catch { monitor.stop(); return strdup(error.localizedDescription) }
}

/// Whether a preview capture session opened exactly the devices a recording needs.
func livePreviewSessionMatches(deviceIDs: Set<String>, cameraID: String?, micID: String?) -> Bool {
  !deviceIDs.isEmpty && deviceIDs == Set([cameraID, micID].compactMap { $0 })
}

/// Stops the preview because a recording is starting. If the preview's camera
/// and microphone session runs exactly the recording's devices, `adopt` is
/// offered it first; when it accepts, the session keeps running for the
/// recording, so the camera does not restart (about 1.3 s for a FaceTime
/// camera). Returns whether the session was adopted. Serialized with preview
/// start and stop by Rust's command lock.
func livePreviewHandOff(cameraID: String?, micID: String?, captureScreen: Bool, adopt: (AVCaptureSession) -> Bool) -> Bool {
  var adopted = false
  if let monitor = liveMonitor, let session = monitor.camera, session.isRunning {
    let deviceIDs = Set(session.inputs.compactMap { ($0 as? AVCaptureDeviceInput)?.device.uniqueID })
    if livePreviewSessionMatches(deviceIDs: deviceIDs, cameraID: cameraID, micID: micID), adopt(session) {
      monitor.camera = nil
      adopted = true
    }
  }
  liveMonitor?.stop()
  liveMonitor = nil
  LivePreviewLevels.clear()
  // Keep the newest frames of sources the recording continues, so the preview
  // does not blank while the recording's first frames arrive.
  LivePreviewFrames.lock.lock()
  if !captureScreen { LivePreviewFrames.screen = nil }
  if cameraID == nil { LivePreviewFrames.camera = nil }
  LivePreviewFrames.showScreen = captureScreen
  LivePreviewFrames.sequence &+= 1
  LivePreviewFrames.lock.unlock()
  return adopted
}

@_cdecl("aeroshoot_live_preview_copy_levels_json")
func livePreviewCopyLevelsJson() -> UnsafeMutablePointer<CChar>? {
  LivePreviewLevels.lock.lock(); let systemPeak = LivePreviewLevels.systemPeakDb; let micPeak = LivePreviewLevels.micPeakDb; LivePreviewLevels.lock.unlock()
  let object: [String: Any] = ["systemAudioPeakDb": systemPeak ?? NSNull(), "micPeakDb": micPeak ?? NSNull()]
  guard let data = try? JSONSerialization.data(withJSONObject: object),
    let text = String(data: data, encoding: .utf8) else { return nil }
  return strdup(text)
}
private func livePreviewPlaced(_ pixel: CVPixelBuffer, _ rect: CGRect, cover: Bool) -> CIImage {
  let source = CIImage(cvPixelBuffer: pixel)
  let scale = cover
    ? max(rect.width / source.extent.width, rect.height / source.extent.height)
    : min(rect.width / source.extent.width, rect.height / source.extent.height)
  let scaledW = source.extent.width * scale
  let scaledH = source.extent.height * scale
  let tx = rect.midX - scaledW / 2 - source.extent.minX * scale
  let ty = rect.midY - scaledH / 2 - source.extent.minY * scale
  let image = source
    .transformed(by: CGAffineTransform(scaleX: scale, y: scale))
    .transformed(by: CGAffineTransform(translationX: tx, y: ty))
  return cover ? image.cropped(to: rect) : image
}

private let livePreviewBounds = CGRect(x: 0, y: 0, width: 1280, height: 720)

/// Screen letterboxed into the 1280x720 preview canvas with the webcam bubble
/// in the corner. Returns nil when there is nothing to show.
private func livePreviewComposite(screen: CVPixelBuffer?, camera: CVPixelBuffer?) -> CIImage? {
  guard screen != nil || camera != nil else { return nil }
  var image = CIImage(color: CIColor(red: 0.02, green: 0.02, blue: 0.03)).cropped(to: livePreviewBounds)
  if let screen { image = livePreviewPlaced(screen, livePreviewBounds, cover: false).composited(over: image) }
  if let camera { image = livePreviewPlaced(camera, CGRect(x: 980, y: 20, width: 280, height: 158), cover: false).composited(over: image) }
  return image
}

/// Receives rendered preview frames on the main thread.
protocol LivePreviewTarget: AnyObject {
  func presentLivePreview(_ buffer: CVPixelBuffer)
}

/// Draws the newest mailbox frames into the on-screen preview entirely on the
/// GPU. Core Image composites into preview-owned IOSurface buffers, so capture
/// buffers are released immediately, and the main thread only swaps the view
/// layer's contents. Nothing is copied through the CPU or Rust.
final class LivePreviewRenderer {
  static let shared = LivePreviewRenderer()

  private let queue = DispatchQueue(label: "ai.aeroshoot.preview-render", autoreleaseFrequency: .workItem)
  private let context: CIContext
  private let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
  private var pool: CVPixelBufferPool?
  private var timer: DispatchSourceTimer?
  private weak var target: LivePreviewTarget?
  private var lastSequence: UInt64?
  /// 30 fps normally; 15 fps while recording, when the preview is only a
  /// monitor and the capture pipeline should get the spare CPU/GPU time.
  private var intervalMs = 33

  private init() {
    // Preview only: skip Core Image color management, which adds conversion
    // passes to every frame. Recorded media never goes through this context.
    let options: [CIContextOption: Any] = [
      .cacheIntermediates: false, .workingColorSpace: NSNull(), .outputColorSpace: NSNull(),
    ]
    if let device = MTLCreateSystemDefaultDevice() {
      context = CIContext(mtlDevice: device, options: options)
    } else {
      context = CIContext(options: options)
    }
    let bufferAttributes: [String: Any] = [
      kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
      kCVPixelBufferWidthKey as String: Int(livePreviewBounds.width),
      kCVPixelBufferHeightKey as String: Int(livePreviewBounds.height),
      kCVPixelBufferIOSurfacePropertiesKey as String: [String: Any](),
      kCVPixelBufferMetalCompatibilityKey as String: true,
    ]
    let poolAttributes: [String: Any] = [kCVPixelBufferPoolMinimumBufferCountKey as String: 3]
    CVPixelBufferPoolCreate(nil, poolAttributes as CFDictionary, bufferAttributes as CFDictionary, &pool)
  }

  /// Render into `next` at ~30 fps while it is set; nil stops rendering.
  func setTarget(_ next: LivePreviewTarget?) {
    queue.async { [self] in
      target = next
      lastSequence = nil
      guard next != nil else {
        timer?.cancel()
        timer = nil
        return
      }
      guard timer == nil else { return }
      let source = DispatchSource.makeTimerSource(queue: queue)
      source.schedule(deadline: .now(), repeating: .milliseconds(intervalMs), leeway: .milliseconds(5))
      source.setEventHandler { [weak self] in self?.renderIfChanged() }
      timer = source
      source.resume()
    }
  }

  /// Lowers the preview rate while a recording is running.
  func setRecording(_ recording: Bool) {
    queue.async { [self] in
      intervalMs = recording ? 66 : 33
      timer?.schedule(deadline: .now(), repeating: .milliseconds(intervalMs), leeway: .milliseconds(5))
    }
  }

  /// Render immediately, on the render queue. Used by contract tests.
  func renderNow() {
    queue.sync { renderIfChanged() }
  }

  private func renderIfChanged() {
    guard let target, let pool else { return }
    LivePreviewFrames.lock.lock()
    let sequence = LivePreviewFrames.sequence
    let screen = LivePreviewFrames.showScreen ? LivePreviewFrames.screen : nil
    let camera = LivePreviewFrames.camera
    LivePreviewFrames.lock.unlock()
    guard sequence != lastSequence, let image = livePreviewComposite(screen: screen, camera: camera) else { return }
    var rendered: CVPixelBuffer?
    guard CVPixelBufferPoolCreatePixelBuffer(nil, pool, &rendered) == kCVReturnSuccess, let output = rendered else { return }
    CVBufferSetAttachment(output, kCVImageBufferCGColorSpaceKey, colorSpace, .shouldPropagate)
    context.render(image, to: output, bounds: livePreviewBounds, colorSpace: nil)
    lastSequence = sequence
    DispatchQueue.main.async { [weak target] in
      target?.presentLivePreview(output)
    }
  }
}

@_cdecl("aeroshoot_live_preview_read")
func livePreviewRead(_ bytes: UnsafeMutableRawPointer?, _ length: Int32) -> Int32 {
  // Called repeatedly by a thread without an AppKit run loop autorelease
  // pool. CIImage's autoreleased render graph retains IOSurfaces; without a
  // per-call pool it exhausts SCStream's queue after a few frames.
  return autoreleasepool {
  guard let bytes, length >= 1280 * 720 * 4 else { return 0 }
  LivePreviewFrames.lock.lock()
  let screen = LivePreviewFrames.showScreen ? LivePreviewFrames.screen : nil
  let camera = LivePreviewFrames.camera
  LivePreviewFrames.lock.unlock()
  guard let image = livePreviewComposite(screen: screen, camera: camera) else { return 0 }
  liveContext.render(image, toBitmap: bytes, rowBytes: 1280 * 4, bounds: livePreviewBounds, format: .BGRA8, colorSpace: CGColorSpace(name: CGColorSpace.itur_709)!)
  return (screen == nil ? 0 : 1) | (camera == nil ? 0 : 2)
  }
}

@_cdecl("aeroshoot_live_preview_read_camera")
func livePreviewReadCamera(_ bytes: UnsafeMutableRawPointer?, _ length: Int32) -> Int32 {
  return autoreleasepool {
  guard let bytes, length >= 1280 * 720 * 4 else { return 0 }
  LivePreviewFrames.lock.lock()
  let camera = LivePreviewFrames.camera
  LivePreviewFrames.lock.unlock()
  guard let camera else { return 0 }
  let image = livePreviewPlaced(camera, livePreviewBounds, cover: true)
    .composited(over: CIImage(color: CIColor(red: 0.02, green: 0.02, blue: 0.03)).cropped(to: livePreviewBounds))
  liveContext.render(image, toBitmap: bytes, rowBytes: 1280 * 4, bounds: livePreviewBounds, format: .BGRA8, colorSpace: CGColorSpace(name: CGColorSpace.itur_709)!)
  return 1
  }
}
