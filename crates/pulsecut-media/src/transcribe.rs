//! Local speech-to-text with whisper.cpp's `whisper-cli` (no cloud, no Python).
//! The model is only loaded per request (a process per clip) so nothing stays resident.

use crate::captions::Word;
use pulsecut_core::process::{Tools, run};
use pulsecut_core::{Error, Result};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static GPU_BROKEN: AtomicBool = AtomicBool::new(false);

/// Extract mono 16 kHz PCM WAV (what whisper.cpp expects) from any media file.
pub async fn extract_wav(tools: &Tools, input: &Path, start_ms: i64, dur_ms: i64, out: &Path) -> Result<()> {
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-v".into(), "error".into(), "-y".into()];
    if start_ms > 0 {
        args.extend(["-ss".into(), format!("{:.3}", start_ms as f64 / 1000.0)]);
    }
    args.extend(["-i".into(), input.to_string_lossy().to_string()]);
    if dur_ms > 0 {
        args.extend(["-t".into(), format!("{:.3}", dur_ms as f64 / 1000.0)]);
    }
    args.extend(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"].map(String::from));
    args.push(out.to_string_lossy().to_string());
    run(tools.ffmpeg()?, &args, Duration::from_secs(120)).await?;
    Ok(())
}

/// Parse whisper-cli `-ojf` output into words (sub-word tokens merged, special tokens dropped).
pub fn parse_whisper_json(json: &str) -> Result<Vec<Word>> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| Error::other(format!("whisper json: {e}")))?;
    let mut words: Vec<Word> = vec![];
    let segs = v
        .get("transcription")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    for seg in segs {
        for tok in seg.get("tokens").and_then(|t| t.as_array()).into_iter().flatten() {
            let text = tok.get("text").and_then(|t| t.as_str()).unwrap_or("");
            if text.starts_with("[_") || text.starts_with("[BLANK") || text.trim().is_empty() && !text.is_empty() {
                continue;
            }
            let from = tok.pointer("/offsets/from").and_then(|x| x.as_i64()).unwrap_or(0);
            let to = tok.pointer("/offsets/to").and_then(|x| x.as_i64()).unwrap_or(from);
            let p = tok.get("p").and_then(|x| x.as_f64()).unwrap_or(1.0) as f32;
            let starts_word = text.starts_with(' ') || words.is_empty();
            let pure_punct = text.chars().all(|c| !c.is_alphanumeric());
            if starts_word && !pure_punct {
                words.push(Word {
                    text: text.trim().to_string(),
                    start_ms: from,
                    end_ms: to.max(from),
                    prob: p,
                });
            } else if let Some(last) = words.last_mut() {
                last.text.push_str(text.trim());
                last.end_ms = to.max(last.end_ms);
                last.prob = last.prob.min(p);
            }
        }
    }
    // Drop non-speech annotations like "(music)" / "[laughs]" but keep them out of captions.
    words.retain(|w| !(w.text.starts_with('(') || w.text.starts_with('[') || w.text.starts_with('*')));
    // Enforce monotonic, non-overlapping timings.
    for i in 1..words.len() {
        if words[i].start_ms < words[i - 1].end_ms {
            words[i].start_ms = words[i - 1].end_ms;
        }
        if words[i].end_ms < words[i].start_ms {
            words[i].end_ms = words[i].start_ms + 80;
        }
    }
    Ok(words)
}

/// Run whisper-cli on a 16 kHz WAV. Tries Metal/GPU first, falls back to CPU (`-ng`) and
/// remembers the fallback for the rest of the process.
pub async fn transcribe(tools: &Tools, wav: &Path, threads: u32) -> Result<Vec<Word>> {
    let cli = tools.path(pulsecut_core::process::Tool::WhisperCli)?;
    let model = tools
        .whisper_model
        .as_deref()
        .ok_or_else(|| Error::other("no whisper model configured (Settings → Tools)"))?;
    let base = wav.with_extension("");
    let mut attempts = vec![false];
    if !GPU_BROKEN.load(Ordering::Relaxed) {
        attempts.insert(0, true);
    }
    let mut last_err = None;
    for gpu in attempts {
        let mut args: Vec<String> = vec![];
        if !gpu {
            args.push("-ng".into());
        }
        args.extend([
            "-m".into(),
            model.to_string_lossy().to_string(),
            "-f".into(),
            wav.to_string_lossy().to_string(),
            "-ojf".into(),
            "-of".into(),
            base.to_string_lossy().to_string(),
            "-np".into(),
            "-t".into(),
            threads.clamp(1, 8).to_string(),
        ]);
        match run(cli, &args, Duration::from_secs(300)).await {
            Ok(_) => {
                let json = std::fs::read_to_string(base.with_extension("json"))?;
                return parse_whisper_json(&json);
            }
            Err(e) => {
                if gpu {
                    tracing::warn!(
                        "whisper GPU run failed, falling back to CPU: {}",
                        e.to_string()
                            .lines()
                            .find(|l| l.contains("GGML") || l.contains("error"))
                            .unwrap_or("see debug log")
                    );
                    GPU_BROKEN.store(true, Ordering::Relaxed);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| Error::other("whisper failed")))
}

/// Normalize for phrase matching: lowercase alphanumerics and single spaces.
pub fn norm_phrase(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Voice-command matcher ("clip it", "clip that"); tolerant of common mishearings.
pub fn matches_command(transcript: &str, phrases: &[String]) -> bool {
    let t = format!(" {} ", norm_phrase(transcript));
    phrases.iter().any(|p| {
        let p = norm_phrase(p);
        if p.is_empty() {
            return false;
        }
        let variants = [
            p.clone(),
            p.replace("clip", "clipped"),
            p.replace("clip", "flip"),
            p.replace(' ', ""),
        ];
        variants.iter().any(|v| t.contains(&format!(" {v} ")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tokens_into_words() {
        let j = r#"{"transcription":[{"tokens":[
            {"text":"[_BEG_]","offsets":{"from":0,"to":0},"p":0.9},
            {"text":" And","offsets":{"from":320,"to":370},"p":0.7},
            {"text":" Amer","offsets":{"from":400,"to":600},"p":0.9},
            {"text":"icans","offsets":{"from":600,"to":900},"p":0.8},
            {"text":",","offsets":{"from":900,"to":910},"p":0.9},
            {"text":" (music)","offsets":{"from":1000,"to":1500},"p":0.9},
            {"text":"[_TT_550]","offsets":{"from":11000,"to":11000},"p":0.9}]}]}"#;
        let w = parse_whisper_json(j).unwrap();
        assert_eq!(w.len(), 2);
        assert_eq!(w[1].text, "Americans,");
        assert_eq!((w[1].start_ms, w[1].end_ms), (400, 910));
    }

    #[test]
    fn voice_commands() {
        let p = vec!["clip it".to_string(), "clip that".to_string()];
        assert!(matches_command("Oh my god, CLIP IT!", &p));
        assert!(matches_command("clip that please", &p));
        assert!(matches_command("flip it", &p));
        assert!(!matches_command("eclipse it", &p));
        assert!(!matches_command("we should clip the nails", &p));
    }
}
