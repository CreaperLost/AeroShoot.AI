import AVFoundation

@main
struct RecordingWriterTests {
  static func main() throws {
    try previewReleasesCaptureBuffers()
    try RecordingWriterContracts.run()
  }

  // Model a capture provider with a small reusable IOSurface pool, called from
  // a long-lived Rust worker. An outer pool deliberately does not drain between
  // frames: the Swift FFI entry point must release its own Cocoa temporaries.
  static func previewReleasesCaptureBuffers() throws {
    for camera in [false, true] {
      try autoreleasepool {
        var pool: CVPixelBufferPool?
        precondition(CVPixelBufferPoolCreate(nil, nil, [
          kCVPixelBufferWidthKey: 64,
          kCVPixelBufferHeightKey: 64,
          kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA,
          kCVPixelBufferIOSurfacePropertiesKey: [:]
        ] as CFDictionary, &pool) == kCVReturnSuccess)
        var bytes = [UInt8](repeating: 0, count: 1280 * 720 * 4)
        for index in 0..<180 {
          var pixel: CVPixelBuffer?
          let status = CVPixelBufferPoolCreatePixelBufferWithAuxAttributes(nil, pool!,
            [kCVPixelBufferPoolAllocationThresholdKey: 6] as CFDictionary, &pixel)
          guard status == kCVReturnSuccess else {
            throw NSError(domain: "RecordingTests", code: Int(status), userInfo: [
              NSLocalizedDescriptionKey: "\(camera ? "Camera" : "Screen") preview exhausted the capture buffer pool at frame \(index)"
            ])
          }
          LivePreviewFrames.lock.lock()
          if camera { LivePreviewFrames.camera = pixel } else { LivePreviewFrames.screen = pixel }
          LivePreviewFrames.lock.unlock()
          pixel = nil
          bytes.withUnsafeMutableBytes { buffer in
            precondition(livePreviewRead(buffer.baseAddress, Int32(buffer.count)) != 0)
            if camera { precondition(livePreviewReadCamera(buffer.baseAddress, Int32(buffer.count)) != 0) }
          }
          LivePreviewFrames.clear()
        }
      }
    }
    print("Preview passed: 180 screen and camera frames reuse a six-buffer capture pool")
  }
}
