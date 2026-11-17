//! Microphone capture → 16 kHz mono f32 chunks. Opt-in only (Settings → Audio); macOS asks for
//! microphone permission the first time the stream opens.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use pulsecut_core::{Error, Result, TsMs, now_ms};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

pub const TARGET_SR: u32 = 16_000;

#[derive(Debug, Clone)]
pub struct MicChunk {
    /// Wall-clock time of the first sample.
    pub ts: TsMs,
    pub samples: Vec<f32>,
}

pub fn list_inputs() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|it| {
            it.filter_map(|d| d.description().ok().map(|x| x.name().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Linear resampler from `from` Hz to 16 kHz, carrying fractional position across calls.
pub struct Resampler {
    step: f64,
    pos: f64,
    last: f32,
}

impl Resampler {
    pub fn new(from: u32) -> Self {
        Resampler {
            step: from as f64 / TARGET_SR as f64,
            pos: -1.0,
            last: 0.0,
        }
    }
    /// `pos` is measured in input samples relative to the current chunk; index -1 is the
    /// last sample of the previous chunk, so interpolation is continuous across chunks.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let n = input.len() as f64;
        let at = |i: isize| if i < 0 { self.last } else { input[i as usize] };
        let mut out = Vec::with_capacity((n / self.step) as usize + 1);
        // Need floor(pos)+1 < len to interpolate.
        while self.pos + 1.0 < n {
            let i = self.pos.floor() as isize;
            let frac = (self.pos - i as f64) as f32;
            let (x0, x1) = (at(i), at(i + 1));
            out.push(x0 + (x1 - x0) * frac);
            self.pos += self.step;
        }
        self.pos -= n;
        if let Some(l) = input.last() {
            self.last = *l;
        }
        out
    }
}

pub struct MicStream {
    stop: Arc<AtomicBool>,
    pub device_name: String,
}

impl MicStream {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for MicStream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Open `device` (empty = default) on a dedicated thread (cpal streams are not `Send` on every
/// platform) and deliver ~100 ms chunks.
pub fn start(device: &str, tx: mpsc::Sender<MicChunk>) -> Result<MicStream> {
    let host = cpal::default_host();
    let dev = if device.is_empty() {
        host.default_input_device()
    } else {
        host.input_devices()
            .map_err(|e| Error::other(e.to_string()))?
            .find(|d| d.description().map(|x| x.name() == device).unwrap_or(false))
    }
    .ok_or_else(|| Error::other(format!("microphone '{device}' not found")))?;
    let name = dev.description().map(|d| d.name().to_string()).unwrap_or_default();
    let cfg = dev.default_input_config().map_err(|e| Error::other(e.to_string()))?;
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
    std::thread::Builder::new()
        .name("pulsecut-mic".into())
        .spawn(move || {
            let channels = cfg.channels() as usize;
            let mut rs = Resampler::new(cfg.sample_rate());
            let mut pending: Vec<f32> = Vec::with_capacity(4000);
            let mut pending_ts: Option<TsMs> = None;
            let fmt = cfg.sample_format();
            let mut on_data = move |mono: Vec<f32>| {
                if pending_ts.is_none() {
                    pending_ts = Some(now_ms() - (mono.len() as i64 * 1000 / TARGET_SR as i64));
                }
                pending.extend(rs.process(&mono));
                if pending.len() >= 1600 {
                    let chunk = MicChunk {
                        ts: pending_ts.take().unwrap_or_else(now_ms),
                        samples: std::mem::take(&mut pending),
                    };
                    let _ = tx.try_send(chunk);
                }
            };
            let err_fn = |e| tracing::warn!("microphone stream error: {e}");
            let stream = match fmt {
                cpal::SampleFormat::F32 => dev.build_input_stream::<f32, _, _>(
                    cfg.config(),
                    move |d, _| {
                        on_data(
                            d.chunks(channels)
                                .map(|f| f.iter().sum::<f32>() / channels as f32)
                                .collect(),
                        )
                    },
                    err_fn,
                    None,
                ),
                cpal::SampleFormat::I16 => dev.build_input_stream::<i16, _, _>(
                    cfg.config(),
                    move |d, _| {
                        on_data(
                            d.chunks(channels)
                                .map(|f| f.iter().map(|s| *s as f32 / 32768.0).sum::<f32>() / channels as f32)
                                .collect(),
                        )
                    },
                    err_fn,
                    None,
                ),
                other => {
                    let _ = ready_tx.send(Err(format!("unsupported sample format {other:?}")));
                    return;
                }
            };
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(e.to_string()));
                return;
            }
            let _ = ready_tx.send(Ok(()));
            while !stop2.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            drop(stream);
        })
        .map_err(|e| Error::other(e.to_string()))?;
    match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(MicStream {
            stop,
            device_name: name,
        }),
        Ok(Err(e)) => Err(Error::other(format!(
            "could not open microphone: {e} (on macOS grant access in System Settings → Privacy & Security → Microphone)"
        ))),
        Err(_) => Err(Error::other("microphone did not start within 5 s")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resampler_rate_and_continuity() {
        let mut r = Resampler::new(48_000);
        let input: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin())
            .collect();
        let mut out = vec![];
        for c in input.chunks(480) {
            out.extend(r.process(c));
        }
        assert!((out.len() as i64 - 16_000).abs() <= 2, "{}", out.len());
        // No discontinuity spikes at chunk boundaries.
        let max_step = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(max_step < 0.2, "{max_step}");
    }
}
