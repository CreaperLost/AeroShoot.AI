use std::{ffi::c_void, ptr::NonNull};
#[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
extern "C" {
    fn aeroshoot_audio_open() -> *mut c_void;
    fn aeroshoot_audio_close(handle: *mut c_void);
    fn aeroshoot_audio_queue(handle: *mut c_void, pcm: *const i16, frames: i32) -> i32;
    fn aeroshoot_audio_play(handle: *mut c_void);
    fn aeroshoot_audio_position(handle: *mut c_void) -> i64;
}
pub struct AudioOutput {
    handle: NonNull<c_void>,
}
// Calls are serialized by the playback mutex; no callbacks retain the Rust owner.
unsafe impl Send for AudioOutput {}
impl AudioOutput {
    pub fn new() -> Result<Self, String> {
        #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
        unsafe {
            return NonNull::new(aeroshoot_audio_open())
                .map(|handle| Self { handle })
                .ok_or("Could not start the audio output device".into());
        }
        #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
        Err("Audio playback is not supported on this platform".into())
    }
    pub fn queue(&mut self, pcm: &[i16]) -> Result<(), String> {
        if pcm.is_empty() || pcm.len() % 2 != 0 || pcm.len() / 2 > crate::media::audio::CHUNK_FRAMES
        {
            return Err("Invalid playback PCM chunk".into());
        }
        #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
        unsafe {
            if aeroshoot_audio_queue(self.handle.as_ptr(), pcm.as_ptr(), (pcm.len() / 2) as i32)
                != 0
            {
                return Err("Audio scheduling failed".into());
            }
        }
        Ok(())
    }
    pub fn play(&mut self) {
        #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
        unsafe {
            aeroshoot_audio_play(self.handle.as_ptr());
        }
    }
    pub fn position_frames(&self) -> Result<u64, String> {
        #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
        unsafe {
            return u64::try_from(aeroshoot_audio_position(self.handle.as_ptr()))
                .map_err(|_| "Audio device stopped".into());
        }
        #[cfg(not(all(target_os = "macos", not(stub_swift_ffi))))]
        Err("No audio output".into())
    }
}
impl Drop for AudioOutput {
    fn drop(&mut self) {
        #[cfg(all(target_os = "macos", not(stub_swift_ffi)))]
        unsafe {
            aeroshoot_audio_close(self.handle.as_ptr());
        }
    }
}
