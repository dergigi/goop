//! Local camera worker. A short-lived child owns the device so Cancel also
//! releases cameras whose drivers are blocked waiting for the next frame.
use anyhow::{Result, anyhow, bail, ensure};
use image::{DynamicImage, RgbaImage};
use nokhwa::{
    Camera,
    pixel_format::RgbFormat,
    utils::{ApiBackend, RequestedFormat, RequestedFormatType, Resolution},
};
use nostr_sdk::prelude::*;
use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const ARG: &str = "--goop-qr-camera";
const MAX_PACKET: usize = 4 * 1920 * 1080 + 16;
const MAX_URL: usize = 8192;

pub(crate) fn bunker_url(value: &str) -> Result<String> {
    ensure!(
        value.len() <= MAX_URL,
        "The QR code is too large for a signer connection."
    );
    match NostrConnectUri::parse(value.trim()) {
        Ok(uri @ NostrConnectUri::Bunker { .. }) => Ok(uri.to_string()),
        _ => bail!(
            "This QR code is not a bunker connection. Show the bunker QR code from your signer app."
        ),
    }
}

pub(crate) enum CaptureEvent {
    Devices(Vec<String>),
    Frame(RgbaImage),
    Decoded(String),
    Error(String),
}

pub(crate) struct Capture {
    child: Child,
}
impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CameraCapture")
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Capture {
    pub fn start(index: usize) -> Result<(Self, flume::Receiver<CaptureEvent>)> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg(ARG)
            .arg(index.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn()?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Camera preview is unavailable"))?;
        let (sender, receiver) = flume::bounded(2);
        std::thread::spawn(move || {
            loop {
                let event = match read_packet(&mut stdout) {
                    Ok(event) => event,
                    Err(_) => {
                        let _ = sender.send(CaptureEvent::Error("The camera stopped responding. Check its connection and camera permissions, then try again.".into()));
                        break;
                    }
                };
                match event {
                    CaptureEvent::Frame(_) => {
                        if sender.try_send(event).is_err() && sender.is_disconnected() {
                            break;
                        }
                    }
                    CaptureEvent::Decoded(_) | CaptureEvent::Error(_) => {
                        let _ = sender.send(event);
                        break;
                    }
                    _ => {
                        if sender.send(event).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok((Self { child }, receiver))
    }
}

fn write_packet(writer: &mut impl Write, kind: u8, bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() <= MAX_PACKET,
        "Camera data exceeds preview limit"
    );
    writer.write_all(&[kind])?;
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(bytes)?;
    writer.flush()?;
    Ok(())
}
fn read_packet(reader: &mut impl Read) -> Result<CaptureEvent> {
    let mut header = [0; 5];
    reader.read_exact(&mut header)?;
    let size = u32::from_le_bytes(header[1..].try_into()?) as usize;
    ensure!(size <= MAX_PACKET, "Camera data exceeds preview limit");
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes)?;
    match header[0] {
        1 => Ok(CaptureEvent::Devices(serde_json::from_slice(&bytes)?)),
        2 => {
            ensure!(bytes.len() >= 8, "Invalid camera frame");
            let width = u32::from_le_bytes(bytes[..4].try_into()?);
            let height = u32::from_le_bytes(bytes[4..8].try_into()?);
            ensure!(
                width > 0 && height > 0 && width <= 1920 && height <= 1080,
                "Invalid camera dimensions"
            );
            let image = RgbaImage::from_raw(width, height, bytes[8..].to_vec())
                .ok_or_else(|| anyhow!("Invalid camera frame"))?;
            Ok(CaptureEvent::Frame(image))
        }
        3 => {
            ensure!(size <= MAX_URL, "Oversized QR code");
            Ok(CaptureEvent::Decoded(String::from_utf8(bytes)?))
        }
        4 => Ok(CaptureEvent::Error(String::from_utf8(bytes)?)),
        _ => bail!("Invalid camera response"),
    }
}

fn decode(image: &image::GrayImage) -> Option<String> {
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(
        image.width() as usize,
        image.height() as usize,
        |x, y| image.get_pixel(x as u32, y as u32)[0],
    );
    prepared
        .detect_grids()
        .iter()
        .find_map(|grid| grid.decode().ok().map(|(_, text)| text))
}

/// Called before starting GPUI or logging. No camera work occurs on normal launch.
pub fn run_if_requested() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(ARG)) {
        return false;
    }
    let index = args
        .next()
        .and_then(|value| value.to_str().and_then(|value| value.parse().ok()))
        .unwrap_or(0);
    // The parent keeps stdin open. An app crash must not leave its camera worker alive.
    watch_parent();
    let result = run(index);
    if let Err(error) = result {
        let _ = write_packet(
            &mut std::io::stdout().lock(),
            4,
            error.to_string().as_bytes(),
        );
    }
    true
}
fn watch_parent() {
    std::thread::spawn(|| {
        let mut byte = [0];
        let _ = std::io::stdin().read(&mut byte);
        std::process::exit(0);
    });
}
fn run(index: usize) -> Result<()> {
    // AVFoundation's permission callback is asynchronous; this process can be
    // terminated while permission is pending, without keeping the app busy.
    let (sender, receiver) = std::sync::mpsc::channel();
    nokhwa::nokhwa_initialize(move |allowed| {
        let _ = sender.send(allowed);
    });
    ensure!(
        receiver
            .recv_timeout(Duration::from_secs(120))
            .unwrap_or(false),
        "Camera access is unavailable. Allow Goop to use your camera in system privacy settings, then try again. You can also paste the bunker URL."
    );
    let devices = nokhwa::query(ApiBackend::Auto).map_err(|_| {
        anyhow!("Could not list cameras. Check camera permissions and reconnect your camera.")
    })?;
    ensure!(
        !devices.is_empty(),
        "No camera found. Connect a camera and try again, or paste the bunker URL."
    );
    let names: Vec<_> = devices.iter().map(|device| device.human_name()).collect();
    let mut stdout = std::io::stdout().lock();
    write_packet(&mut stdout, 1, &serde_json::to_vec(&names)?)?;
    let device = devices.get(index).ok_or_else(|| {
        anyhow!("The selected camera is no longer connected. Try scanning again.")
    })?;
    // Prefer 720p for dense signer QR codes; fall back for webcams with other modes.
    let requested = RequestedFormat::new::<RgbFormat>(RequestedFormatType::HighestResolution(
        Resolution::new(1280, 720),
    ));
    let mut camera = Camera::new(device.index().clone(),requested)
        .or_else(|_| Camera::new(device.index().clone(),RequestedFormat::new::<RgbFormat>(RequestedFormatType::None)))
        .map_err(|_| anyhow!("Could not open this camera. Close other apps using it, check camera permissions, or select another camera."))?;
    camera.open_stream().map_err(|_| {
        anyhow!("Could not start the camera. Close other apps using it and try again.")
    })?;
    let started = Instant::now();
    loop {
        ensure!(
            started.elapsed() < Duration::from_secs(120),
            "Scanning timed out. Try again or paste the bunker URL."
        );
        let tick = Instant::now();
        let frame = camera.frame().map_err(|_| {
            anyhow!(
                "The camera disconnected or stopped providing images. Reconnect it and try again."
            )
        })?;
        let rgb = frame.decode_image::<RgbFormat>().map_err(|_| anyhow!("This camera's image format is not supported. Select another camera or paste the bunker URL."))?;
        let image = DynamicImage::ImageRgb8(rgb);
        // Bound decoder cost even when a webcam chooses a 4K fallback mode.
        let scan = image
            .resize(1280, 1280, image::imageops::FilterType::Triangle)
            .to_luma8();
        if let Some(text) = decode(&scan) {
            camera.stop_stream().ok();
            write_packet(&mut stdout, 3, text.as_bytes())?;
            return Ok(());
        }
        let preview = image
            .resize(640, 480, image::imageops::FilterType::Triangle)
            .to_rgba8();
        let mut bytes = Vec::with_capacity(preview.len() + 8);
        bytes.extend_from_slice(&preview.width().to_le_bytes());
        bytes.extend_from_slice(&preview.height().to_le_bytes());
        bytes.extend_from_slice(preview.as_raw());
        write_packet(&mut stdout, 2, &bytes)?;
        std::thread::sleep(Duration::from_millis(100).saturating_sub(tick.elapsed()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn url() -> String {
        format!(
            "bunker://{}?relay=wss%3A%2F%2Frelay.example.com&secret=test-only",
            Keys::generate().public_key().to_hex()
        )
    }
    #[test]
    fn validates_only_bunker_urls_without_exposing_scanned_secrets() {
        let url = url();
        assert!(bunker_url(&format!("  {url}\n")).is_ok());
        for value in [
            "nsec1secret",
            "https://example.com",
            "nostrconnect://bad",
            "",
        ] {
            let error = bunker_url(value).unwrap_err().to_string();
            assert!(!error.contains(value) || value.is_empty());
        }
        assert!(bunker_url(&"x".repeat(MAX_URL + 1)).is_err());
    }
    #[test]
    fn decodes_real_qr_pixels_for_a_bunker_link() {
        let url = url();
        let code = qrcode::QrCode::new(url.as_bytes()).unwrap();
        let scale = 5;
        let border = 4;
        let width = (code.width() + border * 2) * scale;
        let image = image::GrayImage::from_fn(width as u32, width as u32, |x, y| {
            let (x, y) = (x as usize / scale, y as usize / scale);
            let black = x >= border
                && y >= border
                && x < code.width() + border
                && y < code.width() + border
                && code[(x - border, y - border)] == qrcode::Color::Dark;
            image::Luma([if black { 0 } else { 255 }])
        });
        assert_eq!(decode(&image), Some(url.clone()));
        assert_eq!(decode(&image::imageops::rotate90(&image)), Some(url));
        assert!(decode(&image::GrayImage::from_pixel(200, 200, image::Luma([255]))).is_none());
    }
    #[test]
    fn camera_packets_are_bounded_and_validate_frame_dimensions() {
        let mut bytes = Vec::new();
        write_packet(&mut bytes, 3, url().as_bytes()).unwrap();
        assert!(matches!(
            read_packet(&mut bytes.as_slice()).unwrap(),
            CaptureEvent::Decoded(_)
        ));
        assert!(read_packet(&mut [2, 255, 255, 255, 255].as_slice()).is_err());
        let mut bytes = Vec::new();
        write_packet(&mut bytes, 2, &[0; 8]).unwrap();
        assert!(read_packet(&mut bytes.as_slice()).is_err());
    }
    // Subprocess fixture: never opens a camera. Its parent either cancels or dies.
    #[test]
    fn camera_worker_fixture() {
        if std::env::var_os("GOOP_CAMERA_TEST_CHILD").is_none() {
            return;
        }
        watch_parent();
        println!("camera-fixture-ready");
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    #[test]
    fn cancel_and_parent_disconnect_stop_a_stalled_camera_worker() {
        for cancel in [true, false] {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "qr_camera::tests::camera_worker_fixture",
                    "--nocapture",
                ])
                .env("GOOP_CAMERA_TEST_CHILD", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(stdout)
                    .lines()
                    .map_while(std::result::Result::ok)
                {
                    if line.contains("camera-fixture-ready") {
                        let _ = ready_tx.send(());
                    }
                }
                let _ = done_tx.send(());
            });
            let mut capture = Capture { child };
            ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if cancel {
                drop(capture);
            } else {
                drop(capture.child.stdin.take());
                done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(capture.child.wait().unwrap().success());
                continue;
            }
            done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }
}
