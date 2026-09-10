import AppKit
import AVFoundation
import CoreGraphics
import CoreVideo
import Darwin
import Foundation
import QuartzCore

/// Child overlay on the window contentView, sibling above WKWebView.
/// React sends CSS points relative to the webview; this adapter owns physical
/// sizing, z-order, hit testing and lifetime. Pixels never cross the Tauri IPC.
private enum PreviewHitMode: Int32 {
  case consume = 0
  case circlePassthrough = 1
  case passthrough = 2
  case circleDragPassthrough = 3
  case squircleDragPassthrough = 4
}

private final class AeroShootPreviewView: NSView {
  var fill = NSColor(calibratedRed: 0.06, green: 0.09, blue: 0.18, alpha: 1)
  var image: CGImage?
  var hitMode = PreviewHitMode.consume
  var generation: UInt64 = 0
  var copies: UInt64 = 0
  var presentedBytes: UInt64 = 0
  var presentedKind = "none"
  var occluded = false
  var clipRect: NSRect?

  override var isFlipped: Bool { true }
  override var isOpaque: Bool { image == nil && fill.alphaComponent >= 1 }
  override var wantsUpdateLayer: Bool { true }

  override func hitTest(_ point: NSPoint) -> NSView? {
    if hitMode == .passthrough || hitMode == .circleDragPassthrough || hitMode == .squircleDragPassthrough {
      return nil
    }
    let local = convert(point, from: superview)
    if let clipRect, !clipRect.contains(local) { return nil }
    if hitMode == .circlePassthrough && !PreviewGeometry.circleContains(bounds: bounds, point: local) {
      return nil
    }
    return super.hitTest(point)
  }

  func applyShape() {
    guard let layer else { return }
    layer.masksToBounds = true
    switch hitMode {
    case .circlePassthrough, .circleDragPassthrough:
      layer.cornerRadius = min(bounds.width, bounds.height) / 2
    case .squircleDragPassthrough:
      layer.cornerRadius = min(28, min(bounds.width, bounds.height) / 4)
    case .consume, .passthrough:
      layer.cornerRadius = 0
    }
    // HUD overlays are square/circle windows showing a 16:9 camera mailbox.
    // Aspect-fit left a large empty (often white) disk with a squeezed image.
    switch hitMode {
    case .consume:
      layer.contentsGravity = .resizeAspect
    case .passthrough, .circlePassthrough, .circleDragPassthrough, .squircleDragPassthrough:
      layer.contentsGravity = .resizeAspectFill
    }
  }

  override func updateLayer() {
    layer?.backgroundColor = fill.cgColor
    layer?.contents = image
    applyShape()
  }
}

enum PreviewGeometry {
  static func circleContains(bounds: NSRect, point: NSPoint) -> Bool {
    let rx = bounds.width / 2
    let ry = bounds.height / 2
    if rx <= 0 || ry <= 0 { return false }
    let dx = (point.x - bounds.midX) / rx
    let dy = (point.y - bounds.midY) / ry
    return dx * dx + dy * dy <= 1
  }

  static func physical(x: Double, y: Double, width: Double, height: Double, scale: Double) -> NSRect {
    let s = max(scale, 0.5)
    return NSRect(
      x: (x * s).rounded(),
      y: (y * s).rounded(),
      width: (width * s).rounded(),
      height: (height * s).rounded()
    )
  }

  /// DOM rectangles use a top-left origin. Most AppKit views use a bottom-left
  /// origin, so convert the y-axis before asking AppKit to translate between
  /// the webview and its parent.
  static func webRect(css: NSRect, bounds: NSRect, isFlipped: Bool) -> NSRect {
    NSRect(
      x: bounds.minX + css.minX,
      y: isFlipped ? bounds.minY + css.minY : bounds.maxY - css.maxY,
      width: css.width,
      height: css.height
    )
  }
}

private final class PreviewSurface {
  let view = AeroShootPreviewView(frame: .zero)
  var windowLabel = ""
  var visible = true
  var lastRevision: UInt64 = 0
  var backingScale: CGFloat = 1
  var physical = NSRect.zero

  init() {
    view.wantsLayer = true
    view.layerContentsRedrawPolicy = .onSetNeedsDisplay
  }

  func attach(to window: NSWindow) {
    view.removeFromSuperview()
    guard let parent = window.contentView else { return }
    if view.superview !== parent {
      parent.addSubview(view, positioned: .above, relativeTo: findWebView(parent))
    }
  }

  func detach() {
    view.removeFromSuperview()
    view.image = nil
  }
}

private func findWebView(_ view: NSView) -> NSView? {
  if NSStringFromClass(type(of: view)).contains("WKWebView") {
    return view
  }
  for child in view.subviews {
    if let found = findWebView(child) {
      return found
    }
  }
  return nil
}

private func onMain<T>(_ body: () -> T) -> T {
  if Thread.isMainThread {
    return body()
  }
  return DispatchQueue.main.sync(execute: body)
}

private func takeSurface(_ handle: UnsafeMutableRawPointer?) -> PreviewSurface? {
  guard let handle else { return nil }
  return Unmanaged<PreviewSurface>.fromOpaque(handle).takeUnretainedValue()
}

@_cdecl("aeroshoot_preview_attach")
func previewAttach(_ nsWindow: UnsafeMutableRawPointer?, _ generation: UInt64) -> UnsafeMutableRawPointer? {
  guard let nsWindow else { return nil }
  return onMain {
    let window = Unmanaged<NSWindow>.fromOpaque(nsWindow).takeUnretainedValue()
    let surface = PreviewSurface()
    surface.view.generation = generation == 0 ? 1 : generation
    surface.attach(to: window)
    return Unmanaged.passRetained(surface).toOpaque()
  }
}

@_cdecl("aeroshoot_preview_detach")
func previewDetach(_ handle: UnsafeMutableRawPointer?) {
  guard let handle else { return }
  onMain {
    let surface = Unmanaged<PreviewSurface>.fromOpaque(handle).takeRetainedValue()
    surface.detach()
  }
}

@_cdecl("aeroshoot_preview_set_geometry")
func previewSetGeometry(
  _ handle: UnsafeMutableRawPointer?,
  _ x: Double,
  _ y: Double,
  _ width: Double,
  _ height: Double,
  _ backingScale: Double,
  _ visible: Bool,
  _ occluded: Bool,
  _ revision: UInt64,
  _ generation: UInt64
) -> Int32 {
  guard let surface = takeSurface(handle) else { return 1 }
  return onMain {
    if generation != 0 && generation != surface.view.generation {
      return 2
    }
    if revision < surface.lastRevision {
      return 3
    }
    if width <= 0 || height <= 0 || backingScale <= 0 || width > 8192 || height > 8192 {
      return 4
    }
    surface.lastRevision = revision
    surface.backingScale = CGFloat(backingScale)
    surface.visible = visible
    surface.view.occluded = occluded
    surface.view.isHidden = !visible || occluded
    let parent = surface.view.superview
    let web = parent.flatMap(findWebView) ?? parent
    let css = NSRect(x: x, y: y, width: width, height: height)
    if let web, let parent, web !== parent {
      let webRect = PreviewGeometry.webRect(css: css, bounds: web.bounds, isFlipped: web.isFlipped)
      surface.view.frame = web.convert(webRect, to: parent)
    } else {
      let bounds = parent?.bounds ?? .zero
      surface.view.frame = PreviewGeometry.webRect(
        css: css,
        bounds: bounds,
        isFlipped: parent?.isFlipped ?? true)
    }
    surface.physical = PreviewGeometry.physical(
      x: x, y: y, width: width, height: height, scale: backingScale)
    surface.view.applyShape()
    surface.view.needsDisplay = true
    return 0
  }
}

@_cdecl("aeroshoot_preview_set_clip")
func previewSetClip(_ handle: UnsafeMutableRawPointer?, _ x: Double, _ y: Double, _ width: Double, _ height: Double) -> Int32 {
  guard let surface = takeSurface(handle) else { return 1 }
  return onMain {
    let rect = NSRect(x: x, y: y, width: max(0, width), height: max(0, height))
    surface.view.clipRect = rect
    let mask = CAShapeLayer()
    mask.path = CGPath(rect: rect, transform: nil)
    surface.view.layer?.mask = mask
    return 0
  }
}

@_cdecl("aeroshoot_preview_set_hit_mode")
func previewSetHitMode(_ handle: UnsafeMutableRawPointer?, _ mode: Int32) -> Int32 {
  guard let surface = takeSurface(handle) else { return 1 }
  return onMain {
    surface.view.hitMode = PreviewHitMode(rawValue: mode) ?? .consume
    surface.view.applyShape()
    return 0
  }
}

@_cdecl("aeroshoot_preview_present_fixed")
func previewPresentFixed(
  _ handle: UnsafeMutableRawPointer?,
  _ r: Float,
  _ g: Float,
  _ b: Float,
  _ generation: UInt64
) -> Int32 {
  guard let surface = takeSurface(handle) else { return 1 }
  return onMain {
    if generation != 0 && generation != surface.view.generation {
      return 2
    }
    surface.view.image = nil
    surface.view.fill = NSColor(
      calibratedRed: CGFloat(r), green: CGFloat(g), blue: CGFloat(b), alpha: 1)
    surface.view.copies += 1
    surface.view.presentedBytes = 0
    surface.view.presentedKind = "fixed"
    surface.view.needsDisplay = true
    surface.view.updateLayer()
    return 0
  }
}

@_cdecl("aeroshoot_preview_present_bgra")
func previewPresentBgra(
  _ handle: UnsafeMutableRawPointer?,
  _ width: Int32,
  _ height: Int32,
  _ stride: Int32,
  _ pixels: UnsafePointer<UInt8>?,
  _ len: Int32,
  _ generation: UInt64
) -> Int32 {
  guard let surface = takeSurface(handle), let pixels else { return 1 }
  if width <= 0 || height <= 0 || stride < width * 4 || len < stride * height {
    return 4
  }
  return onMain {
    if generation != 0 && generation != surface.view.generation {
      return 2
    }
    let bytesPerRow = Int(stride)
    guard let provider = CGDataProvider(
      data: Data(bytes: pixels, count: Int(len)) as CFData)
    else {
      return 5
    }
    guard let image = CGImage(
      width: Int(width),
      height: Int(height),
      bitsPerComponent: 8,
      bitsPerPixel: 32,
      bytesPerRow: bytesPerRow,
      space: CGColorSpace(name: CGColorSpace.itur_709)!,
      bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue)
        .union(.byteOrder32Little),
      provider: provider,
      decode: nil,
      shouldInterpolate: false,
      intent: .defaultIntent
    ) else {
      return 5
    }
    surface.view.image = image
    surface.view.copies += 1
    surface.view.presentedBytes = UInt64(len)
    surface.view.presentedKind = "decoded"
    surface.view.needsDisplay = true
    surface.view.updateLayer()
    return 0
  }
}

@_cdecl("aeroshoot_preview_decode_present")
func previewDecodePresent(
  _ handle: UnsafeMutableRawPointer?,
  _ path: UnsafePointer<CChar>?,
  _ generation: UInt64
) -> Int32 {
  guard let path else { return 1 }
  let url = URL(fileURLWithPath: String(cString: path))
  let asset = AVURLAsset(url: url)
  let generator = AVAssetImageGenerator(asset: asset)
  generator.appliesPreferredTrackTransform = true
  generator.maximumSize = CGSize(width: 128, height: 128)
  var actual = CMTime.zero
  let image: CGImage
  do {
    image = try generator.copyCGImage(at: .zero, actualTime: &actual)
  } catch {
    return 6
  }
  guard let surface = takeSurface(handle) else { return 1 }
  return onMain {
    if generation != 0 && generation != surface.view.generation {
      return 2
    }
    surface.view.image = image
    surface.view.copies += 1
    surface.view.presentedBytes = UInt64(image.width * image.height * 4)
    surface.view.presentedKind = "decoded"
    surface.view.needsDisplay = true
    surface.view.updateLayer()
    return 0
  }
}

@_cdecl("aeroshoot_preview_hit_test")
func previewHitTest(_ handle: UnsafeMutableRawPointer?, _ x: Double, _ y: Double) -> Int32 {
  guard let surface = takeSurface(handle) else { return 0 }
  return onMain {
    let point = NSPoint(x: x, y: y)
    return surface.view.hitTest(point) == nil ? 0 : 1
  }
}

@_cdecl("aeroshoot_preview_copy_stats_json")
func previewCopyStatsJson(_ handle: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>? {
  guard let surface = takeSurface(handle) else {
    return strdup("{\"attached\":false}")
  }
  return onMain {
    let stats: [String: Any] = [
      "attached": surface.view.superview != nil,
      "arrangement": "child_overlay",
      "generation": NSNumber(value: surface.view.generation),
      "layoutRevision": NSNumber(value: surface.lastRevision),
      "visible": surface.visible,
      "occluded": surface.view.occluded,
      "backingScale": Double(surface.backingScale),
      "physicalWidth": Double(surface.physical.width),
      "physicalHeight": Double(surface.physical.height),
      "copiesPerPresent": 1,
      "copies": NSNumber(value: surface.view.copies),
      "presentedBytes": NSNumber(value: surface.view.presentedBytes),
      "presentedKind": surface.view.presentedKind,
      "hitMode": Int(surface.view.hitMode.rawValue),
    ]
    guard JSONSerialization.isValidJSONObject(stats),
          let data = try? JSONSerialization.data(withJSONObject: stats),
          let text = String(data: data, encoding: .utf8)
    else {
      return strdup("{\"attached\":true}")
    }
    return strdup(text)
  }
}

@_cdecl("aeroshoot_preview_write_solid_mp4")
func previewWriteSolidMp4(
  _ path: UnsafePointer<CChar>?,
  _ width: Int32,
  _ height: Int32,
  _ r: Float,
  _ g: Float,
  _ b: Float
) -> Int32 {
  guard let path, width >= 16, height >= 16, width % 2 == 0, height % 2 == 0 else { return 1 }
  let url = URL(fileURLWithPath: String(cString: path))
  try? FileManager.default.removeItem(at: url)
  guard let writer = try? AVAssetWriter(outputURL: url, fileType: .mp4) else { return 5 }
    let settings: [String: Any] = [
      AVVideoCodecKey: AVVideoCodecType.h264,
      AVVideoWidthKey: width,
      AVVideoHeightKey: height,
    AVVideoColorPropertiesKey: [
      AVVideoColorPrimariesKey: AVVideoColorPrimaries_ITU_R_709_2,
      AVVideoTransferFunctionKey: AVVideoTransferFunction_ITU_R_709_2,
      AVVideoYCbCrMatrixKey: AVVideoYCbCrMatrix_ITU_R_709_2,
    ],
      AVVideoCompressionPropertiesKey: [
        AVVideoAverageBitRateKey: 120_000,
        AVVideoProfileLevelKey: AVVideoProfileLevelH264BaselineAutoLevel,
      ],
    ]
  let input = AVAssetWriterInput(mediaType: .video, outputSettings: settings)
  input.expectsMediaDataInRealTime = false
  let adaptor = AVAssetWriterInputPixelBufferAdaptor(
    assetWriterInput: input,
    sourcePixelBufferAttributes: [
      kCVPixelBufferPixelFormatTypeKey as String: Int(kCVPixelFormatType_32BGRA),
      kCVPixelBufferWidthKey as String: width,
      kCVPixelBufferHeightKey as String: height,
    ]
  )
  guard writer.canAdd(input) else { return 5 }
  writer.add(input)
  guard writer.startWriting() else { return 5 }
  writer.startSession(atSourceTime: .zero)
  var buffer: CVPixelBuffer?
  let status = CVPixelBufferCreate(
    kCFAllocatorDefault,
    Int(width),
    Int(height),
    kCVPixelFormatType_32BGRA,
    nil,
    &buffer
  )
  guard status == kCVReturnSuccess, let buffer else { return 5 }
  CVPixelBufferLockBaseAddress(buffer, [])
  if let base = CVPixelBufferGetBaseAddress(buffer) {
    let stride = CVPixelBufferGetBytesPerRow(buffer)
    let rb = UInt8(max(0, min(255, r * 255)))
    let gb = UInt8(max(0, min(255, g * 255)))
    let bb = UInt8(max(0, min(255, b * 255)))
    for row in 0..<Int(height) {
      let rowPtr = base.advanced(by: row * stride).assumingMemoryBound(to: UInt8.self)
      for col in 0..<Int(width) {
        let o = col * 4
        rowPtr[o] = bb
        rowPtr[o + 1] = gb
        rowPtr[o + 2] = rb
        rowPtr[o + 3] = 255
      }
    }
  }
  CVPixelBufferUnlockBaseAddress(buffer, [])
  func appendFrame(_ time: CMTime) -> Bool {
    let started = Date()
    while !input.isReadyForMoreMediaData && Date().timeIntervalSince(started) < 2 {
      Thread.sleep(forTimeInterval: 0.01)
    }
    return input.isReadyForMoreMediaData && adaptor.append(buffer, withPresentationTime: time)
  }
  for index in 0..<6 {
    guard appendFrame(CMTime(value: CMTimeValue(index), timescale: 30)) else { return 5 }
  }
  writer.endSession(atSourceTime: CMTime(value: 6, timescale: 30))
  input.markAsFinished()
  let done = DispatchSemaphore(value: 0)
  writer.finishWriting { done.signal() }
  _ = done.wait(timeout: .now() + 8)
  if writer.status != .completed {
    fputs(
      "previewWriteSolidMp4 failed: \(writer.status.rawValue) \(String(describing: writer.error))\n",
      stderr)
    return 5
  }
  return 0
}

#if PREVIEW_CONTRACT_TESTS
enum AeroShootPreviewTests {
  static func run() {
    assert(PreviewGeometry.circleContains(
      bounds: NSRect(x: 0, y: 0, width: 100, height: 100),
      point: NSPoint(x: 50, y: 50)))
    assert(!PreviewGeometry.circleContains(
      bounds: NSRect(x: 0, y: 0, width: 100, height: 100),
      point: NSPoint(x: 1, y: 1)))
    let physical = PreviewGeometry.physical(x: 10, y: 20, width: 100, height: 50, scale: 2)
    assert(physical.width == 200 && physical.height == 100)
    let css = NSRect(x: 10, y: 20, width: 100, height: 50)
    assert(PreviewGeometry.webRect(
      css: css,
      bounds: NSRect(x: 0, y: 0, width: 320, height: 240),
      isFlipped: false) == NSRect(x: 10, y: 170, width: 100, height: 50))
    assert(PreviewGeometry.webRect(
      css: css,
      bounds: NSRect(x: 0, y: 0, width: 320, height: 240),
      isFlipped: true) == css)

    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 320, height: 240),
      styleMask: [.titled, .closable],
      backing: .buffered,
      defer: false
    )
    window.isReleasedWhenClosed = false
    let handle = previewAttach(Unmanaged.passUnretained(window).toOpaque(), 1)
    assert(handle != nil)
    assert(previewSetGeometry(handle, 16, 16, 160, 90, 2, true, false, 1, 1) == 0)
    assert(previewSetGeometry(handle, 16, 16, 160, 90, 2, true, false, 0, 1) == 3)
    assert(previewSetGeometry(handle, 16, 16, 160, 90, 2, false, true, 2, 1) == 0)
    if let stats = previewCopyStatsJson(handle) {
      let text = String(cString: stats)
      free(stats)
      assert(text.contains("\"occluded\":true"))
      assert(text.contains("\"arrangement\":\"child_overlay\""))
      assert(!text.contains("pixels"))
    }
    assert(previewSetGeometry(handle, 16, 16, 160, 90, 2, true, false, 3, 1) == 0)
    assert(previewPresentFixed(handle, 0.2, 0.4, 0.8, 1) == 0)
    assert(previewPresentFixed(handle, 1, 0, 0, 99) == 2)

    var bgra = [UInt8](repeating: 0, count: 16 * 16 * 4)
    for i in stride(from: 0, to: bgra.count, by: 4) {
      bgra[i] = 16
      bgra[i + 1] = 32
      bgra[i + 2] = 48
      bgra[i + 3] = 255
    }
    assert(bgra.withUnsafeBufferPointer {
      previewPresentBgra(handle, 16, 16, 64, $0.baseAddress, Int32(bgra.count), 1)
    } == 0)

    _ = NSApplication.shared
    let fixture = FileManager.default.temporaryDirectory
      .appendingPathComponent("aeroshoot-f1.mp4")
    assert(fixture.path.withCString { previewWriteSolidMp4($0, 128, 128, 1, 0, 0) } == 0)
    assert(fixture.path.withCString { previewDecodePresent(handle, $0, 1) } == 0)

    let hud = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 120, height: 120),
      styleMask: [.borderless],
      backing: .buffered,
      defer: false
    )
    hud.isOpaque = false
    hud.backgroundColor = .clear
    hud.isReleasedWhenClosed = false
    let hudHandle = previewAttach(Unmanaged.passUnretained(hud).toOpaque(), 1)
    assert(previewSetGeometry(hudHandle, 0, 0, 120, 120, 1, true, false, 1, 1) == 0)
    assert(previewSetHitMode(hudHandle, 1) == 0)
    assert(previewHitTest(hudHandle, 60, 60) == 1)
    assert(previewHitTest(hudHandle, 1, 1) == 0)
    assert(previewSetClip(hudHandle, 60, 0, 60, 120) == 0)
    assert(previewHitTest(hudHandle, 30, 60) == 0)
    assert(previewHitTest(hudHandle, 90, 60) == 1)
    assert(previewSetHitMode(hudHandle, 3) == 0)
    assert(previewHitTest(hudHandle, 60, 60) == 0)
    if let stats = previewCopyStatsJson(hudHandle) {
      let text = String(cString: stats)
      free(stats)
      assert(text.contains("\"hitMode\":3"))
    }
    let hudView = AeroShootPreviewView(frame: NSRect(x: 0, y: 0, width: 120, height: 120))
    hudView.wantsLayer = true
    hudView.hitMode = .circleDragPassthrough
    hudView.updateLayer()
    assert(hudView.layer?.contentsGravity == .resizeAspectFill)
    hudView.hitMode = .consume
    hudView.updateLayer()
    assert(hudView.layer?.contentsGravity == .resizeAspect)

    previewDetach(handle)
    previewDetach(hudHandle)
    let reopened = previewAttach(Unmanaged.passUnretained(window).toOpaque(), 7)
    assert(reopened != nil)
    assert(previewPresentFixed(reopened, 0, 1, 0, 7) == 0)
    assert(previewPresentFixed(reopened, 0, 1, 0, 1) == 2)
    previewDetach(reopened)
    window.close()
    hud.close()
    print("Native preview contracts passed (child overlay, decode fixture, HUD hit test)")
  }
}
#endif
