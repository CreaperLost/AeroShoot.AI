import AVFoundation
import CoreGraphics
import CoreImage
import CoreVideo
import Foundation
import VideoToolbox

/// F2 media adapter: owned BGRA frames in-process. Pixels never cross Tauri IPC.
private let maxDim: Int32 = 4096
private let maxFrames: Int32 = 8

private func copyImageBgra(_ image: CGImage) -> (UnsafeMutablePointer<UInt8>, Int32, Int32, Int32)? {
  let width = image.width
  let height = image.height
  if width <= 0 || height <= 0 || width > Int(maxDim) || height > Int(maxDim) {
    return nil
  }
  let stride = width * 4
  let count = stride * height
  guard let raw = malloc(count) else { return nil }
  let ptr = raw.assumingMemoryBound(to: UInt8.self)
  memset(ptr, 0, count)
  let bitmapInfo = CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue)
    .union(.byteOrder32Little)
  guard let ctx = CGContext(
    data: ptr,
    width: width,
    height: height,
    bitsPerComponent: 8,
    bytesPerRow: stride,
    space: CGColorSpace(name: CGColorSpace.itur_709)!,
    bitmapInfo: bitmapInfo.rawValue
  ) else {
    free(raw)
    return nil
  }
  ctx.interpolationQuality = .none
  ctx.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
  return (ptr, Int32(width), Int32(height), Int32(stride))
}

// Each decoder retains only the current and next sample. Two source files are
// sufficient for the screen and camera; seeks backwards reopen that file reader.
private final class SegmentDecoder {
  let asset: AVURLAsset
  let track: AVAssetTrack
  var reader: AVAssetReader!
  var output: AVAssetReaderTrackOutput!
  var current: CMSampleBuffer?
  var next: CMSampleBuffer?
  var lastRequest: Int64 = -1
  init(url: URL) throws {
    asset = AVURLAsset(url: url)
    guard let track = asset.tracks(withMediaType: .video).first else { throw NSError(domain: "AeroShoot", code: 1) }
    self.track = track
    try reset()
  }
  func reset() throws {
    reader?.cancelReading()
    reader = try AVAssetReader(asset: asset)
    output = AVAssetReaderTrackOutput(track: track, outputSettings: [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA])
    output.alwaysCopiesSampleData = false
    guard reader.canAdd(output) else { throw NSError(domain: "AeroShoot", code: 2) }
    reader.add(output)
    guard reader.startReading() else { throw reader.error ?? NSError(domain: "AeroShoot", code: 3) }
    current = nil; next = output.copyNextSampleBuffer(); lastRequest = -1
  }
  func frame(at timeUs: Int64) throws -> CMSampleBuffer? {
    guard timeUs >= 0, Double(timeUs) / 1_000_000 < asset.duration.seconds else { return nil }
    if timeUs < lastRequest { try reset() }
    lastRequest = timeUs
    while let candidate = next {
      let pts = Int64((CMSampleBufferGetPresentationTimeStamp(candidate).seconds * 1_000_000).rounded())
      if pts > timeUs && current != nil { break }
      current = candidate
      next = output.copyNextSampleBuffer()
      if pts > timeUs { break }
    }
    if reader.status == .failed { throw reader.error ?? NSError(domain: "AeroShoot", code: 4) }
    return current
  }
}
private let decoderLock = NSLock()
private var decoderCache: [(String, SegmentDecoder)] = []
private let imageContext = CIContext(options: [.cacheIntermediates: false, .workingColorSpace: CGColorSpace(name: CGColorSpace.itur_709)!, .outputColorSpace: CGColorSpace(name: CGColorSpace.itur_709)!])

@_cdecl("aeroshoot_media_decode_bgra")
func mediaDecodeBgra(
  _ path: UnsafePointer<CChar>?, _ timeUs: Int64,
  _ outWidth: UnsafeMutablePointer<Int32>?, _ outHeight: UnsafeMutablePointer<Int32>?,
  _ outStride: UnsafeMutablePointer<Int32>?, _ outPtsUs: UnsafeMutablePointer<Int64>?,
  _ outLen: UnsafeMutablePointer<Int32>?
) -> UnsafeMutablePointer<UInt8>? {
  guard let path, let outWidth, let outHeight, let outStride, let outPtsUs, let outLen else { return nil }
  decoderLock.lock(); defer { decoderLock.unlock() }
  let name = String(cString: path)
  let url = URL(fileURLWithPath: name)
  guard let attrs = try? FileManager.default.attributesOfItem(atPath: name) else { return nil }
  let key = "\(name):\(String(describing: attrs[.size])):\(String(describing: attrs[.modificationDate]))"
  let decoder: SegmentDecoder
  if let index = decoderCache.firstIndex(where: { $0.0 == key }) {
    decoder = decoderCache.remove(at: index).1
  } else {
    guard let created = try? SegmentDecoder(url: url) else { return nil }
    decoder = created
  }
  decoderCache.append((key, decoder))
  if decoderCache.count > 2 { decoderCache.removeFirst() }
  guard let sample = try? decoder.frame(at: timeUs), let buffer = CMSampleBufferGetImageBuffer(sample) else { return nil }
  var ci = CIImage(cvPixelBuffer: buffer).transformed(by: decoder.track.preferredTransform)
  let scale = min(1, CGFloat(maxDim) / max(ci.extent.width, ci.extent.height))
  if scale < 1 { ci = ci.transformed(by: CGAffineTransform(scaleX: scale, y: scale)) }
  guard let image = imageContext.createCGImage(ci, from: ci.extent), let copied = copyImageBgra(image) else { return nil }
  outWidth.pointee = copied.1; outHeight.pointee = copied.2; outStride.pointee = copied.3
  outPtsUs.pointee = Int64((CMSampleBufferGetPresentationTimeStamp(sample).seconds * 1_000_000).rounded())
  outLen.pointee = copied.2 * copied.3
  return copied.0
}

@_cdecl("aeroshoot_media_free")
func mediaFree(_ ptr: UnsafeMutableRawPointer?) {
  free(ptr)
}

@_cdecl("aeroshoot_media_encode_bgra_mp4")
func mediaEncodeBgraMp4(
  _ path: UnsafePointer<CChar>?,
  _ width: Int32,
  _ height: Int32,
  _ fps: Int32,
  _ frameCount: Int32,
  _ pixels: UnsafePointer<UInt8>?,
  _ len: Int32,
  _ stride: Int32
) -> Int32 {
  guard let path, let pixels else { return 1 }
  guard width >= 16, height >= 16, width % 2 == 0, height % 2 == 0 else { return 1 }
  guard width <= maxDim, height <= maxDim else { return 1 }
  guard frameCount >= 1, frameCount <= maxFrames, fps >= 1, fps <= 60 else { return 1 }
  guard stride >= width * 4, len >= stride * height * frameCount else { return 4 }
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
      AVVideoAverageBitRateKey: max(2_000_000, Int(width * height * 32)),
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

  for index in 0..<Int(frameCount) {
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
      let destStride = CVPixelBufferGetBytesPerRow(buffer)
      let srcOff = index * Int(stride) * Int(height)
      for row in 0..<Int(height) {
        let src = pixels.advanced(by: srcOff + row * Int(stride))
        let dst = base.advanced(by: row * destStride)
        memcpy(dst, src, Int(width) * 4)
      }
    }
    CVPixelBufferUnlockBaseAddress(buffer, [])
    let started = Date()
    while !input.isReadyForMoreMediaData && Date().timeIntervalSince(started) < 2 {
      Thread.sleep(forTimeInterval: 0.01)
    }
    let time = CMTime(value: CMTimeValue(index), timescale: CMTimeScale(fps))
    guard input.isReadyForMoreMediaData, adaptor.append(buffer, withPresentationTime: time) else {
      return 5
    }
  }
  input.markAsFinished()
  let done = DispatchSemaphore(value: 0)
  writer.finishWriting { done.signal() }
  _ = done.wait(timeout: .now() + 8)
  if writer.status != .completed {
    fputs(
      "mediaEncodeBgraMp4 failed: \(writer.status.rawValue) \(String(describing: writer.error))\n",
      stderr)
    return 5
  }
  return 0
}
