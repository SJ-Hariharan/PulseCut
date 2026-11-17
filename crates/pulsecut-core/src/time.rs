//! All PulseCut timestamps are Unix-epoch milliseconds (UTC wall clock). Media positions
//! are converted to wall clock via per-source anchors (see `pulsecut-capture::sync`).

use std::time::{SystemTime, UNIX_EPOCH};

pub type TsMs = i64;

pub fn now_ms() -> TsMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Format a millisecond duration as `m:ss.mmm` for logs and UI.
pub fn fmt_dur_ms(ms: i64) -> String {
    let neg = ms < 0;
    let ms = ms.abs();
    let m = ms / 60_000;
    let s = (ms % 60_000) / 1000;
    let r = ms % 1000;
    format!("{}{m}:{s:02}.{r:03}", if neg { "-" } else { "" })
}

pub fn fmt_local(ts: TsMs) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ts) {
        chrono::LocalResult::Single(t) => t.format("%Y-%m-%d %H:%M:%S").to_string(),
        _ => ts.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_durations() {
        assert_eq!(fmt_dur_ms(61_250), "1:01.250");
        assert_eq!(fmt_dur_ms(-500), "-0:00.500");
    }
}
