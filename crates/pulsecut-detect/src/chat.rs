//! Chat ingestion: spam suppression and per-second feature bins.

use crate::text::{self, Category, N_CATEGORIES};
use pulsecut_core::TsMs;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};

/// A platform-neutral chat message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Server timestamp (ms) when available, else receive time.
    pub ts: TsMs,
    pub user: String,
    pub text: String,
    /// Emote names the platform tagged in this message.
    #[serde(default)]
    pub emotes: Vec<String>,
    #[serde(default)]
    pub is_mod: bool,
    #[serde(default)]
    pub is_subscriber: bool,
}

/// Aggregated features for one second of chat.
#[derive(Debug, Clone, Default)]
pub struct ChatBin {
    /// Messages after spam suppression.
    pub msgs: u32,
    /// Messages dropped as spam / bot / low-signal.
    pub suppressed: u32,
    pub users: Vec<u64>,
    pub reactions: [u32; N_CATEGORIES],
    pub emotes: u32,
    /// Messages that joined a copy-paste wave (≥3 distinct users, same text, within 5 s).
    pub copypasta: u32,
    pub distinct_texts: u32,
    pub phrases: HashMap<String, u32>,
    pub emote_names: HashMap<String, u32>,
}

impl ChatBin {
    pub fn reaction_total(&self) -> u32 {
        // Clip requests are a strong, separate signal; included in the total too.
        self.reactions.iter().sum()
    }
}

#[derive(Debug, Default)]
struct UserState {
    last_norm: String,
    last_ts: TsMs,
    bin_sec: i64,
    in_bin: u32,
}

/// Turns raw messages into per-second bins. Keeps `history_secs` of bins.
#[derive(Debug)]
pub struct ChatAggregator {
    bins: VecDeque<(i64, ChatBin)>,
    users: HashMap<u64, UserState>,
    /// copypasta key -> (first seen ms, distinct users)
    waves: HashMap<String, (TsMs, HashSet<u64>)>,
    history_secs: i64,
    extra_vocab: Vec<String>,
    /// Max messages per user per second that count toward velocity.
    pub per_user_per_sec: u32,
    pub total_seen: u64,
    pub total_suppressed: u64,
}

pub fn user_hash(user: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    user.to_ascii_lowercase().hash(&mut h);
    h.finish()
}

impl ChatAggregator {
    pub fn new(history_secs: i64, extra_vocab: Vec<String>) -> Self {
        ChatAggregator {
            bins: VecDeque::new(),
            users: HashMap::new(),
            waves: HashMap::new(),
            history_secs: history_secs.max(30),
            extra_vocab: extra_vocab.iter().map(|s| text::normalize(s)).collect(),
            per_user_per_sec: 1,
            total_seen: 0,
            total_suppressed: 0,
        }
    }

    fn bin_mut(&mut self, sec: i64) -> &mut ChatBin {
        // Bins are kept sorted by second; messages are almost always in order, but tolerate
        // small reordering (server timestamps from different edges).
        if self.bins.back().map(|(s, _)| *s < sec).unwrap_or(true) {
            let start = self.bins.back().map(|(s, _)| s + 1).unwrap_or(sec);
            // Fill gaps with empty bins so indexing by second stays dense (cap the fill).
            let start = start.max(sec - self.history_secs);
            for s in start..=sec {
                self.bins.push_back((s, ChatBin::default()));
            }
            while self.bins.len() as i64 > self.history_secs {
                self.bins.pop_front();
            }
        }
        let front = self.bins.front().map(|(s, _)| *s).unwrap_or(sec);
        let idx = (sec - front).max(0) as usize;
        let idx = idx.min(self.bins.len() - 1);
        &mut self.bins[idx].1
    }

    pub fn push(&mut self, msg: &ChatMessage) {
        self.total_seen += 1;
        let sec = msg.ts.div_euclid(1000);
        let uname = msg.user.to_ascii_lowercase();
        let uh = user_hash(&uname);
        let norm = text::normalize(&msg.text);

        let is_bot = text::KNOWN_BOTS.contains(&uname.as_str());
        let low = text::is_low_signal(&msg.text, &norm) || norm.is_empty() && msg.emotes.is_empty();

        // Per-user spam suppression: identical repeat within 30 s, or above the per-second cap.
        let st = self.users.entry(uh).or_default();
        let repeat = st.last_norm == norm && msg.ts - st.last_ts < 30_000 && !norm.is_empty();
        if st.bin_sec != sec {
            st.bin_sec = sec;
            st.in_bin = 0;
        }
        st.in_bin += 1;
        let over_cap = st.in_bin > self.per_user_per_sec;
        st.last_norm = norm.clone();
        st.last_ts = msg.ts;

        // Copy-paste waves (computed before suppression decisions on rate).
        let mut wave_hit = false;
        if !is_bot && !low && norm.chars().count() >= 6 {
            let key: String = norm.chars().take(40).collect();
            let e = self.waves.entry(key).or_insert_with(|| (msg.ts, HashSet::new()));
            if msg.ts - e.0 > 5_000 {
                *e = (msg.ts, HashSet::new());
            }
            e.1.insert(uh);
            wave_hit = e.1.len() >= 3;
        }
        if self.waves.len() > 4096 {
            let now = msg.ts;
            self.waves.retain(|_, (t, _)| now - *t < 10_000);
        }

        let extra = self.extra_vocab.clone();
        let bin = self.bin_mut(sec);
        if is_bot || low || repeat {
            bin.suppressed += 1;
            self.total_suppressed += 1;
            return;
        }
        let cats = text::classify(&norm, &extra);
        let first_in_bin_for_user = !bin.users.contains(&uh);
        if first_in_bin_for_user {
            bin.users.push(uh);
            for (i, c) in cats.iter().enumerate() {
                if *c {
                    bin.reactions[i] += 1;
                }
            }
        }
        if over_cap {
            bin.suppressed += 1;
            self.total_suppressed += 1;
            return;
        }
        bin.msgs += 1;
        if wave_hit {
            bin.copypasta += 1;
        }
        let mut emotes = msg.emotes.len() as u32;
        for name in &msg.emotes {
            *bin.emote_names.entry(name.clone()).or_default() += 1;
        }
        if msg.emotes.is_empty() {
            for t in norm.split(' ').filter(|t| text::is_known_emote(t)) {
                emotes += 1;
                *bin.emote_names.entry(t.to_string()).or_default() += 1;
            }
        }
        bin.emotes += emotes.min(5);
        let phrase: String = norm.chars().take(48).collect();
        let entry = bin.phrases.entry(phrase).or_default();
        if *entry == 0 {
            bin.distinct_texts += 1;
        }
        *entry += 1;
        // Bound memory: keep only the top phrases per bin.
        if bin.phrases.len() > 256 {
            bin.phrases.retain(|_, n| *n > 1);
        }
    }

    pub fn bin(&self, sec: i64) -> Option<&ChatBin> {
        let front = self.bins.front()?.0;
        if sec < front {
            return None;
        }
        self.bins.get((sec - front) as usize).map(|(_, b)| b)
    }

    pub fn latest_sec(&self) -> Option<i64> {
        self.bins.back().map(|(s, _)| *s)
    }

    /// Sum a per-bin quantity over `[from, to]` inclusive.
    pub fn sum(&self, from: i64, to: i64, f: impl Fn(&ChatBin) -> u32) -> u32 {
        (from..=to).filter_map(|s| self.bin(s)).map(&f).sum()
    }

    /// Distinct chatters over `[from, to]`.
    pub fn unique_users(&self, from: i64, to: i64) -> u32 {
        let mut set = HashSet::new();
        for s in from..=to {
            if let Some(b) = self.bin(s) {
                set.extend(b.users.iter().copied());
            }
        }
        set.len() as u32
    }

    #[allow(clippy::needless_range_loop)]
    pub fn reactions(&self, from: i64, to: i64) -> [u32; N_CATEGORIES] {
        let mut r = [0u32; N_CATEGORIES];
        for s in from..=to {
            if let Some(b) = self.bin(s) {
                for i in 0..N_CATEGORIES {
                    r[i] += b.reactions[i];
                }
            }
        }
        r
    }

    /// Most common phrases / emotes over a window, for titles and explanations.
    pub fn top_phrases(&self, from: i64, to: i64, n: usize) -> Vec<(String, u32)> {
        let mut acc: HashMap<&str, u32> = HashMap::new();
        for s in from..=to {
            if let Some(b) = self.bin(s) {
                for (p, c) in &b.phrases {
                    *acc.entry(p.as_str()).or_default() += c;
                }
            }
        }
        let mut v: Vec<(String, u32)> = acc
            .into_iter()
            .filter(|(p, c)| *c >= 2 && !p.is_empty())
            .map(|(p, c)| (p.to_string(), c))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    pub fn top_emotes(&self, from: i64, to: i64, n: usize) -> Vec<(String, u32)> {
        let mut acc: HashMap<&str, u32> = HashMap::new();
        for s in from..=to {
            if let Some(b) = self.bin(s) {
                for (p, c) in &b.emote_names {
                    *acc.entry(p.as_str()).or_default() += c;
                }
            }
        }
        let mut v: Vec<(String, u32)> = acc.into_iter().map(|(p, c)| (p.to_string(), c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    pub fn dominant_category(&self, from: i64, to: i64) -> Option<(Category, f32)> {
        let r = self.reactions(from, to);
        let total: u32 = r.iter().sum();
        if total == 0 {
            return None;
        }
        let (i, n) = r
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != Category::ClipRequest as usize)
            .max_by_key(|(_, n)| **n)?;
        Some((Category::ALL[i], *n as f32 / total as f32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(ts: TsMs, user: &str, text: &str) -> ChatMessage {
        ChatMessage {
            ts,
            user: user.into(),
            text: text.into(),
            emotes: vec![],
            is_mod: false,
            is_subscriber: false,
        }
    }

    #[test]
    fn suppresses_single_user_spam() {
        let mut a = ChatAggregator::new(120, vec![]);
        for i in 0..20 {
            a.push(&m(1000 + i * 40, "spammer", &format!("LUL {i}")));
        }
        let b = a.bin(1).unwrap();
        assert_eq!(b.msgs, 1, "one counted message per user per second");
        assert_eq!(b.reactions[Category::Laugh as usize], 1);
        assert_eq!(b.users.len(), 1);
    }

    #[test]
    fn ignores_bots_commands_and_polls() {
        let mut a = ChatAggregator::new(120, vec![]);
        a.push(&m(1000, "Nightbot", "Follow the stream!"));
        a.push(&m(1001, "u1", "!enter"));
        a.push(&m(1002, "u2", "2"));
        a.push(&m(1003, "u3", "KEKW"));
        let b = a.bin(1).unwrap();
        assert_eq!(b.msgs, 1);
        assert_eq!(b.suppressed, 3);
    }

    #[test]
    fn detects_copypasta_wave() {
        let mut a = ChatAggregator::new(120, vec![]);
        for (i, u) in ["a", "b", "c", "d"].iter().enumerate() {
            a.push(&m(5000 + i as i64 * 300, u, "this streamer is literally insane"));
        }
        assert_eq!(a.sum(5, 6, |b| b.copypasta), 2);
    }

    #[test]
    fn unique_and_top_phrases() {
        let mut a = ChatAggregator::new(120, vec![]);
        for (i, u) in ["a", "b", "c"].iter().enumerate() {
            a.push(&m(2000 + i as i64 * 100, u, "NO WAY"));
        }
        assert_eq!(a.unique_users(0, 5), 3);
        assert_eq!(a.top_phrases(0, 5, 3)[0], ("no way".to_string(), 3));
    }
}
