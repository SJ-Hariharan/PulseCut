//! Microphone: live audio features for the home stream's detector, and the "Clip it!" voice
//! command. Speech is segmented with a cheap energy VAD; only short utterances (≤ max_chunk_secs)
//! are sent to whisper.cpp, one at a time, so nothing heavy runs continuously.

use crate::Engine;
use pulsecut_core::{Result, TriggerSource};
use pulsecut_detect::AudioAnalyzer;
use pulsecut_media::transcribe;
use pulsecut_platform::mic::{self, MicChunk, MicStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

pub struct VoiceHandle {
    mic: MicStream,
    task: tokio::task::JoinHandle<()>,
}

/// Energy VAD over 30 ms frames with hangover; yields complete utterances.
pub struct Vad {
    frame: usize,
    floor_db: f32,
    in_speech: bool,
    silence_frames: usize,
    buf: Vec<f32>,
    max_samples: usize,
}

impl Vad {
    pub fn new(max_secs: f32) -> Self {
        Vad {
            frame: 480,
            floor_db: -55.0,
            in_speech: false,
            silence_frames: 0,
            buf: vec![],
            max_samples: (max_secs.clamp(1.0, 8.0) * 16_000.0) as usize,
        }
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<Vec<f32>> {
        let mut out = vec![];
        for f in samples.chunks(self.frame) {
            let db = 10.0 * (f.iter().map(|x| x * x).sum::<f32>() / f.len() as f32 + 1e-10).log10();
            if db < self.floor_db + 3.0 {
                self.floor_db = 0.9 * self.floor_db + 0.1 * db;
            } else {
                self.floor_db += 0.001 * (db - self.floor_db);
            }
            let voiced = db > self.floor_db + 12.0 && db > -48.0;
            if voiced {
                self.in_speech = true;
                self.silence_frames = 0;
            } else if self.in_speech {
                self.silence_frames += 1;
            }
            if self.in_speech {
                self.buf.extend_from_slice(f);
                let ended = self.silence_frames >= 12; // ~360 ms of silence
                if ended || self.buf.len() >= self.max_samples {
                    if self.buf.len() >= 16_000 * 4 / 10 {
                        out.push(std::mem::take(&mut self.buf));
                    } else {
                        self.buf.clear();
                    }
                    self.in_speech = false;
                    self.silence_frames = 0;
                }
            }
        }
        out
    }
}

pub fn start(e: &Arc<Engine>) -> Result<()> {
    let s = e.settings();
    let (tx, mut rx) = mpsc::channel::<MicChunk>(64);
    let mic = mic::start(&s.audio.mic_device, tx)?;
    e.log(format!("microphone active: {}", mic.device_name));
    let eng = Arc::clone(e);
    let task = tokio::spawn(async move {
        let mut analyzer: Option<AudioAnalyzer> = None;
        let mut vad = Vad::new(eng.settings().voice.max_chunk_secs);
        let busy = Arc::new(AtomicBool::new(false));
        while let Some(chunk) = rx.recv().await {
            let s = eng.settings();
            if s.audio.mic_enabled {
                let a = analyzer.get_or_insert_with(|| AudioAnalyzer::new(16_000, chunk.ts));
                for sec in a.push(&chunk.samples) {
                    let _ = eng.audio_tx.try_send((eng.home_stream(), sec));
                }
            }
            if !s.voice.enabled {
                continue;
            }
            for utt in vad.push(&chunk.samples) {
                if busy.swap(true, Ordering::SeqCst) {
                    continue; // one recognition at a time; drop the rest
                }
                let (e2, busy2, phrases) = (Arc::clone(&eng), Arc::clone(&busy), s.voice.phrases.clone());
                tokio::spawn(async move {
                    let heard = recognize(&e2, &utt).await;
                    busy2.store(false, Ordering::SeqCst);
                    match heard {
                        Ok(text) if transcribe::matches_command(&text, &phrases) => {
                            e2.log(format!("voice command heard: \"{}\"", text.trim()));
                            e2.clip_now(TriggerSource::Voice);
                        }
                        Ok(_) => {}
                        Err(err) => tracing::debug!("voice recognition: {err}"),
                    }
                });
            }
        }
    });
    *e.voice.lock().unwrap() = Some(VoiceHandle { mic, task });
    Ok(())
}

async fn recognize(e: &Arc<Engine>, samples: &[f32]) -> Result<String> {
    let dir = e.paths.work_dir().join("voice");
    std::fs::create_dir_all(&dir)?;
    let wav = dir.join(format!("utt_{}.wav", uuid::Uuid::new_v4().simple()));
    {
        let spec = pulsecut_media_wav_spec();
        let mut w = hound_writer(&wav, spec)?;
        for s in samples {
            w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)
                .map_err(|x| pulsecut_core::Error::other(x.to_string()))?;
        }
        w.finalize().map_err(|x| pulsecut_core::Error::other(x.to_string()))?;
    }
    // Prefer the tiny model for commands when present (faster), else the configured one.
    let mut tools = e.tools.clone();
    for d in [e.paths.models_dir(), std::path::PathBuf::from("models")] {
        let tiny = d.join("ggml-tiny.en.bin");
        if tiny.is_file() {
            tools.whisper_model = Some(tiny);
            break;
        }
    }
    let words = transcribe::transcribe(&tools, &wav, 2).await;
    let _ = std::fs::remove_file(&wav);
    let _ = std::fs::remove_file(wav.with_extension("json"));
    Ok(words?.into_iter().map(|w| w.text).collect::<Vec<_>>().join(" "))
}

fn pulsecut_media_wav_spec() -> hound::WavSpec {
    hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }
}

fn hound_writer(
    p: &std::path::Path,
    spec: hound::WavSpec,
) -> Result<hound::WavWriter<std::io::BufWriter<std::fs::File>>> {
    hound::WavWriter::create(p, spec).map_err(|x| pulsecut_core::Error::other(x.to_string()))
}

pub fn stop(e: &Arc<Engine>) {
    stop_inner(e);
}

pub(crate) fn stop_inner(e: &Engine) {
    if let Some(v) = e.voice.lock().unwrap().take() {
        v.mic.stop();
        v.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vad_segments_utterances() {
        let mut v = Vad::new(3.0);
        let silence = vec![0.0005f32; 16_000];
        let speech: Vec<f32> = (0..12_000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        assert!(v.push(&silence).is_empty());
        let mut got = v.push(&speech);
        got.extend(v.push(&silence));
        assert_eq!(got.len(), 1);
        assert!(got[0].len() >= 12_000);
        // Long monologue is cut at max length.
        let long: Vec<f32> = (0..16_000 * 5).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let cuts = v.push(&long);
        assert!(!cuts.is_empty() && cuts[0].len() <= 16_000 * 3 + 480);
    }
}
