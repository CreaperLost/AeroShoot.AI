import AVFoundation
import Foundation

// Owned by one Rust PlaybackOwner, whose mutex serializes all calls.
private final class PlaybackAudio {
  let engine = AVAudioEngine()
  let player = AVAudioPlayerNode()
  let format = AVAudioFormat(standardFormatWithSampleRate: 48000, channels: 2)!
  var scheduled: Int64 = 0
  init() throws {
    engine.attach(player)
    engine.connect(player, to: engine.mainMixerNode, format: format)
    engine.prepare()
    try engine.start()
  }
  deinit { player.stop(); engine.stop() }
}
@_cdecl("aeroshoot_audio_open")
func audioOpen() -> UnsafeMutableRawPointer? {
  guard let audio = try? PlaybackAudio() else { return nil }
  return Unmanaged.passRetained(audio).toOpaque()
}
@_cdecl("aeroshoot_audio_close")
func audioClose(_ handle: UnsafeMutableRawPointer?) {
  guard let handle else { return }
  let audio = Unmanaged<PlaybackAudio>.fromOpaque(handle).takeRetainedValue()
  audio.player.stop(); audio.engine.stop()
}
@_cdecl("aeroshoot_audio_queue")
func audioQueue(_ handle: UnsafeMutableRawPointer?, _ pcm: UnsafePointer<Int16>?, _ frames: Int32) -> Int32 {
  guard let handle, let pcm, frames > 0, frames <= 4800 else { return 1 }
  let audio = Unmanaged<PlaybackAudio>.fromOpaque(handle).takeUnretainedValue()
  guard let buffer = AVAudioPCMBuffer(pcmFormat: audio.format, frameCapacity: AVAudioFrameCount(frames)), let data = buffer.floatChannelData else { return 2 }
  buffer.frameLength = AVAudioFrameCount(frames)
  for f in 0..<Int(frames) {
    data[0][f] = Float(pcm[2*f]) / 32768
    data[1][f] = Float(pcm[2*f+1]) / 32768
  }
  let when = AVAudioTime(sampleTime: audio.scheduled, atRate: 48000)
  audio.player.scheduleBuffer(buffer, at: when, options: [], completionHandler: nil)
  audio.scheduled += Int64(frames)
  return 0
}
@_cdecl("aeroshoot_audio_play")
func audioPlay(_ handle: UnsafeMutableRawPointer?) {
  guard let handle else { return }
  Unmanaged<PlaybackAudio>.fromOpaque(handle).takeUnretainedValue().player.play()
}
@_cdecl("aeroshoot_audio_position")
func audioPosition(_ handle: UnsafeMutableRawPointer?) -> Int64 {
  guard let handle else { return -1 }
  let audio = Unmanaged<PlaybackAudio>.fromOpaque(handle).takeUnretainedValue()
  guard audio.engine.isRunning else { return -1 }
  guard let render = audio.player.lastRenderTime, let time = audio.player.playerTime(forNodeTime: render) else { return 0 }
  let latency = Int64((audio.player.outputPresentationLatency * 48000).rounded())
  return min(audio.scheduled, max(0, time.sampleTime - latency))
}
