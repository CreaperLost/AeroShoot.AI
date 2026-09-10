import AVFoundation
import ScreenCaptureKit
import CoreImage
import AppKit

// Capture callbacks retain only the newest pixel buffer per video source.
// Recording and idle monitoring use the same mailbox; no frames go through JS.
enum LivePreviewFrames {
  static let lock = NSLock()
  static var screen: CVPixelBuffer?
  static var camera: CVPixelBuffer?
  static func offer(_ sample: CMSampleBuffer, camera isCamera: Bool) {
    guard let pixel = CMSampleBufferGetImageBuffer(sample) else { return }
    lock.lock(); defer { lock.unlock() }
    if isCamera { camera = pixel } else { screen = pixel }
  }
  static func clear() { lock.lock(); screen = nil; camera = nil; lock.unlock() }
}

private final class LiveMonitor: NSObject, SCStreamOutput, SCStreamDelegate, AVCaptureVideoDataOutputSampleBufferDelegate {
  let queue = DispatchQueue(label: "ai.aeroshoot.live-preview", autoreleaseFrequency: .workItem)
  var stream: SCStream?
  var camera: AVCaptureSession?
  func start(source: String, cameraID: String?) throws {
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
    config.showsCursor = true; config.capturesAudio = false
    if #available(macOS 14.0, *) {
      config.preservesAspectRatio = true
    }
    let next = SCStream(filter: filter, configuration: config, delegate: self)
    try next.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
    stream = next
    let started = DispatchSemaphore(value: 0)
    next.startCapture { error in failure = error; started.signal() }
    guard started.wait(timeout: .now() + 10) == .success else { throw NSError(domain: "AeroShoot", code: 4, userInfo: [NSLocalizedDescriptionKey: "Screen preview startup timed out"]) }
    if let failure { throw failure }
    if let cameraID {
      do {
        guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else {
          throw NSError(domain: "AeroShoot", code: 5, userInfo: [NSLocalizedDescriptionKey: "Allow Camera access to preview the selected webcam"])
        }
        guard let device = AVCaptureDevice(uniqueID: cameraID) else { throw NSError(domain: "AeroShoot", code: 6, userInfo: [NSLocalizedDescriptionKey: "Selected webcam is unavailable"]) }
        let session = AVCaptureSession()
        session.beginConfiguration()
        session.sessionPreset = .hd1280x720
        let input = try AVCaptureDeviceInput(device: device)
        let output = AVCaptureVideoDataOutput()
        output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
        output.alwaysDiscardsLateVideoFrames = true
        output.setSampleBufferDelegate(self, queue: queue)
        guard session.canAddInput(input), session.canAddOutput(output) else { throw NSError(domain: "AeroShoot", code: 7, userInfo: [NSLocalizedDescriptionKey: "Cannot open webcam preview"]) }
        session.addInput(input); session.addOutput(output); session.commitConfiguration()
        camera = session; session.startRunning()
        guard session.isRunning else { throw NSError(domain: "AeroShoot", code: 8, userInfo: [NSLocalizedDescriptionKey: "Webcam preview did not start"]) }
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
    guard type == .screen, sample.isValid,
      let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
      let status = attachments.first?[.status] as? Int, let frameStatus = SCFrameStatus(rawValue: status) else { return }
    if frameStatus == .complete || frameStatus == .idle {
      LivePreviewFrames.offer(sample, camera: false)
    }
  }
  func captureOutput(_ output: AVCaptureOutput, didOutput sample: CMSampleBuffer, from connection: AVCaptureConnection) {
    LivePreviewFrames.offer(sample, camera: true)
  }
}
// Configuration calls are serialized by Rust's command lock; reads use the mailbox lock.
private var liveMonitor: LiveMonitor?
private let liveContext = CIContext(options: [.cacheIntermediates: false])
@_cdecl("aeroshoot_live_preview_stop")
func livePreviewStop() { liveMonitor?.stop(); liveMonitor = nil; LivePreviewFrames.clear() }
@_cdecl("aeroshoot_live_preview_start")
func livePreviewStart(_ source: UnsafePointer<CChar>?, _ camera: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>? {
  livePreviewStop()
  guard let source else { return strdup("No screen source selected") }
  let monitor = LiveMonitor()
  do {
    try monitor.start(source: String(cString: source), cameraID: camera.map { String(cString: $0) })
    liveMonitor = monitor
    return nil
  } catch { monitor.stop(); return strdup(error.localizedDescription) }
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

@_cdecl("aeroshoot_live_preview_read")
func livePreviewRead(_ bytes: UnsafeMutableRawPointer?, _ length: Int32) -> Int32 {
  // Called repeatedly by a Rust std::thread, which has no AppKit run loop
  // autorelease pool. CIImage's autoreleased render graph retains IOSurfaces;
  // without a per-call pool it exhausts SCStream's queue after a few frames.
  return autoreleasepool {
  guard let bytes, length >= 1280 * 720 * 4 else { return 0 }
  LivePreviewFrames.lock.lock()
  let screen = LivePreviewFrames.screen, camera = LivePreviewFrames.camera
  LivePreviewFrames.lock.unlock()
  guard screen != nil || camera != nil else { return 0 }
  let bounds = CGRect(x: 0, y: 0, width: 1280, height: 720)
  var image = CIImage(color: CIColor(red: 0.02, green: 0.02, blue: 0.03)).cropped(to: bounds)
  if let screen { image = livePreviewPlaced(screen, bounds, cover: false).composited(over: image) }
  if let camera { image = livePreviewPlaced(camera, CGRect(x: 980, y: 20, width: 280, height: 158), cover: false).composited(over: image) }
  liveContext.render(image, toBitmap: bytes, rowBytes: 1280 * 4, bounds: bounds, format: .BGRA8, colorSpace: CGColorSpace(name: CGColorSpace.itur_709)!)
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
  let bounds = CGRect(x: 0, y: 0, width: 1280, height: 720)
  let image = livePreviewPlaced(camera, bounds, cover: true)
    .composited(over: CIImage(color: CIColor(red: 0.02, green: 0.02, blue: 0.03)).cropped(to: bounds))
  liveContext.render(image, toBitmap: bytes, rowBytes: 1280 * 4, bounds: bounds, format: .BGRA8, colorSpace: CGColorSpace(name: CGColorSpace.itur_709)!)
  return 1
  }
}
