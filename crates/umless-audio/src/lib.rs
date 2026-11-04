//! Audio input for Umless: microphone capture, resampling to 16 kHz mono,
//! WAV loading, and voice activity detection.

mod capture;
mod permission;
mod vad;
mod wav;

pub use capture::{MicCapture, MicDevice, MicInfo, list_inputs};
pub use permission::{MicPermission, mic_permission, request_mic_permission};
pub use vad::{SpeechSegment, SpeechSegmenter, SpeechTrimmer, VadConfig};
pub use wav::{load_wav_16k, save_wav_16k};

/// Every model in the pipeline consumes 16 kHz mono f32 PCM.
pub const SAMPLE_RATE: u32 = 16_000;

/// Converts a sample count at [`SAMPLE_RATE`] to seconds.
pub fn samples_to_secs(n: usize) -> f32 {
    n as f32 / SAMPLE_RATE as f32
}

/// Converts seconds to a sample count at [`SAMPLE_RATE`].
pub fn secs_to_samples(secs: f32) -> usize {
    (secs.max(0.0) * SAMPLE_RATE as f32) as usize
}
