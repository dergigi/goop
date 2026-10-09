//! Local dictation. Microphone capture and inference never run on the UI thread.
pub mod model;
mod permission;
mod recording;

use anyhow::{Context, Result, bail};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

static BUSY: AtomicBool = AtomicBool::new(false);
struct Lease;
impl Drop for Lease {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

#[derive(Debug)]
pub enum Event {
    Download(u64),
    Ready,
    Preparing,
    Recording { seconds: u64, level: f32 },
    Processing,
    Transcript(String),
    Notice(String),
    Failed { message: String, retryable: bool },
}
#[derive(Debug)]
enum Command {
    Finish,
    Retry,
    Cancel,
}
#[derive(Debug)]
pub struct Session {
    commands: flume::Sender<Command>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.commands.send(Command::Cancel);
    }
}
impl Session {
    pub fn finish(&self) {
        let _ = self.commands.send(Command::Finish);
    }
    pub fn retry(&self) {
        let _ = self.commands.send(Command::Retry);
    }
    pub fn start(root: PathBuf, download: bool) -> Result<(Self, flume::Receiver<Event>)> {
        if BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            bail!("Dictation is busy. Try again in a moment");
        }
        let lease = Lease;
        let (commands, receiver) = flume::unbounded();
        let (events, updates) = flume::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        std::thread::Builder::new()
            .name("goop-dictation".into())
            .spawn(move || {
                let _lease = lease;
                let result = if download {
                    model::install(&root, &stop, |bytes| {
                        let _ = events.send(Event::Download(bytes));
                    })
                    .map(|_| {
                        let _ = events.send(Event::Ready);
                    })
                } else {
                    run(root, &receiver, &events, &stop)
                };
                if let Err(error) = result {
                    if !stop.load(Ordering::Relaxed) {
                        let _ = events.send(Event::Failed {
                            message: error.to_string(),
                            retryable: false,
                        });
                    }
                }
            })?;
        Ok((Self { commands, cancel }, updates))
    }
}

fn run(
    root: PathBuf,
    commands: &flume::Receiver<Command>,
    events: &flume::Sender<Event>,
    cancel: &AtomicBool,
) -> Result<()> {
    let _model_lock = model::lock(&root)?;
    if !model::ready(&root) {
        bail!("Download the dictation model first");
    }
    events.send(Event::Preparing)?;
    permission::microphone(cancel)?;
    let recognizer = recognizer(&root)?;
    model::check_cancel(cancel)?;
    let (samples, sample_rate) = recording::capture(commands, events, cancel)?;
    model::check_cancel(cancel)?;
    loop {
        events.send(Event::Processing)?;
        let result = transcribe(&recognizer, &samples, sample_rate, cancel);
        model::check_cancel(cancel)?;
        let message = match result {
            Ok(text) if !text.is_empty() => {
                events.send(Event::Transcript(text))?;
                return Ok(());
            }
            Ok(_) => "No speech recognized. Retry or discard this recording".into(),
            Err(error) => format!("{error}. Retry or discard this recording"),
        };
        events.send(Event::Failed {
            message,
            retryable: true,
        })?;
        loop {
            model::check_cancel(cancel)?;
            match commands.recv_timeout(Duration::from_millis(100)) {
                Ok(Command::Retry) => break,
                Ok(Command::Cancel) | Err(flume::RecvTimeoutError::Disconnected) => return Ok(()),
                _ => {}
            }
        }
    }
}

fn recognizer(root: &std::path::Path) -> Result<OfflineRecognizer> {
    let directory = model::directory(root);
    let path = |name| directory.join(name).to_string_lossy().into_owned();
    let mut config = OfflineRecognizerConfig::default();
    config.model_config.transducer = OfflineTransducerModelConfig {
        encoder: Some(path("encoder.int8.onnx")),
        decoder: Some(path("decoder.int8.onnx")),
        joiner: Some(path("joiner.int8.onnx")),
    };
    config.model_config.tokens = Some(path("tokens.txt"));
    config.model_config.model_type = Some("nemo_transducer".into());
    config.model_config.provider = Some("cpu".into());
    config.model_config.num_threads =
        std::thread::available_parallelism().map_or(2, |n| n.get().min(4)) as i32;
    OfflineRecognizer::create(&config)
        .context("Could not load the dictation model. Remove it in Settings and download it again")
}

/// Bound attention memory for long recordings. Prefer a pause near each boundary.
fn segment_length(samples: &[f32], sample_rate: u32) -> usize {
    let rate = sample_rate as usize;
    let limit = (30 * rate).min(samples.len());
    if limit == samples.len() {
        return limit;
    }
    let silence = rate / 5;
    let start = 25 * rate;
    samples[start..limit]
        .chunks(silence)
        .enumerate()
        .rev()
        .find(|(_, chunk)| chunk.iter().all(|s| s.abs() < 0.01))
        .map_or(limit, |(i, chunk)| start + i * silence + chunk.len() / 2)
}
fn transcribe(
    recognizer: &OfflineRecognizer,
    mut samples: &[f32],
    sample_rate: u32,
    cancel: &AtomicBool,
) -> Result<String> {
    let mut parts = Vec::new();
    while !samples.is_empty() {
        model::check_cancel(cancel)?;
        let count = segment_length(samples, sample_rate);
        let stream = recognizer.create_stream();
        stream.accept_waveform(sample_rate as i32, &samples[..count]);
        recognizer.decode(&stream);
        model::check_cancel(cancel)?;
        let result = stream
            .get_result()
            .context("Could not transcribe this recording")?;
        if !result.text.trim().is_empty() {
            parts.push(result.text.trim().to_owned());
        }
        samples = &samples[count..];
    }
    Ok(parts.join(" "))
}

/// Run on a worker thread; cannot remove a model currently being used.
pub fn remove_model(root: &std::path::Path) -> Result<()> {
    if BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        bail!("Finish or cancel dictation before removing the model");
    }
    let _lease = Lease;
    let _model_lock = model::lock(root)?;
    let directory = model::directory(root);
    if directory.exists() {
        std::fs::remove_dir_all(directory)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_audio_prefers_a_pause_and_always_advances() {
        let mut audio = vec![0.3; 45 * 16000];
        assert_eq!(segment_length(&audio, 16000), 30 * 16000);
        audio[28 * 16000..29 * 16000].fill(0.);
        let boundary = segment_length(&audio, 16000);
        assert!((28 * 16000..29 * 16000).contains(&boundary));
        assert_eq!(segment_length(&audio[..400], 16000), 400);
    }
    #[test]
    #[ignore = "requires GOOP_SPEECH_MODEL_ROOT and GOOP_SPEECH_WAV; never opens the microphone"]
    fn real_model_transcribes_fixture() {
        let root = PathBuf::from(std::env::var("GOOP_SPEECH_MODEL_ROOT").unwrap());
        let wav = sherpa_onnx::Wave::read(&std::env::var("GOOP_SPEECH_WAV").unwrap()).unwrap();
        let start = std::time::Instant::now();
        let recognizer = recognizer(&root).unwrap();
        eprintln!("Model load: {:?}", start.elapsed());
        let start = std::time::Instant::now();
        let text = transcribe(
            &recognizer,
            wav.samples(),
            wav.sample_rate() as u32,
            &AtomicBool::new(false),
        )
        .unwrap();
        eprintln!("Transcription: {:?}: {text}", start.elapsed());
        match std::env::var("GOOP_SPEECH_EXPECT") {
            Ok(expected) if expected.is_empty() => assert!(text.is_empty()),
            Ok(expected) => assert!(
                text.to_lowercase().contains(&expected.to_lowercase()),
                "{text}"
            ),
            Err(_) => assert!(!text.is_empty()),
        }
        assert!(
            transcribe(
                &recognizer,
                wav.samples(),
                wav.sample_rate() as u32,
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
}
