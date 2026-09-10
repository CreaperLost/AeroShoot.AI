import AVFoundation
import CoreMedia
import CoreVideo
import Darwin
import Foundation

/// Streaming H.264 + AAC writer. Frames stay in-process; Rust owns the job.
private final class ExportSession {
  let writer: AVAssetWriter
  let video: AVAssetWriterInput
  let adaptor: AVAssetWriterInputPixelBufferAdaptor
  let audio: AVAssetWriterInput?
  let width: Int32
  let height: Int32
  let sampleRate: Int32
  let channels: Int32
  var started = false

  init(
    writer: AVAssetWriter,
    video: AVAssetWriterInput,
    adaptor: AVAssetWriterInputPixelBufferAdaptor,
    audio: AVAssetWriterInput?,
    width: Int32,
    height: Int32,
    sampleRate: Int32,
    channels: Int32
  ) {
    self.writer = writer
    self.video = video
    self.adaptor = adaptor
    self.audio = audio
    self.width = width
    self.height = height
    self.sampleRate = sampleRate
    self.channels = channels
  }
}

private func waitReady(_ input: AVAssetWriterInput, writer: AVAssetWriter) -> Bool {
  let started = Date()
  while !input.isReadyForMoreMediaData
    && writer.status == .writing
    && Date().timeIntervalSince(started) < 30
  {
    Thread.sleep(forTimeInterval: 0.01)
  }
  return writer.status == .writing && input.isReadyForMoreMediaData
}

private func exportErrorMessage(_ session: ExportSession) -> String {
  if let error = session.writer.error as NSError? {
    return "\(error.domain) \(error.code): \(error.localizedDescription)"
  }
  return "AVAssetWriter status \(session.writer.status.rawValue)"
}

@_cdecl("aeroshoot_export_begin")
func exportBegin(
  _ path: UnsafePointer<CChar>?,
  _ width: Int32,
  _ height: Int32,
  _ fps: Int32,
  _ sampleRate: Int32,
  _ channels: Int32
) -> UnsafeMutableRawPointer? {
  guard let path, width >= 16, height >= 16, width % 2 == 0, height % 2 == 0 else { return nil }
  guard fps >= 1, fps <= 60, width <= 4096, height <= 4096 else { return nil }
  let url = URL(fileURLWithPath: String(cString: path))
  guard let writer = try? AVAssetWriter(outputURL: url, fileType: .mp4) else { return nil }
  let videoSettings: [String: Any] = [
    AVVideoCodecKey: AVVideoCodecType.h264,
    AVVideoWidthKey: width,
    AVVideoHeightKey: height,
    AVVideoColorPropertiesKey: [
      AVVideoColorPrimariesKey: AVVideoColorPrimaries_ITU_R_709_2,
      AVVideoTransferFunctionKey: AVVideoTransferFunction_ITU_R_709_2,
      AVVideoYCbCrMatrixKey: AVVideoYCbCrMatrix_ITU_R_709_2,
    ],
    AVVideoCompressionPropertiesKey: [
      // About 0.12 bits/pixel/frame is visually solid for screen capture and
      // stays within VideoToolbox's practical H.264 level limits. The old
      // width*height*32 value requested 66 Mbps for 1080p and caused the
      // encoder to fail after its initial frame queue filled.
      AVVideoAverageBitRateKey: max(
        2_000_000,
        Int(Double(width) * Double(height) * Double(fps) * 0.12)),
      AVVideoProfileLevelKey: AVVideoProfileLevelH264MainAutoLevel,
    ],
  ]
  let video = AVAssetWriterInput(mediaType: .video, outputSettings: videoSettings)
  video.expectsMediaDataInRealTime = false
  let adaptor = AVAssetWriterInputPixelBufferAdaptor(
    assetWriterInput: video,
    sourcePixelBufferAttributes: [
      kCVPixelBufferPixelFormatTypeKey as String: Int(kCVPixelFormatType_32BGRA),
      kCVPixelBufferWidthKey as String: width,
      kCVPixelBufferHeightKey as String: height,
    ]
  )
  guard writer.canAdd(video) else { return nil }
  writer.add(video)
  var audioInput: AVAssetWriterInput?
  if sampleRate >= 8_000, channels >= 1, channels <= 2 {
    let audioSettings: [String: Any] = [
      AVFormatIDKey: kAudioFormatMPEG4AAC,
      AVSampleRateKey: sampleRate,
      AVNumberOfChannelsKey: channels,
      AVEncoderBitRateKey: 96_000,
    ]
    let input = AVAssetWriterInput(mediaType: .audio, outputSettings: audioSettings)
    input.expectsMediaDataInRealTime = false
    guard writer.canAdd(input) else { return nil }
    writer.add(input)
    audioInput = input
  }
  guard writer.startWriting() else { return nil }
  writer.startSession(atSourceTime: .zero)
  let session = ExportSession(
    writer: writer,
    video: video,
    adaptor: adaptor,
    audio: audioInput,
    width: width,
    height: height,
    sampleRate: sampleRate,
    channels: channels
  )
  session.started = true
  return Unmanaged.passRetained(session).toOpaque()
}

@_cdecl("aeroshoot_export_video")
func exportVideo(
  _ handle: UnsafeMutableRawPointer?,
  _ ptsUs: Int64,
  _ width: Int32,
  _ height: Int32,
  _ stride: Int32,
  _ pixels: UnsafePointer<UInt8>?,
  _ len: Int32
) -> Int32 {
  guard let handle, let pixels else { return 1 }
  let session = Unmanaged<ExportSession>.fromOpaque(handle).takeUnretainedValue()
  guard width == session.width, height == session.height else { return 4 }
  guard stride >= width * 4, len >= stride * height else { return 4 }
  guard let pool = session.adaptor.pixelBufferPool else { return 5 }
  var buffer: CVPixelBuffer?
  let status = CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, pool, &buffer)
  guard status == kCVReturnSuccess, let buffer else { return 5 }
  CVPixelBufferLockBaseAddress(buffer, [])
  if let base = CVPixelBufferGetBaseAddress(buffer) {
    let destStride = CVPixelBufferGetBytesPerRow(buffer)
    for row in 0..<Int(height) {
      memcpy(
        base.advanced(by: row * destStride),
        pixels.advanced(by: row * Int(stride)),
        Int(width) * 4)
    }
  }
  CVPixelBufferUnlockBaseAddress(buffer, [])
  guard waitReady(session.video, writer: session.writer) else { return 5 }
  let time = CMTime(value: ptsUs, timescale: 1_000_000)
  return session.adaptor.append(buffer, withPresentationTime: time) ? 0 : 5
}

@_cdecl("aeroshoot_export_audio")
func exportAudio(
  _ handle: UnsafeMutableRawPointer?,
  _ ptsUs: Int64,
  _ frames: Int32,
  _ pcm: UnsafePointer<Int16>?,
  _ len: Int32
) -> Int32 {
  guard let handle, let pcm, frames > 0 else { return 1 }
  let session = Unmanaged<ExportSession>.fromOpaque(handle).takeUnretainedValue()
  guard let audio = session.audio, session.channels >= 1 else { return 1 }
  let expected = frames * session.channels
  guard len >= expected else { return 4 }
  guard let sample = makePcmBuffer(
    samples: pcm,
    frames: Int(frames),
    channels: Int(session.channels),
    sampleRate: session.sampleRate,
    ptsUs: ptsUs)
  else {
    return 5
  }
  guard waitReady(audio, writer: session.writer) else { return 5 }
  return audio.append(sample) ? 0 : 5
}

@_cdecl("aeroshoot_export_finish")
func exportFinish(_ handle: UnsafeMutableRawPointer?, _ durationUs: Int64) -> Int32 {
  guard let handle else { return 1 }
  let session = Unmanaged<ExportSession>.fromOpaque(handle).takeRetainedValue()
  session.writer.endSession(atSourceTime: CMTime(value: durationUs, timescale: 1_000_000))
  session.video.markAsFinished()
  session.audio?.markAsFinished()
  let done = DispatchSemaphore(value: 0)
  session.writer.finishWriting { done.signal() }
  _ = done.wait(timeout: .now() + 12)
  if session.writer.status != .completed {
    fputs(
      "exportFinish failed: \(session.writer.status.rawValue) \(String(describing: session.writer.error))\n",
      stderr)
    return 5
  }
  return 0
}

@_cdecl("aeroshoot_export_abort")
func exportAbort(_ handle: UnsafeMutableRawPointer?) {
  guard let handle else { return }
  let session = Unmanaged<ExportSession>.fromOpaque(handle).takeRetainedValue()
  session.writer.cancelWriting()
}

@_cdecl("aeroshoot_export_copy_error")
func exportCopyError(_ handle: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>? {
  guard let handle else { return strdup("Export session is closed") }
  let session = Unmanaged<ExportSession>.fromOpaque(handle).takeUnretainedValue()
  return strdup(exportErrorMessage(session))
}

@_cdecl("aeroshoot_media_duration_us")
func mediaDurationUs(_ path: UnsafePointer<CChar>?) -> Int64 {
  guard let path else { return -1 }
  let asset = AVURLAsset(url: URL(fileURLWithPath: String(cString: path)))
  let seconds = asset.duration.seconds
  guard seconds.isFinite, seconds >= 0 else { return -1 }
  return Int64((seconds * 1_000_000).rounded())
}

private func makePcmBuffer(
  samples: UnsafePointer<Int16>,
  frames: Int,
  channels: Int,
  sampleRate: Int32,
  ptsUs: Int64
) -> CMSampleBuffer? {
  var asbd = AudioStreamBasicDescription(
    mSampleRate: Float64(sampleRate),
    mFormatID: kAudioFormatLinearPCM,
    mFormatFlags: kAudioFormatFlagIsSignedInteger | kLinearPCMFormatFlagIsPacked,
    mBytesPerPacket: UInt32(2 * channels),
    mFramesPerPacket: 1,
    mBytesPerFrame: UInt32(2 * channels),
    mChannelsPerFrame: UInt32(channels),
    mBitsPerChannel: 16,
    mReserved: 0
  )
  var format: CMAudioFormatDescription?
  guard CMAudioFormatDescriptionCreate(
    allocator: kCFAllocatorDefault,
    asbd: &asbd,
    layoutSize: 0,
    layout: nil,
    magicCookieSize: 0,
    magicCookie: nil,
    extensions: nil,
    formatDescriptionOut: &format
  ) == noErr, let format else {
    return nil
  }
  let byteCount = frames * channels * MemoryLayout<Int16>.size
  var block: CMBlockBuffer?
  guard CMBlockBufferCreateWithMemoryBlock(
    allocator: kCFAllocatorDefault,
    memoryBlock: nil,
    blockLength: byteCount,
    blockAllocator: kCFAllocatorDefault,
    customBlockSource: nil,
    offsetToData: 0,
    dataLength: byteCount,
    flags: 0,
    blockBufferOut: &block
  ) == noErr, let block else {
    return nil
  }
  guard CMBlockBufferReplaceDataBytes(
    with: samples, blockBuffer: block, offsetIntoDestination: 0, dataLength: byteCount)
    == noErr
  else {
    return nil
  }
  var timing = CMSampleTimingInfo(
    duration: CMTime(value: 1, timescale: sampleRate),
    presentationTimeStamp: CMTime(value: ptsUs, timescale: 1_000_000),
    decodeTimeStamp: .invalid
  )
  var sample: CMSampleBuffer?
  guard CMSampleBufferCreateReady(
    allocator: kCFAllocatorDefault,
    dataBuffer: block,
    formatDescription: format,
    sampleCount: frames,
    sampleTimingEntryCount: 1,
    sampleTimingArray: &timing,
    sampleSizeEntryCount: 0,
    sampleSizeArray: nil,
    sampleBufferOut: &sample
  ) == noErr else {
    return nil
  }
  return sample
}
