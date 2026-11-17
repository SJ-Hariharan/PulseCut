//! Unified stream selection ("Hot Watch"): one engine decides which Twitch streams get a
//! video buffer. Watchlisted streamers are preferred, favorites first; trending live streams
//! are only a fallback. Switching is damped by a minimum watch time, a switch cooldown and a
//! hysteresis ratio, and every decision carries a human-readable explanation.
//!
//! This module is pure (no I/O, no clocks) so every rule is unit-testable.

use pulsecut_core::TsMs;
use pulsecut_core::settings::WatchSettings;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Watchlist,
    Trending,
}

/// What the engine knows about one channel at decision time.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub login: String,
    pub origin: Origin,
    pub enabled: bool,
    pub favorite: bool,
    pub priority: i32,
    /// `Some(true/false)` from the Twitch API; `None` when live status is unknown
    /// (no API credentials) — such watchlist streams stay eligible.
    pub live: Option<bool>,
    pub viewers: Option<u32>,
    /// Chat/audio hype relative to the detector threshold (1.0 = at threshold).
    pub hype: f32,
}

/// A stream that currently has a buffer.
#[derive(Debug, Clone, Serialize)]
pub struct Active {
    pub login: String,
    pub since: TsMs,
}

#[derive(Debug, Clone, Serialize)]
pub struct Rules {
    pub max_streams: usize,
    pub auto_select: bool,
    pub manual_override: Option<String>,
    pub trending_enabled: bool,
    pub min_watch_ms: i64,
    pub switch_cooldown_ms: i64,
    pub hysteresis: f32,
    pub favorites_preempt: bool,
}

impl Rules {
    pub fn from_settings(w: &WatchSettings, max_streams: usize) -> Rules {
        Rules {
            max_streams,
            auto_select: w.auto_select,
            manual_override: (!w.manual_override.is_empty()).then(|| w.manual_override.clone()),
            trending_enabled: w.trending_enabled,
            min_watch_ms: w.min_watch_secs as i64 * 1000,
            switch_cooldown_ms: w.switch_cooldown_secs as i64 * 1000,
            hysteresis: w.hysteresis.max(1.0),
            favorites_preempt: w.favorites_preempt,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Scored {
    pub login: String,
    pub origin: Origin,
    pub favorite: bool,
    pub live: Option<bool>,
    pub viewers: Option<u32>,
    pub eligible: bool,
    pub score: f32,
    pub note: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Decision {
    /// Logins that should have a buffer after this decision, best first.
    pub selected: Vec<String>,
    /// One line per change or kept stream, e.g. "keep tenz: min watch time not reached".
    pub reasons: Vec<String>,
    pub candidates: Vec<Scored>,
    /// True when at least one stream was swapped voluntarily (resets the switch cooldown).
    pub switched: bool,
}

/// Tier dominates the score so viewer counts and hype only order streams *within* a tier:
/// a trending stream never outranks a live watchlisted one just because it is bigger.
pub fn score(c: &Candidate) -> f32 {
    let tier = match (c.origin, c.favorite) {
        (Origin::Watchlist, true) => 30.0,
        (Origin::Watchlist, false) => 20.0,
        (Origin::Trending, _) => 10.0,
    };
    let prio = c.priority.clamp(-100, 100) as f32 * 0.5;
    let hype = c.hype.clamp(0.0, 4.0) * 1.0;
    let viewers = c.viewers.map(|v| ((v as f32) + 1.0).log10()).unwrap_or(0.0) * 0.5;
    (tier + prio + hype + viewers).max(0.1)
}

fn eligibility(c: &Candidate) -> (bool, &'static str) {
    if !c.enabled {
        return (false, "disabled");
    }
    match (c.origin, c.live) {
        (_, Some(false)) => (false, "offline"),
        (_, Some(true)) => (true, "live"),
        (Origin::Watchlist, None) => (true, "live status unknown"),
        (Origin::Trending, None) => (false, "unknown"),
    }
}

/// Decide which streams get buffers.
pub fn decide(
    cands: &[Candidate],
    current: &[Active],
    last_switch: Option<TsMs>,
    rules: &Rules,
    now: TsMs,
) -> Decision {
    let mut d = Decision::default();
    let max = rules.max_streams;
    let watch_eligible = cands.iter().any(|c| c.origin == Origin::Watchlist && eligibility(c).0);
    let mut pool: Vec<(Candidate, f32)> = vec![];
    for c in cands {
        let (mut ok, mut note) = eligibility(c);
        if ok && c.origin == Origin::Trending {
            if !rules.trending_enabled {
                (ok, note) = (false, "trending disabled");
            } else if watch_eligible {
                (ok, note) = (false, "standby: a watchlisted stream is live");
            }
        }
        if ok && !rules.auto_select && rules.manual_override.as_deref() != Some(c.login.as_str()) {
            (ok, note) = (false, "manual selection mode");
        }
        let s = score(c);
        d.candidates.push(Scored {
            login: c.login.clone(),
            origin: c.origin,
            favorite: c.favorite,
            live: c.live,
            viewers: c.viewers,
            eligible: ok,
            score: s,
            note: note.into(),
        });
        if ok {
            pool.push((c.clone(), s));
        }
    }
    d.candidates
        .sort_by(|a, b| b.eligible.cmp(&a.eligible).then(b.score.total_cmp(&a.score)));
    if max == 0 {
        d.reasons.push("no buffers allowed (max streams = 0)".into());
        return d;
    }
    pool.sort_by(|a, b| b.1.total_cmp(&a.1));
    let get = |login: &str| pool.iter().find(|(c, _)| c.login == login);

    // 1. Manual override takes the first slot while it is eligible.
    if let Some(o) = &rules.manual_override {
        if get(o).is_some() {
            d.selected.push(o.clone());
            d.reasons.push(format!("{o}: manual override"));
        } else {
            d.reasons
                .push(format!("{o}: manual override not available (offline or disabled)"));
        }
    }

    // 2. Keep incumbents that are still eligible, best first.
    let cooldown_ok = last_switch.is_none_or(|t| now - t >= rules.switch_cooldown_ms);
    let mut incumbents: Vec<(&Active, f32, bool)> = current
        .iter()
        .filter(|a| !d.selected.contains(&a.login))
        .filter_map(|a| match get(&a.login) {
            Some((c, s)) => Some((a, *s, c.favorite)),
            None => {
                let why = d
                    .candidates
                    .iter()
                    .find(|x| x.login == a.login)
                    .map(|x| x.note.clone())
                    .unwrap_or_else(|| "removed from watchlist".into());
                d.reasons.push(format!("release {}: {why}", a.login));
                d.switched = true;
                None
            }
        })
        .collect();
    incumbents.sort_by(|a, b| b.1.total_cmp(&a.1));
    let challengers: Vec<&(Candidate, f32)> = pool
        .iter()
        .filter(|(c, _)| !d.selected.contains(&c.login) && !current.iter().any(|a| a.login == c.login))
        .collect();
    let mut ch_iter = challengers.iter().peekable();
    for (a, s, fav) in incumbents {
        if d.selected.len() >= max {
            d.reasons
                .push(format!("release {}: slot needed for manual override", a.login));
            d.switched = true;
            continue;
        }
        let watched = now - a.since;
        let replace = ch_iter.peek().and_then(|(c, cs)| {
            if watched < rules.min_watch_ms {
                return None;
            }
            let favorite_preempt = rules.favorites_preempt && c.favorite && !fav && c.origin == Origin::Watchlist;
            if favorite_preempt {
                return Some(format!("favorite {} is live", c.login));
            }
            if cooldown_ok && *cs > s * rules.hysteresis {
                return Some(format!(
                    "{} scores {:.1} vs {:.1} (> x{:.2})",
                    c.login, cs, s, rules.hysteresis
                ));
            }
            None
        });
        // Only replace when every slot is taken; otherwise the challenger just gets a free slot.
        let slots_full = current.len() >= max;
        match replace {
            Some(why) if slots_full => {
                let (c, _) = ch_iter.next().unwrap();
                d.reasons.push(format!("switch {} -> {}: {why}", a.login, c.login));
                d.selected.push(c.login.clone());
                d.switched = true;
            }
            _ => {
                let why = if watched < rules.min_watch_ms {
                    format!("min watch time ({}s) not reached", rules.min_watch_ms / 1000)
                } else if !cooldown_ok {
                    "switch cooldown active".to_string()
                } else {
                    "still the best option".to_string()
                };
                d.reasons.push(format!("keep {}: {why}", a.login));
                d.selected.push(a.login.clone());
            }
        }
    }

    // 3. Fill free slots with the best remaining candidates.
    for (c, s) in ch_iter {
        if d.selected.len() >= max {
            break;
        }
        if d.selected.contains(&c.login) {
            continue;
        }
        d.reasons.push(format!(
            "start {}: {} {:.1}{}",
            c.login,
            match (c.origin, c.favorite) {
                (Origin::Watchlist, true) => "favorite, score",
                (Origin::Watchlist, false) => "watchlist, score",
                (Origin::Trending, _) => "trending fallback, score",
            },
            s,
            c.viewers.map(|v| format!(", {v} viewers")).unwrap_or_default()
        ));
        d.selected.push(c.login.clone());
    }
    if d.selected.is_empty() {
        d.reasons.push(if pool.is_empty() {
            "nothing to monitor: no eligible live stream".into()
        } else {
            "nothing selected".into()
        });
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules {
            max_streams: 1,
            auto_select: true,
            manual_override: None,
            trending_enabled: true,
            min_watch_ms: 300_000,
            switch_cooldown_ms: 120_000,
            hysteresis: 1.25,
            favorites_preempt: true,
        }
    }
    fn w(login: &str, live: Option<bool>) -> Candidate {
        Candidate {
            login: login.into(),
            origin: Origin::Watchlist,
            enabled: true,
            favorite: false,
            priority: 0,
            live,
            viewers: None,
            hype: 0.0,
        }
    }
    fn t(login: &str, viewers: u32) -> Candidate {
        Candidate {
            origin: Origin::Trending,
            viewers: Some(viewers),
            ..w(login, Some(true))
        }
    }
    fn act(login: &str, since: TsMs) -> Active {
        Active {
            login: login.into(),
            since,
        }
    }
    const NOW: TsMs = 10_000_000;

    #[test]
    fn prefers_live_watchlist_over_bigger_trending() {
        let c = vec![t("huge", 90_000), w("mine", Some(true))];
        let d = decide(&c, &[], None, &rules(), NOW);
        assert_eq!(d.selected, vec!["mine"]);
        let huge = d.candidates.iter().find(|x| x.login == "huge").unwrap();
        assert!(!huge.eligible && huge.note.contains("standby"));
    }

    #[test]
    fn trending_fallback_when_watchlist_offline() {
        let c = vec![w("mine", Some(false)), t("small", 800), t("big", 40_000)];
        let d = decide(&c, &[], None, &rules(), NOW);
        assert_eq!(d.selected, vec!["big"]);
        assert!(d.reasons[0].contains("trending fallback"));
    }

    #[test]
    fn no_trending_when_disabled() {
        let mut r = rules();
        r.trending_enabled = false;
        let d = decide(&[w("mine", Some(false)), t("big", 40_000)], &[], None, &r, NOW);
        assert!(d.selected.is_empty());
        assert!(d.reasons.iter().any(|x| x.contains("nothing to monitor")));
    }

    #[test]
    fn unknown_live_status_keeps_watchlist_eligible() {
        let d = decide(&[w("mine", None)], &[], None, &rules(), NOW);
        assert_eq!(d.selected, vec!["mine"]);
    }

    #[test]
    fn disabled_streamer_is_skipped() {
        let mut c = w("mine", Some(true));
        c.enabled = false;
        let d = decide(&[c, w("other", Some(true))], &[], None, &rules(), NOW);
        assert_eq!(d.selected, vec!["other"]);
    }

    #[test]
    fn favorite_and_priority_order() {
        let mut fav = w("fav", Some(true));
        fav.favorite = true;
        let mut prio = w("prio", Some(true));
        prio.priority = 10;
        let mut r = rules();
        r.max_streams = 2;
        let d = decide(&[w("plain", Some(true)), prio, fav], &[], None, &r, NOW);
        assert_eq!(d.selected, vec!["fav", "prio"]);
    }

    #[test]
    fn no_switch_for_slightly_more_viewers() {
        // Two trending streams; the challenger is a bit bigger but well within hysteresis.
        let c = vec![t("current", 10_000), t("bigger", 12_000)];
        let d = decide(&c, &[act("current", NOW - 3_600_000)], None, &rules(), NOW);
        assert_eq!(d.selected, vec!["current"]);
        assert!(!d.switched);
    }

    #[test]
    fn min_watch_time_blocks_switch() {
        let mut fav = w("fav", Some(true));
        fav.favorite = true;
        let c = vec![t("trend", 5_000), fav];
        // Trending stream started 1 minute ago; favorite just went live.
        let d = decide(&c, &[act("trend", NOW - 60_000)], None, &rules(), NOW);
        // Watchlist live => trending becomes ineligible => released immediately.
        assert_eq!(d.selected, vec!["fav"]);
        assert!(d.switched);
    }

    #[test]
    fn favorite_preempts_watchlist_after_min_watch() {
        let mut fav = w("fav", Some(true));
        fav.favorite = true;
        let c = vec![w("plain", Some(true)), fav.clone()];
        let early = decide(&c, &[act("plain", NOW - 60_000)], None, &rules(), NOW);
        assert_eq!(early.selected, vec!["plain"], "min watch time protects the incumbent");
        let later = decide(&c, &[act("plain", NOW - 400_000)], Some(NOW - 10_000), &rules(), NOW);
        assert_eq!(later.selected, vec!["fav"], "favorite preempts even during cooldown");
        let mut r = rules();
        r.favorites_preempt = false;
        let off = decide(&c, &[act("plain", NOW - 400_000)], None, &r, NOW);
        // 30 + 0 vs 20 + 0: 1.5x > 1.25 hysteresis, so the score rule still switches.
        assert_eq!(off.selected, vec!["fav"]);
    }

    #[test]
    fn switch_cooldown_blocks_score_based_switch() {
        let mut hot = w("hot", Some(true));
        hot.priority = 40; // big score gap
        let c = vec![w("cur", Some(true)), hot];
        let d = decide(&c, &[act("cur", NOW - 900_000)], Some(NOW - 30_000), &rules(), NOW);
        assert_eq!(d.selected, vec!["cur"]);
        assert!(d.reasons[0].contains("cooldown"));
        let d2 = decide(&c, &[act("cur", NOW - 900_000)], Some(NOW - 600_000), &rules(), NOW);
        assert_eq!(d2.selected, vec!["hot"]);
    }

    #[test]
    fn offline_incumbent_is_released_immediately() {
        let c = vec![w("cur", Some(false)), w("next", Some(true))];
        let d = decide(&c, &[act("cur", NOW - 1_000)], Some(NOW - 1_000), &rules(), NOW);
        assert_eq!(d.selected, vec!["next"]);
        assert!(d.reasons.iter().any(|r| r.contains("release cur: offline")));
    }

    #[test]
    fn manual_override_wins_and_manual_mode_restricts() {
        let mut r = rules();
        r.manual_override = Some("pick".into());
        let c = vec![w("a", Some(true)), w("pick", Some(true))];
        let d = decide(&c, &[act("a", NOW - 1_000)], None, &r, NOW);
        assert_eq!(d.selected, vec!["pick"]);
        r.auto_select = false;
        r.max_streams = 2;
        let d = decide(&c, &[], None, &r, NOW);
        assert_eq!(d.selected, vec!["pick"], "manual mode monitors only the override");
        r.manual_override = Some("gone".into());
        let d = decide(&c, &[], None, &r, NOW);
        assert!(d.selected.is_empty());
        assert!(d.reasons[0].contains("not available"));
    }

    #[test]
    fn fills_free_slots_without_replacing() {
        let mut r = rules();
        r.max_streams = 2;
        let c = vec![w("cur", Some(true)), w("new", Some(true))];
        let d = decide(&c, &[act("cur", NOW - 1_000)], Some(NOW), &r, NOW);
        assert_eq!(d.selected, vec!["cur", "new"]);
        assert!(!d.switched);
    }

    #[test]
    fn zero_slots_selects_nothing() {
        let mut r = rules();
        r.max_streams = 0;
        assert!(decide(&[w("a", Some(true))], &[], None, &r, NOW).selected.is_empty());
    }
}
