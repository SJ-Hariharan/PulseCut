//! H.264 encoder selection. Hardware encoders are only used after a real test encode succeeds
//! on this machine ("never claim hardware acceleration works without testing").

use pulsecut_core::Result;
use pulsecut_core::process::{Tools, run};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoEncoder {
    VideoToolbox,
    MediaFoundation,
    Nvenc,
    QuickSync,
    Amf,
    X264,
}

impl VideoEncoder {
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            VideoEncoder::VideoToolbox => "h264_videotoolbox",
            VideoEncoder::MediaFoundation => "h264_mf",
            VideoEncoder::Nvenc => "h264_nvenc",
            VideoEncoder::QuickSync => "h264_qsv",
            VideoEncoder::Amf => "h264_amf",
            VideoEncoder::X264 => "libx264",
        }
    }
    pub fn is_hardware(self) -> bool {
        self != VideoEncoder::X264
    }

    /// Encoder arguments targeting short-form quality (~10 Mb/s at 1080x1920).
    pub fn args(self, threads: u32) -> Vec<String> {
        let mut a: Vec<String> = vec!["-c:v".into(), self.ffmpeg_name().into()];
        match self {
            VideoEncoder::X264 => {
                a.extend(["-preset", "veryfast", "-crf", "20"].map(String::from));
                if threads > 0 {
                    a.extend(["-threads".to_string(), threads.to_string()]);
                }
            }
            VideoEncoder::VideoToolbox => {
                a.extend(
                    [
                        "-b:v",
                        "10M",
                        "-maxrate",
                        "14M",
                        "-bufsize",
                        "20M",
                        "-profile:v",
                        "high",
                    ]
                    .map(String::from),
                );
            }
            VideoEncoder::Nvenc => a.extend(["-preset", "p5", "-cq", "21", "-b:v", "0"].map(String::from)),
            VideoEncoder::QuickSync => a.extend(["-global_quality", "22"].map(String::from)),
            VideoEncoder::Amf | VideoEncoder::MediaFoundation => {
                a.extend(["-b:v", "10M"].map(String::from));
            }
        }
        a.extend(["-pix_fmt", "yuv420p"].map(String::from));
        a
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncoderProbe {
    pub encoder: VideoEncoder,
    pub ok: bool,
    pub ms: u128,
    pub error: Option<String>,
}

fn candidates() -> Vec<VideoEncoder> {
    if cfg!(target_os = "macos") {
        vec![VideoEncoder::VideoToolbox, VideoEncoder::X264]
    } else if cfg!(windows) {
        vec![
            VideoEncoder::Nvenc,
            VideoEncoder::QuickSync,
            VideoEncoder::Amf,
            VideoEncoder::MediaFoundation,
            VideoEncoder::X264,
        ]
    } else {
        vec![VideoEncoder::Nvenc, VideoEncoder::X264]
    }
}

/// Encode 2 s of 1080x1920 test video with each candidate; report timing and failures.
pub async fn probe_encoders(tools: &Tools) -> Result<Vec<EncoderProbe>> {
    let ffmpeg = tools.ffmpeg()?;
    let mut out = vec![];
    for enc in candidates() {
        let mut args: Vec<String> = [
            "-hide_banner",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1080x1920:rate=30:duration=2",
        ]
        .map(String::from)
        .to_vec();
        args.extend(enc.args(0));
        args.extend(["-f", "null", "-"].map(String::from));
        let t = Instant::now();
        let r = run(ffmpeg, &args, Duration::from_secs(60)).await;
        out.push(EncoderProbe {
            encoder: enc,
            ok: r.is_ok(),
            ms: t.elapsed().as_millis(),
            error: r.err().map(|e| e.to_string().chars().take(300).collect()),
        });
    }
    Ok(out)
}

/// Pick the encoder: the first hardware encoder that passed, else libx264.
pub fn choose(probes: &[EncoderProbe], prefer_hardware: bool) -> VideoEncoder {
    if prefer_hardware && let Some(p) = probes.iter().find(|p| p.ok && p.encoder.is_hardware()) {
        return p.encoder;
    }
    VideoEncoder::X264
}
