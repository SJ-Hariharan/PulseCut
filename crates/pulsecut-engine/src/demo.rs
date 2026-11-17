//! Offline demo / acceptance harness.
//!
//! A synthetic "livestream" video (test pattern, real speech and an impact sound) and a
//! synthetic chat audience that reacts to it in real time, so the full pipeline can be
//! exercised without going live.

use crate::Engine;
use pulsecut_core::process::{Tools, run};
use pulsecut_core::{MomentKind, Result, TsMs, now_ms};
use pulsecut_detect::synth::{MomentSpec, Scenario, generate};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub struct DemoStream {
    pub path: PathBuf,
    pub duration_secs: u32,
    /// Second (in the video) where the "moment" happens.
    pub moment_secs: u32,
    pub has_speech: bool,
}

/// Locate whisper.cpp's bundled JFK sample (public-domain speech) for caption testing.
pub fn find_speech_sample() -> Option<PathBuf> {
    let candidates = [
        "/opt/homebrew/share/whisper.cpp/jfk.wav",
        "/usr/local/share/whisper.cpp/jfk.wav",
    ];
    for c in candidates {
        if Path::new(c).is_file() {
            return Some(c.into());
        }
    }
    std::fs::read_dir("/opt/homebrew/Cellar/whisper.cpp")
        .ok()?
        .flatten()
        .map(|d| d.path().join("share/whisper.cpp/jfk.wav"))
        .find(|p| p.is_file())
}

/// Build a 75 s 1280x720 "stream": moving test pattern, quiet room noise, speech starting at
/// 33 s, and a loud impact at 40 s (the moment).
pub async fn make_demo_stream(tools: &Tools, out: &Path) -> Result<DemoStream> {
    let speech = find_speech_sample();
    let (dur, moment) = (75u32, 40u32);
    let mut args: Vec<String> = ["-hide_banner", "-v", "error", "-y", "-f", "lavfi", "-i"]
        .map(String::from)
        .to_vec();
    args.push(format!("testsrc2=size=1280x720:rate=30:duration={dur}"));
    args.extend(["-f".into(), "lavfi".into(), "-i".into()]);
    args.push(format!(
        "anoisesrc=color=pink:amplitude=0.015:duration={dur}:sample_rate=48000"
    ));
    args.extend(["-f".into(), "lavfi".into(), "-i".into()]);
    args.push(format!(
        "aevalsrc='0.9*sin(2*PI*70*t)*exp(-3*(t-{m}))*gte(t,{m})*lt(t,{m}+1.2)+0.5*sin(2*PI*140*t)*exp(-6*(t-{m}))*gte(t,{m})*lt(t,{m}+0.6)':s=48000:d={dur}",
        m = moment
    ));
    let filter = if let Some(sp) = &speech {
        args.extend(["-i".into(), sp.display().to_string()]);
        "[3:a]aresample=48000,adelay=33000|33000,volume=1.6[sp];[1:a][2:a][sp]amix=inputs=3:normalize=0:duration=first[a]"
    } else {
        "[1:a][2:a]amix=inputs=2:normalize=0:duration=first[a]"
    };
    args.extend(["-filter_complex".into(), filter.into()]);
    args.extend(
        [
            "-map", "0:v", "-map", "[a]", "-c:v", "libx264", "-preset", "veryfast", "-g", "60", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-b:a", "160k", "-t",
        ]
        .map(String::from),
    );
    args.push(dur.to_string());
    args.push(out.display().to_string());
    run(tools.ffmpeg()?, &args, Duration::from_secs(180)).await?;
    Ok(DemoStream {
        path: out.to_path_buf(),
        duration_secs: dur,
        moment_secs: moment,
        has_speech: speech.is_some(),
    })
}

/// Feed a synthetic audience into `stream_id`: steady chatter from `start` plus a reaction burst
/// to a moment at wall time `moment` (chat reacts ~6 s later, like real viewers).
pub fn replay_chat(
    engine: Arc<Engine>,
    stream_id: String,
    start: TsMs,
    moment: TsMs,
    end: TsMs,
    kind: MomentKind,
) -> tokio::task::JoinHandle<usize> {
    let sc = Scenario {
        name: "demo".into(),
        seed: 4242,
        duration_secs: (end - start) / 1000,
        base_rate: 2.0,
        population: 300,
        moments: vec![MomentSpec {
            at_sec: (moment - start) / 1000,
            kind,
            intensity: 7.0,
            duration_secs: 7,
            audio_spike: false,
            laugh_audio: false,
        }],
        distractors: vec![],
        reaction_delay_ms: 6_000,
        audio: false,
        start_ms: start,
    };
    let data = generate(&sc);
    tokio::spawn(async move {
        let mut n = 0;
        for m in data.messages {
            let wait = m.ts - now_ms();
            if wait > 0 {
                tokio::time::sleep(Duration::from_millis(wait as u64)).await;
            }
            engine.inject_chat(&stream_id, m);
            n += 1;
        }
        n
    })
}
