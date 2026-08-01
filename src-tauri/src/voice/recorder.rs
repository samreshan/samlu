//! Mic capture via `cpal`. The `cpal::Stream` lives and dies entirely on one
//! dedicated OS thread spawned per recording — it is never stored in
//! shared/async-managed state, since `cpal::Stream`'s `Send`-ness isn't
//! guaranteed portable across platforms. Start/stop is driven over
//! `std::sync::mpsc` channels; the encoded WAV bytes come back the same way.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MIN_CAPTURE_DURATION: Duration = Duration::from_millis(250);

/// Handle to an in-progress recording.
pub struct Recording {
    stop_tx: mpsc::Sender<()>,
    result_rx: mpsc::Receiver<Result<Vec<u8>, String>>,
    started_at: Instant,
}

impl Recording {
    /// Spawns the dedicated capture thread and starts recording immediately.
    /// Blocks only long enough to confirm the stream actually started (or to
    /// get the error if it couldn't) — the recording itself proceeds on the
    /// capture thread in the background.
    pub fn start<F>(on_level: F) -> Result<Self, String>
    where
        F: Fn(f32) + Send + Sync + 'static,
    {
        let (started_tx, started_rx) = mpsc::channel::<Result<(), String>>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (result_tx, result_rx) = mpsc::channel::<Result<Vec<u8>, String>>();
        let (level_tx, level_rx) = mpsc::sync_channel::<f32>(2);
        let reporter = Arc::new(LevelReporter::new(move |level| {
            let _ = level_tx.try_send(level);
        }));
        thread::spawn(move || {
            while let Ok(level) = level_rx.recv() {
                on_level(level);
            }
        });

        thread::spawn(move || {
            capture_thread(started_tx, stop_rx, result_tx, reporter);
        });

        started_rx
            .recv()
            .map_err(|_| "recording thread exited before starting".to_string())??;

        Ok(Self {
            stop_tx,
            result_rx,
            started_at: Instant::now(),
        })
    }

    /// Signals the capture thread to stop and blocks until the encoded WAV
    /// bytes (or an error) come back.
    pub fn stop(self) -> Result<Vec<u8>, String> {
        if let Some(remaining) = MIN_CAPTURE_DURATION.checked_sub(self.started_at.elapsed()) {
            thread::sleep(remaining);
        }
        log::info!("[voice] Recording::stop — signaling capture thread");
        let _ = self.stop_tx.send(());
        let outcome = self
            .result_rx
            .recv()
            .map_err(|_| "recording thread exited before finishing".to_string())?;
        log::info!(
            "[voice] Recording::stop — capture thread returned ({})",
            match &outcome {
                Ok(bytes) => format!("{} bytes", bytes.len()),
                Err(e) => format!("error: {e}"),
            }
        );
        outcome
    }

    pub fn cancel(self) {
        let _ = self.stop_tx.send(());
        let _ = self.result_rx.recv();
    }
}

struct LevelReporter {
    callback: Box<dyn Fn(f32) + Send + Sync>,
    last_emit_ms: Mutex<u128>,
}

impl LevelReporter {
    fn new<F>(callback: F) -> Self
    where
        F: Fn(f32) + Send + Sync + 'static,
    {
        Self {
            callback: Box::new(callback),
            last_emit_ms: Mutex::new(0),
        }
    }

    fn report(&self, samples: impl Iterator<Item = f32>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let mut last = self.last_emit_ms.lock().unwrap();
        if now.saturating_sub(*last) < 42 {
            return;
        }
        *last = now;
        drop(last);

        let mut sum = 0.0_f32;
        let mut count = 0_u32;
        for sample in samples {
            sum += sample * sample;
            count += 1;
        }
        if count == 0 {
            return;
        }
        let rms = (sum / count as f32).sqrt();
        let normalized = (rms * 5.5).clamp(0.015, 1.0);
        (self.callback)(normalized);
    }
}

fn capture_thread(
    started_tx: mpsc::Sender<Result<(), String>>,
    stop_rx: mpsc::Receiver<()>,
    result_tx: mpsc::Sender<Result<Vec<u8>, String>>,
    reporter: Arc<LevelReporter>,
) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => {
            let _ = started_tx.send(Err("no microphone input device found".into()));
            return;
        }
    };
    let config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            let _ = started_tx.send(Err(format!("couldn't read input config: {e}")));
            return;
        }
    };

    let sample_rate = config.sample_rate().0;
    let input_channels = config.channels();
    let sample_format = config.sample_format();

    let initial_capacity = usize::try_from(sample_rate).unwrap_or(48_000) * 10;
    let buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(initial_capacity)));
    let buffer_for_stream = buffer.clone();

    let err_fn = |err| log::error!("[voice] cpal stream error: {err}");

    let stream_result = match sample_format {
        cpal::SampleFormat::F32 => {
            let reporter = reporter.clone();
            device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    reporter.report(data.iter().copied());
                    append_mono(
                        &mut buffer_for_stream.lock().unwrap(),
                        data,
                        input_channels,
                        |sample| sample,
                    );
                },
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let reporter = reporter.clone();
            device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    reporter.report(data.iter().map(|sample| *sample as f32 / i16::MAX as f32));
                    let mut buf = buffer_for_stream.lock().unwrap();
                    append_mono(&mut buf, data, input_channels, |sample| {
                        sample as f32 / i16::MAX as f32
                    });
                },
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let reporter = reporter.clone();
            device.build_input_stream(
                &config.into(),
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    reporter.report(
                        data.iter()
                            .map(|sample| (*sample as f32 / u16::MAX as f32) * 2.0 - 1.0),
                    );
                    let mut buf = buffer_for_stream.lock().unwrap();
                    append_mono(&mut buf, data, input_channels, |sample| {
                        (sample as f32 / u16::MAX as f32) * 2.0 - 1.0
                    });
                },
                err_fn,
                None,
            )
        }
        other => {
            let _ = started_tx.send(Err(format!("unsupported input sample format: {other:?}")));
            return;
        }
    };

    let stream = match stream_result {
        Ok(s) => s,
        Err(e) => {
            let _ = started_tx.send(Err(format!("couldn't build input stream: {e}")));
            return;
        }
    };

    if let Err(e) = stream.play() {
        let _ = started_tx.send(Err(format!("couldn't start input stream: {e}")));
        return;
    }

    let _ = started_tx.send(Ok(()));
    log::info!("[voice] capture thread: stream playing, waiting for stop signal…");

    // Block this thread until told to stop — the stream keeps capturing into
    // `buffer` the whole time via its own callback, we're just waiting here.
    let _ = stop_rx.recv();
    log::info!("[voice] capture thread: stop received, dropping stream…");
    drop(stream); // stops capture
    log::info!("[voice] capture thread: stream dropped, encoding…");

    let samples = std::mem::take(&mut *buffer.lock().unwrap());
    let sample_count = samples.len();
    let wav = encode_wav(&samples, sample_rate, 1);
    log::info!("[voice] capture thread: encoded {sample_count} samples, sending result back");
    let _ = result_tx.send(wav);
}

/// Downmix at capture time instead of uploading duplicated stereo channels.
/// This halves the common Mac microphone buffer/upload size and avoids a second
/// full recording allocation when capture stops.
fn append_mono<T: Copy>(
    out: &mut Vec<f32>,
    input: &[T],
    channels: u16,
    convert: impl Fn(T) -> f32,
) {
    let channels = usize::from(channels.max(1));
    out.reserve(input.len().div_ceil(channels));
    for frame in input.chunks(channels) {
        let sum = frame.iter().copied().map(&convert).sum::<f32>();
        out.push(sum / frame.len() as f32);
    }
}

/// Encodes captured samples as 16-bit PCM WAV — a format Groq's
/// transcription endpoint accepts directly.
fn encode_wav(samples: &[f32], sample_rate: u32, channels: u16) -> Result<Vec<u8>, String> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).map_err(|e| e.to_string())?;
        for &sample in samples {
            let clamped = sample.clamp(-1.0, 1.0);
            writer
                .write_sample((clamped * i16::MAX as f32) as i16)
                .map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
    }
    Ok(cursor.into_inner())
}
