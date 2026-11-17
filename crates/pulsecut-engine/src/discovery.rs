//! Stream discovery: polls the official Twitch Helix API for the watchlist's live status and,
//! when enabled, the top live streams ("trending"). Runs independently of selection and
//! clipping; results land in `Engine::monitor` and wake the selection scheduler.
//!
//! Polling is adaptive: the base interval is `watch.poll_secs`; errors back off exponentially
//! (up to 15 minutes), rate limits wait for the server's reset hint, and rejected credentials
//! pause polling for 10 minutes. Without credentials nothing is polled and live status is
//! reported as unknown (watchlist streams stay eligible; the video buffer simply retries
//! while a channel is offline).

use crate::Engine;
use pulsecut_chat::helix::{HelixClient, HelixError, LiveStream};
use pulsecut_core::now_ms;
use pulsecut_platform::secrets::keys;
use std::sync::Arc;
use std::time::Duration;

/// Logins on the watchlist (enabled entries of `watch.streamers` plus the legacy
/// `chat.twitch_channels` list, minus a home channel that has its own capture).
pub fn watchlist(s: &pulsecut_core::Settings) -> Vec<pulsecut_core::settings::WatchStreamer> {
    use pulsecut_core::settings::{WatchStreamer, normalize_login};
    let mut v: Vec<WatchStreamer> = s.watch.streamers.clone();
    let skip_home = crate::multistream::home_has_own_capture(s);
    for (i, ch) in s.chat.twitch_channels.iter().enumerate() {
        let login = normalize_login(ch);
        if login.is_empty() || (i == 0 && skip_home) || v.iter().any(|x| x.login == login) {
            continue;
        }
        v.push(WatchStreamer {
            login,
            ..Default::default()
        });
    }
    v
}

pub fn spawn(e: Arc<Engine>) {
    tokio::spawn(async move {
        let mut client: Option<(String, HelixClient)> = None;
        let mut secret: Option<String> = None;
        let mut secret_for = String::new();
        let mut backoff = 0u32;
        loop {
            let s = e.settings();
            let w = &s.watch;
            let base = Duration::from_secs(w.poll_secs as u64);
            let mut wait = base;
            if !w.monitoring_enabled {
                set_note(&e, false, "monitoring is off".into());
            } else if w.twitch_client_id.trim().is_empty() {
                set_note(
                    &e,
                    false,
                    "no Twitch API credentials: live status unknown, trending off".into(),
                );
            } else {
                if secret_for != w.twitch_client_id || secret.is_none() {
                    secret = e.secrets.get(keys::TWITCH_CLIENT_SECRET).ok().flatten();
                    secret_for = w.twitch_client_id.clone();
                    client = None;
                }
                match &secret {
                    None => set_note(&e, false, "Twitch client secret missing in the keychain".into()),
                    Some(sec) => {
                        if client.as_ref().is_none_or(|(id, _)| id != &w.twitch_client_id) {
                            client = Some((
                                w.twitch_client_id.clone(),
                                HelixClient::new(w.twitch_client_id.trim(), sec),
                            ));
                        }
                        let c = &mut client.as_mut().unwrap().1;
                        match poll(c, &s).await {
                            Ok((live, trending)) => {
                                backoff = 0;
                                let n_live = live.len();
                                let n_trend = trending.len();
                                {
                                    let mut m = e.monitor.lock().unwrap();
                                    m.api_ok = true;
                                    m.api_note = format!("{n_live} watchlisted live, {n_trend} trending candidates");
                                    m.last_poll = Some(now_ms());
                                    m.live = live.into_iter().map(|x| (x.login.clone(), x)).collect();
                                    m.trending = trending;
                                }
                                e.monitor_wake.notify_one();
                            }
                            Err(HelixError::RateLimited(d)) => {
                                set_note(&e, false, format!("Twitch rate limit: waiting {}s", d.as_secs()));
                                wait = d.max(Duration::from_secs(5));
                            }
                            Err(HelixError::Auth(m)) => {
                                set_note(
                                    &e,
                                    false,
                                    format!("Twitch credentials rejected ({m}); retrying in 10 min"),
                                );
                                secret = None;
                                wait = Duration::from_secs(600);
                            }
                            Err(err) => {
                                backoff = (backoff + 1).min(6);
                                wait = (base * 2u32.pow(backoff)).min(Duration::from_secs(900));
                                set_note(
                                    &e,
                                    false,
                                    format!("Twitch API unavailable ({err}); retry in {}s", wait.as_secs()),
                                );
                            }
                        }
                    }
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = e.discovery_wake.notified() => {
                    // Settings changed: re-read immediately but never more than every 5 s.
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });
}

fn set_note(e: &Engine, ok: bool, note: String) {
    let mut m = e.monitor.lock().unwrap();
    if m.api_note != note {
        tracing::info!("discovery: {note}");
    }
    m.api_ok = ok;
    m.api_note = note;
    if !ok {
        // Stale data must not drive selection: unknown beats wrong.
        m.live.clear();
        m.trending.clear();
    }
}

async fn poll(
    c: &mut HelixClient,
    s: &pulsecut_core::Settings,
) -> Result<(Vec<LiveStream>, Vec<LiveStream>), HelixError> {
    let logins: Vec<String> = watchlist(s)
        .into_iter()
        .filter(|x| x.enabled)
        .map(|x| x.login)
        .collect();
    let live = if logins.is_empty() {
        vec![]
    } else {
        c.live_status(&logins).await?
    };
    let w = &s.watch;
    let trending = if w.trending_enabled {
        c.top_streams(w.trending_candidates, &w.trending_language, &w.trending_game_id)
            .await?
            .into_iter()
            .filter(|x| x.viewers >= w.trending_min_viewers && !logins.contains(&x.login))
            .collect()
    } else {
        vec![]
    };
    Ok((live, trending))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchlist_merges_legacy_channels_without_duplicates() {
        let mut s = pulsecut_core::Settings::default();
        s.obs.enabled = false;
        s.watch.streamers = vec![pulsecut_core::settings::WatchStreamer {
            login: "tenz".into(),
            favorite: true,
            ..Default::default()
        }];
        s.chat.twitch_channels = vec!["TenZ".into(), "#1win_dota".into()];
        let w = watchlist(&s);
        assert_eq!(w.len(), 2);
        assert!(w[0].favorite, "explicit entry keeps its flags");
        assert_eq!(w[1].login, "1win_dota");
        // With OBS on, the first legacy channel is the streamer's own (home) channel.
        s.obs.enabled = true;
        s.watch.streamers.clear();
        assert_eq!(
            watchlist(&s).iter().map(|x| x.login.as_str()).collect::<Vec<_>>(),
            vec!["1win_dota"]
        );
    }
}
