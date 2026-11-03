//! Microphone permission. macOS asks the first time an app opens the mic; asking up
//! front lets the setup screen show the prompt at the right moment.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicPermission {
    Granted,
    Denied,
    /// Not asked yet: the next request shows the system prompt.
    NotAsked,
}

pub fn mic_permission() -> MicPermission {
    imp::status()
}

/// Shows the system prompt if the user hasn't been asked yet; otherwise does nothing.
pub fn request_mic_permission() {
    imp::request()
}

#[cfg(target_os = "macos")]
mod imp {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

    use super::MicPermission;

    pub fn status() -> MicPermission {
        // SAFETY: AVMediaTypeAudio is a framework constant; the call only reads state.
        let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
            return MicPermission::NotAsked;
        };
        match unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) } {
            AVAuthorizationStatus::Authorized => MicPermission::Granted,
            AVAuthorizationStatus::NotDetermined => MicPermission::NotAsked,
            _ => MicPermission::Denied,
        }
    }

    pub fn request() {
        // SAFETY: as above; the completion handler does nothing, the UI polls the status.
        if let Some(audio) = unsafe { AVMediaTypeAudio } {
            let done = RcBlock::new(|_granted: Bool| {});
            unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &done) };
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::MicPermission;

    pub fn status() -> MicPermission {
        MicPermission::Granted
    }

    pub fn request() {}
}
