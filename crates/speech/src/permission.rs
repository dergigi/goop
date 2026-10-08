//! Explicit permission request: Core Audio alone can yield a silent input stream.
#[cfg(target_os = "macos")]
pub fn microphone(cancel: &std::sync::atomic::AtomicBool) -> anyhow::Result<()> {
    use anyhow::bail;
    use block2::RcBlock;
    use objc2::{class, msg_send, runtime::Bool};
    use objc2_foundation::ns_string;
    use std::time::Duration;

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {}

    // AVAuthorizationStatus: not determined = 0, restricted = 1, denied = 2,
    // authorized = 3. "soun" is AVMediaTypeAudio.
    let status: isize = unsafe {
        msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: ns_string!("soun")]
    };
    if status == 3 {
        return Ok(());
    }
    if status != 0 {
        bail!("Allow Goop in System Settings → Privacy & Security → Microphone");
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let completion = RcBlock::new(move |granted: Bool| {
        let _ = sender.send(granted.as_bool());
    });
    unsafe {
        let _: () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: ns_string!("soun"), completionHandler: &*completion];
    }
    loop {
        super::model::check_cancel(cancel)?;
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(true) => return Ok(()),
            Ok(false) => bail!("Allow Goop in System Settings → Privacy & Security → Microphone"),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                bail!("Could not request microphone permission")
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn microphone(cancel: &std::sync::atomic::AtomicBool) -> anyhow::Result<()> {
    super::model::check_cancel(cancel)
}
