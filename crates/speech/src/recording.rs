use super::{Command, Event};
use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::{Duration, Instant},
};

const MAX_SECONDS: usize = 300;
struct Audio {
    samples: Vec<f32>,
    level: f32,
    error: Option<String>,
}

pub(super) fn capture(
    commands: &flume::Receiver<Command>,
    events: &flume::Sender<Event>,
    cancel: &AtomicBool,
) -> Result<(Vec<f32>, u32)> {
    let device = cpal::default_host()
        .default_input_device()
        .context("No microphone found. Connect a microphone and try again")?;
    let supported = device
        .default_input_config()
        .context("Could not access the microphone. Check Goop's microphone permission")?;
    let config: cpal::StreamConfig = supported.clone().into();
    if (config.sample_rate.0 < 8000 || config.sample_rate.0 > 192000) || config.channels == 0 {
        bail!("Unsupported microphone format");
    }
    let audio = Arc::new(Mutex::new(Audio {
        samples: Vec::new(),
        level: 0.,
        error: None,
    }));
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, audio.clone()),
        cpal::SampleFormat::F64 => build::<f64>(&device, &config, audio.clone()),
        cpal::SampleFormat::I8 => build::<i8>(&device, &config, audio.clone()),
        cpal::SampleFormat::I64 => build::<i64>(&device, &config, audio.clone()),
        cpal::SampleFormat::U8 => build::<u8>(&device, &config, audio.clone()),
        cpal::SampleFormat::U32 => build::<u32>(&device, &config, audio.clone()),
        cpal::SampleFormat::U64 => build::<u64>(&device, &config, audio.clone()),
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, audio.clone()),
        cpal::SampleFormat::I32 => build::<i32>(&device, &config, audio.clone()),
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, audio.clone()),
        _ => bail!("Unsupported microphone sample format"),
    }
    .context("Could not start the microphone. Check microphone permission and try again")?;
    stream.play().context("Could not start the microphone")?;
    let mut last_size = 0;
    let mut last_audio = Instant::now();
    loop {
        super::model::check_cancel(cancel)?;
        {
            let audio = audio.lock().unwrap();
            if audio.samples.len() != last_size {
                last_size = audio.samples.len();
                last_audio = Instant::now();
            }
            if audio.error.is_some() || last_audio.elapsed() > Duration::from_secs(5) {
                if audio.samples.len() >= (config.sample_rate.0 / 5) as usize {
                    events.send(Event::Notice(
                        "Microphone stopped. Transcribing the audio recorded so far".into(),
                    ))?;
                    break;
                }
                bail!("No audio from the microphone. Check microphone permission and try again");
            }
            let seconds = audio.samples.len() as u64 / config.sample_rate.0 as u64;
            events.send(Event::Recording {
                seconds,
                level: audio.level,
            })?;
            if seconds >= MAX_SECONDS as u64 {
                break;
            }
        }
        match commands.recv_timeout(Duration::from_millis(100)) {
            Ok(Command::Finish) => break,
            Ok(Command::Cancel) | Err(flume::RecvTimeoutError::Disconnected) => bail!("Cancelled"),
            _ => {}
        }
    }
    drop(stream);
    let samples = std::mem::take(&mut audio.lock().unwrap().samples);
    if samples.len() < (config.sample_rate.0 / 5) as usize {
        bail!("Recording was too short. Please try again");
    }
    Ok((samples, config.sample_rate.0))
}
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    audio: Arc<Mutex<Audio>>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let errors = audio.clone();
    let channels = config.channels as usize;
    let max = config.sample_rate.0 as usize * MAX_SECONDS;
    device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mut audio = audio.lock().unwrap();
            audio.append(data, channels, max);
        },
        move |error| {
            errors.lock().unwrap().error = Some(error.to_string());
        },
        None,
    )
}

impl Audio {
    fn append<T>(&mut self, data: &[T], channels: usize, max: usize)
    where
        T: cpal::SizedSample,
        f32: cpal::FromSample<T>,
    {
        let available = max.saturating_sub(self.samples.len());
        let mut peak = 0f32;
        for frame in data.chunks_exact(channels).take(available) {
            let sample = frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32;
            let sample = if sample.is_finite() {
                sample.clamp(-1., 1.)
            } else {
                0.
            };
            peak = peak.max(sample.abs());
            self.samples.push(sample);
        }
        self.level = peak;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn downmix_is_finite_normalized_and_bounded() {
        let mut audio = Audio {
            samples: Vec::new(),
            level: 0.,
            error: None,
        };
        audio.append(&[0.2f32, 0.6, f32::NAN, 0.5, 4., 2., 0.5, 0.5], 2, 3);
        assert_eq!(audio.samples, vec![0.4, 0., 1.]);
        assert_eq!(audio.level, 1.);
        audio.append(&[1f32; 200], 2, 3);
        assert_eq!(audio.samples.len(), 3);
        let mut pcm = Audio {
            samples: Vec::new(),
            level: 0.,
            error: None,
        };
        pcm.append(&[i16::MIN, i16::MAX], 1, 100);
        assert_eq!(pcm.samples[0], -1.);
        assert!(pcm.samples[1] > 0.99 && pcm.samples[1] <= 1.);
    }
}
