use pulsecut_core::process::{Tools, run};
use pulsecut_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaInfo {
    pub duration_ms: i64,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub has_video: bool,
    pub has_audio: bool,
    pub video_codec: String,
    pub audio_sample_rate: u32,
    /// Container start time (ms); non-zero for some OBS/segment outputs.
    pub start_ms: i64,
}

fn parse_rate(s: &str) -> f32 {
    let mut it = s.split('/');
    let n: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let d: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    if d == 0.0 { 0.0 } else { n / d }
}

pub fn parse_ffprobe_json(json: &str) -> Result<MediaInfo> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| Error::other(format!("ffprobe json: {e}")))?;
    let mut info = MediaInfo::default();
    let secs = |x: &serde_json::Value| x.as_str().and_then(|s| s.parse::<f64>().ok());
    if let Some(d) = v.pointer("/format/duration").and_then(secs) {
        info.duration_ms = (d * 1000.0).round() as i64;
    }
    if let Some(s) = v.pointer("/format/start_time").and_then(secs) {
        info.start_ms = (s * 1000.0).round() as i64;
    }
    for s in v.get("streams").and_then(|s| s.as_array()).into_iter().flatten() {
        match s.get("codec_type").and_then(|c| c.as_str()) {
            Some("video") if !info.has_video => {
                // Skip attached cover art.
                if s.pointer("/disposition/attached_pic").and_then(|x| x.as_i64()) == Some(1) {
                    continue;
                }
                info.has_video = true;
                info.width = s.get("width").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                info.height = s.get("height").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                let r = s
                    .get("avg_frame_rate")
                    .and_then(|x| x.as_str())
                    .map(parse_rate)
                    .unwrap_or(0.0);
                info.fps = if r > 0.0 {
                    r
                } else {
                    s.get("r_frame_rate")
                        .and_then(|x| x.as_str())
                        .map(parse_rate)
                        .unwrap_or(30.0)
                };
                info.video_codec = s.get("codec_name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if info.duration_ms == 0
                    && let Some(d) = s.get("duration").and_then(secs)
                {
                    info.duration_ms = (d * 1000.0) as i64;
                }
            }
            Some("audio") if !info.has_audio => {
                info.has_audio = true;
                info.audio_sample_rate = s
                    .get("sample_rate")
                    .and_then(|x| x.as_str())
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(48_000);
            }
            _ => {}
        }
    }
    Ok(info)
}

pub async fn probe(tools: &Tools, path: &Path) -> Result<MediaInfo> {
    let args = vec![
        "-v".into(),
        "error".into(),
        "-print_format".into(),
        "json".into(),
        "-show_format".into(),
        "-show_streams".into(),
        path.to_string_lossy().to_string(),
    ];
    let out = run(tools.ffprobe()?, &args, Duration::from_secs(30)).await?;
    parse_ffprobe_json(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses() {
        let j = r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"60000/1001"},
                    {"codec_type":"audio","sample_rate":"48000"}],"format":{"duration":"31.500","start_time":"1.400"}}"#;
        let i = parse_ffprobe_json(j).unwrap();
        assert_eq!(i.duration_ms, 31_500);
        assert_eq!(i.start_ms, 1_400);
        assert!((i.fps - 59.94).abs() < 0.01);
        assert!(i.has_audio && i.has_video);
    }
}
