//! PulseCut-managed rolling buffer: one FFmpeg process writes fixed-length MPEG-TS segments
//! in a ring (`-segment_wrap`) and a CSV segment list; a second output streams 16 kHz mono PCM
//! on stdout for live audio analysis.
//!
//! Wall-clock anchoring: each CSV line reports a segment's end in stream time; we observe it at
//! wall time W, so `offset = min(W − end)` over recent segments (see `sync::ClockAnchor`).
//! Extraction concatenates *whole* segments covering the window (stream copy, no keyframe
//! surprises) and reports the exact covered span; the renderer trims frame-accurately.

use crate::sync::ClockAnchor;
use crate::{CapturedFile, coverage_for};
use pulsecut_core::process::{Tools, hide_console_window, run};
use pulsecut_core::{Error, Result, TsMs, now_ms};
use pulsecut_detect::audio::{AudioAnalyzer, AudioSecond, s16le_to_f32};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone)]
pub struct SegmentConfig {
    /// FFmpeg input. Special forms: `lavfi:test` (moving test pattern + tone, real time),
    /// `file:<path>` (played in real time with -re). Anything else is passed to `-i` as-is
    /// (rtmp://, srt://, https://...m3u8, avfoundation/dshow devices via `device:` prefix).
    pub input: String,
    pub dir: PathBuf,
    pub segment_secs: u32,
    pub ring_secs: u32,
    pub copy_codec: bool,
    pub audio_analysis: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub file: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SegmentStatus {
    pub running: bool,
    pub segments: usize,
    pub buffered_ms: i64,
    pub anchor_jitter_ms: i64,
    pub last_error: Option<String>,
    pub restarts: u32,
}

struct Shared {
    segments: VecDeque<Segment>,
    anchor: ClockAnchor,
}

pub struct SegmentRecorder {
    cfg: SegmentConfig,
    tools: Tools,
    shared: Arc<Mutex<Shared>>,
    status_tx: watch::Sender<SegmentStatus>,
    stop_tx: watch::Sender<bool>,
}

/// Parse `-segment_list_type csv` lines: `file,start,end`.
pub fn parse_csv(text: &str) -> Vec<Segment> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.rsplitn(3, ',');
            let end: f64 = it.next()?.trim().parse().ok()?;
            let start: f64 = it.next()?.trim().parse().ok()?;
            let file = it.next()?.trim().trim_matches('"').to_string();
            Some(Segment {
                file,
                start_ms: (start * 1000.0).round() as i64,
                end_ms: (end * 1000.0).round() as i64,
            })
        })
        .collect()
}

fn input_args(input: &str) -> Vec<String> {
    let v: Vec<&str> = if input == "lavfi:test" {
        return [
            "-re",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=30",
            "-re",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=330:sample_rate=48000:beep_factor=4",
        ]
        .map(String::from)
        .to_vec();
    } else if let Some(p) = input.strip_prefix("file:") {
        return vec!["-re".into(), "-stream_loop".into(), "-1".into(), "-i".into(), p.into()];
    } else if let Some(d) = input.strip_prefix("device:") {
        let fmt = if cfg!(target_os = "macos") {
            "avfoundation"
        } else if cfg!(windows) {
            "dshow"
        } else {
            "v4l2"
        };
        return vec!["-f".into(), fmt.into(), "-i".into(), d.into()];
    } else {
        vec![
            "-reconnect",
            "1",
            "-reconnect_streamed",
            "1",
            "-reconnect_delay_max",
            "5",
            "-i",
        ]
    };
    let mut out: Vec<String> = if input.starts_with("http") {
        v.iter().map(|s| s.to_string()).collect()
    } else {
        vec!["-i".into()]
    };
    out.push(input.into());
    out
}

/// Page URL for a `twitch:<channel>` or `youtube:<url|video id>` input; `None` for plain inputs.
pub fn live_page_url(input: &str) -> Option<String> {
    if let Some(ch) = input.strip_prefix("twitch:") {
        let ch = ch.trim().trim_start_matches('#').to_ascii_lowercase();
        return (!ch.is_empty()).then(|| format!("https://www.twitch.tv/{ch}"));
    }
    if let Some(v) = input.strip_prefix("youtube:") {
        let v = v.trim();
        if v.is_empty() {
            return None;
        }
        return Some(if v.starts_with("http") {
            v.to_string()
        } else if v.starts_with('@') {
            format!("https://www.youtube.com/{v}/live")
        } else {
            format!("https://www.youtube.com/watch?v={v}")
        });
    }
    None
}

/// Resolve platform inputs to a direct HLS URL with `yt-dlp -g` (best quality up to 1080p).
async fn resolve_live_input(input: &str) -> std::result::Result<String, String> {
    let Some(page) = live_page_url(input) else {
        return Ok(input.to_string());
    };
    let exe = ["/opt/homebrew/bin/yt-dlp", "/usr/local/bin/yt-dlp"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .unwrap_or("yt-dlp");
    let out = Command::new(exe)
        .args(["-g", "--no-warnings", "-f", "best[height<=1080]/best", &page])
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = match tokio::time::timeout(Duration::from_secs(45), out).await {
        Err(_) => return Err(format!("{input}: yt-dlp timed out")),
        Ok(Err(e)) => return Err(format!("{input}: yt-dlp not available ({e}); install yt-dlp")),
        Ok(Ok(o)) => o,
    };
    let url = String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.starts_with("http"))
        .map(str::to_string);
    match url {
        Some(u) if out.status.success() => Ok(u),
        _ => {
            let err = String::from_utf8_lossy(&out.stderr);
            let line = err.lines().last().unwrap_or("not live");
            Err(format!(
                "{input}: offline or unavailable ({})",
                line.chars().take(160).collect::<String>()
            ))
        }
    }
}

impl SegmentRecorder {
    pub fn spawn(
        cfg: SegmentConfig,
        tools: Tools,
        audio_tx: Option<mpsc::Sender<AudioSecond>>,
    ) -> Result<Arc<SegmentRecorder>> {
        std::fs::create_dir_all(&cfg.dir)?;
        tools.ffmpeg()?;
        let (status_tx, _) = watch::channel(SegmentStatus::default());
        let (stop_tx, _) = watch::channel(false);
        let me = Arc::new(SegmentRecorder {
            shared: Arc::new(Mutex::new(Shared {
                segments: VecDeque::new(),
                anchor: ClockAnchor::new(30, 250),
            })),
            cfg,
            tools,
            status_tx,
            stop_tx,
        });
        let m = Arc::clone(&me);
        tokio::spawn(async move { m.supervise(audio_tx).await });
        Ok(me)
    }

    pub fn status(&self) -> watch::Receiver<SegmentStatus> {
        self.status_tx.subscribe()
    }

    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }

    fn ring_count(&self) -> u32 {
        (self.cfg.ring_secs / self.cfg.segment_secs.max(1)).max(4) + 2
    }

    fn build_args(&self, input: &str) -> Vec<String> {
        let c = &self.cfg;
        let mut a: Vec<String> = vec!["-hide_banner".into(), "-nostats".into(), "-v".into(), "error".into()];
        a.extend(input_args(input));
        let test = c.input == "lavfi:test";
        let (vmap, amap) = if test { ("0:v:0", "1:a:0") } else { ("0:v:0", "0:a:0?") };
        a.extend(["-map".into(), vmap.into(), "-map".into(), amap.into()]);
        if c.copy_codec && !test {
            a.extend(["-c".into(), "copy".into()]);
        } else {
            let gop = (30 * c.segment_secs).to_string();
            a.extend(
                [
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                    "-tune",
                    "zerolatency",
                    "-crf",
                    "21",
                    "-g",
                    &gop,
                    "-sc_threshold",
                    "0",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "160k",
                ]
                .map(String::from),
            );
        }
        a.extend([
            "-f".into(),
            "segment".into(),
            "-segment_time".into(),
            c.segment_secs.to_string(),
            "-segment_wrap".into(),
            self.ring_count().to_string(),
            "-segment_list".into(),
            c.dir.join("segments.csv").to_string_lossy().to_string(),
            "-segment_list_type".into(),
            "csv".into(),
            "-segment_list_size".into(),
            self.ring_count().to_string(),
            "-reset_timestamps".into(),
            "1".into(),
            "-segment_format".into(),
            "mpegts".into(),
            c.dir.join("seg_%05d.ts").to_string_lossy().to_string(),
        ]);
        if c.audio_analysis {
            let amap = if test { "1:a:0" } else { "0:a:0?" };
            a.extend(["-map", amap, "-vn", "-ac", "1", "-ar", "16000", "-f", "s16le", "pipe:1"].map(String::from));
        }
        a
    }

    async fn supervise(self: Arc<Self>, audio_tx: Option<mpsc::Sender<AudioSecond>>) {
        let mut stop = self.stop_tx.subscribe();
        let mut restarts = 0u32;
        loop {
            if *stop.borrow() {
                break;
            }
            let _ = std::fs::remove_file(self.cfg.dir.join("segments.csv"));
            {
                let mut sh = self.shared.lock().unwrap();
                sh.segments.clear();
                sh.anchor = ClockAnchor::new(30, 250);
            }
            // `twitch:<channel>` / `youtube:<url>` inputs are resolved to a fresh HLS URL on every
            // (re)start, because platform stream URLs are signed and expire.
            let input = match resolve_live_input(&self.cfg.input).await {
                Ok(i) => i,
                Err(e) => {
                    self.status_tx.send_modify(|s| s.last_error = Some(e));
                    // Offline or not resolvable yet: check again shortly without hammering.
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                        _ = stop.changed() => {}
                    }
                    continue;
                }
            };
            let mut cmd = Command::new(self.tools.ffmpeg().expect("checked at spawn"));
            cmd.args(self.build_args(&input))
                .stdin(Stdio::null())
                .stdout(if self.cfg.audio_analysis {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            hide_console_window(&mut cmd);
            let mut child: Child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    self.status_tx
                        .send_modify(|s| s.last_error = Some(format!("ffmpeg spawn: {e}")));
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            self.status_tx.send_modify(|s| {
                s.running = true;
                s.restarts = restarts;
            });
            let stdout = child.stdout.take();
            let mut stderr = child.stderr.take();
            let shared = Arc::clone(&self.shared);
            let atx = audio_tx.clone();
            let audio_task = tokio::spawn(async move {
                let (Some(mut out), Some(tx)) = (stdout, atx) else {
                    return;
                };
                let mut buf = vec![0u8; 3200]; // 100 ms
                let mut analyzer: Option<AudioAnalyzer> = None;
                let mut consumed: i64 = 0;
                let mut carry: Vec<u8> = vec![];
                loop {
                    let n = match out.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    carry.extend_from_slice(&buf[..n]);
                    let even = carry.len() & !1;
                    let samples = s16le_to_f32(&carry[..even]);
                    carry.drain(..even);
                    let start_media_ms = consumed * 1000 / 16_000;
                    consumed += samples.len() as i64;
                    if analyzer.is_none() {
                        // Wait for the clock anchor so audio seconds land on wall-clock seconds.
                        let off = shared.lock().unwrap().anchor.offset();
                        if let Some(off) = off {
                            analyzer = Some(AudioAnalyzer::new(16_000, start_media_ms + off));
                        } else {
                            continue;
                        }
                    }
                    for sec in analyzer.as_mut().unwrap().push(&samples) {
                        if tx.try_send(sec).is_err() {
                            // Receiver slow or gone: drop (bounded channel, never block capture).
                        }
                    }
                }
            });
            let err_task = tokio::spawn(async move {
                let mut s = String::new();
                if let Some(e) = stderr.as_mut() {
                    let _ = e.read_to_string(&mut s).await;
                }
                s
            });

            // Poll the CSV list for finished segments.
            let mut poll = tokio::time::interval(Duration::from_millis(250));
            let exit = loop {
                tokio::select! {
                    _ = poll.tick() => self.poll_csv(),
                    st = child.wait() => break st.ok(),
                    _ = stop.changed() => { let _ = child.kill().await; break None; }
                }
            };
            audio_task.abort();
            let err = err_task.await.unwrap_or_default();
            self.status_tx.send_modify(|s| {
                s.running = false;
                if !err.trim().is_empty() {
                    s.last_error = Some(pulsecut_core::process::tail(err.trim(), 600));
                }
            });
            if *stop.borrow() {
                break;
            }
            restarts += 1;
            tracing::warn!("segment recorder exited ({exit:?}); restarting: {}", err.trim());
            tokio::time::sleep(Duration::from_secs((2u64 << restarts.min(4)).min(30))).await;
        }
    }

    fn poll_csv(&self) {
        let Ok(text) = std::fs::read_to_string(self.cfg.dir.join("segments.csv")) else {
            return;
        };
        let wall = now_ms();
        let entries = parse_csv(&text);
        let mut sh = self.shared.lock().unwrap();
        for seg in entries {
            let known = sh
                .segments
                .iter()
                .any(|s| s.start_ms == seg.start_ms && s.file == seg.file);
            if known {
                continue;
            }
            // A newly finished segment: anchor on its end, and drop any older entry that
            // used the same (wrapped) file name — that file has been overwritten.
            sh.anchor.observe(seg.end_ms, wall);
            sh.segments.retain(|s| s.file != seg.file);
            sh.segments.push_back(seg);
        }
        let n = sh.segments.len();
        let buffered = match (sh.segments.front(), sh.segments.back()) {
            (Some(a), Some(b)) => b.end_ms - a.start_ms,
            _ => 0,
        };
        let jitter = sh.anchor.jitter_ms();
        drop(sh);
        self.status_tx.send_modify(|s| {
            s.segments = n;
            s.buffered_ms = buffered;
            s.anchor_jitter_ms = jitter;
        });
    }

    /// Latest wall-clock instant fully written to disk.
    pub fn buffered_until(&self) -> Option<TsMs> {
        let sh = self.shared.lock().unwrap();
        sh.segments.back().and_then(|s| sh.anchor.to_wall(s.end_ms))
    }

    /// Concatenate the segments covering `[from, to]` (wall clock) into `out` (stream copy).
    pub async fn extract(&self, from: TsMs, to: TsMs, moment: TsMs, out: &Path) -> Result<CapturedFile> {
        let (list, file_start, dur, uncertainty) = {
            let sh = self.shared.lock().unwrap();
            let off = sh
                .anchor
                .offset()
                .ok_or_else(|| Error::other("segment buffer not anchored yet"))?;
            let (mf, mt) = (from - off, to - off);
            let mut chosen: Vec<&Segment> = sh
                .segments
                .iter()
                .filter(|s| s.end_ms > mf && s.start_ms < mt)
                .collect();
            chosen.sort_by_key(|s| s.start_ms);
            if chosen.is_empty() {
                return Err(Error::CaptureGap {
                    want_start: from,
                    want_end: to,
                    have_start: sh.segments.front().map(|s| s.start_ms + off).unwrap_or(0),
                    have_end: sh.segments.back().map(|s| s.end_ms + off).unwrap_or(0),
                });
            }
            // Only contiguous runs are valid; stop at the first gap (e.g. a restart).
            let mut run: Vec<&Segment> = vec![chosen[0]];
            for s in &chosen[1..] {
                if s.start_ms - run.last().unwrap().end_ms > 200 {
                    break;
                }
                run.push(s);
            }
            let mut list = String::from("ffconcat version 1.0\n");
            for s in &run {
                list.push_str(&format!("file '{}'\n", self.cfg.dir.join(&s.file).display()));
            }
            let first = run[0].start_ms;
            let last = run.last().unwrap().end_ms;
            (
                list,
                first + off,
                last - first,
                sh.anchor.resolution_ms + sh.anchor.jitter_ms().min(1000),
            )
        };
        let list_path = out.with_extension("ffconcat");
        std::fs::write(&list_path, list)?;
        let args: Vec<String> = vec![
            "-hide_banner".into(),
            "-v".into(),
            "error".into(),
            "-y".into(),
            "-f".into(),
            "concat".into(),
            "-safe".into(),
            "0".into(),
            "-i".into(),
            list_path.to_string_lossy().to_string(),
            "-map".into(),
            "0".into(),
            "-c".into(),
            "copy".into(),
            out.to_string_lossy().to_string(),
        ];
        run(self.tools.ffmpeg()?, &args, Duration::from_secs(120)).await?;
        let _ = std::fs::remove_file(&list_path);
        Ok(CapturedFile {
            path: out.to_path_buf(),
            file_start_ts: file_start,
            duration_ms: dur,
            coverage: coverage_for(file_start, dur, from, to, moment),
            sync_uncertainty_ms: uncertainty,
            backend: "segment-buffer".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_segment_csv() {
        let s = parse_csv("seg_00000.ts,0.000000,2.000000\nseg_00001.ts,2.000000,4.033333\n\"a,b.ts\",4.0,6.0\n");
        assert_eq!(s.len(), 3);
        assert_eq!(s[1].end_ms, 4033);
        assert_eq!(s[2].file, "a,b.ts");
    }

    #[test]
    fn input_forms() {
        assert!(input_args("lavfi:test").contains(&"testsrc2=size=1280x720:rate=30".to_string()));
        assert_eq!(input_args("file:/x.mp4")[0], "-re");
        assert!(input_args("https://x/y.m3u8").contains(&"-reconnect".to_string()));
        assert_eq!(
            live_page_url("twitch:#Foo").as_deref(),
            Some("https://www.twitch.tv/foo")
        );
        assert_eq!(
            live_page_url("youtube:@chan").as_deref(),
            Some("https://www.youtube.com/@chan/live")
        );
        assert_eq!(
            live_page_url("youtube:abc").as_deref(),
            Some("https://www.youtube.com/watch?v=abc")
        );
        assert!(live_page_url("https://x/y.m3u8").is_none());
        assert_eq!(
            input_args("rtmp://127.0.0.1/live"),
            vec!["-i".to_string(), "rtmp://127.0.0.1/live".to_string()]
        );
    }
}
