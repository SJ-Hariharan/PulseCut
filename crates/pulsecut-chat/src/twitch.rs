//! Twitch chat via anonymous IRC over WebSocket (`justinfanNNNNN`, read-only).
//! Requests `twitch.tv/tags` (server timestamps, emotes, badges) and `twitch.tv/commands`
//! (RECONNECT notices). Reconnects with exponential backoff; joins many channels on one socket.

use crate::{SourceState, StreamChat};
use futures_util::{SinkExt, StreamExt};
use pulsecut_core::now_ms;
use pulsecut_detect::ChatMessage;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;

pub const TWITCH_IRC_WS: &str = "wss://irc-ws.chat.twitch.tv:443";

/// Unescape IRCv3 tag values.
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('s') => out.push(' '),
                Some(':') => out.push(';'),
                Some('\\') => out.push('\\'),
                Some('r') => out.push('\r'),
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrcLine<'a> {
    pub tags: HashMap<&'a str, String>,
    pub prefix: Option<&'a str>,
    pub command: &'a str,
    pub params: Vec<&'a str>,
    pub trailing: Option<&'a str>,
}

pub fn parse_line(line: &str) -> Option<IrcLine<'_>> {
    let mut rest = line.trim_end_matches(['\r', '\n']);
    let mut tags = HashMap::new();
    if let Some(r) = rest.strip_prefix('@') {
        let (t, r2) = r.split_once(' ')?;
        for kv in t.split(';') {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            tags.insert(k, unescape(v));
        }
        rest = r2;
    }
    let mut prefix = None;
    if let Some(r) = rest.strip_prefix(':') {
        let (p, r2) = r.split_once(' ')?;
        prefix = Some(p);
        rest = r2;
    }
    let (head, trailing) = match rest.split_once(" :") {
        Some((h, t)) => (h, Some(t)),
        None => (rest, None),
    };
    let mut parts = head.split(' ').filter(|s| !s.is_empty());
    let command = parts.next()?;
    Some(IrcLine {
        tags,
        prefix,
        command,
        params: parts.collect(),
        trailing,
    })
}

/// `emotes` tag: `25:0-4,12-16/1902:6-10` (code-point offsets, inclusive) → emote names.
pub fn emote_names(tag: &str, text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![];
    for group in tag.split('/').filter(|g| !g.is_empty()) {
        let Some((_, ranges)) = group.split_once(':') else {
            continue;
        };
        for r in ranges.split(',') {
            let Some((a, b)) = r.split_once('-') else { continue };
            let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>()) else {
                continue;
            };
            if a <= b && b < chars.len() {
                out.push(chars[a..=b].iter().collect());
            }
        }
    }
    out
}

/// Convert a PRIVMSG line into (channel, ChatMessage).
pub fn to_chat(line: &IrcLine<'_>) -> Option<(String, ChatMessage)> {
    if line.command != "PRIVMSG" {
        return None;
    }
    let channel = line.params.first()?.trim_start_matches('#').to_ascii_lowercase();
    let mut text = line.trailing.unwrap_or("").to_string();
    // /me actions arrive as CTCP ACTION.
    if let Some(a) = text.strip_prefix("\u{1}ACTION ").and_then(|t| t.strip_suffix('\u{1}')) {
        text = a.to_string();
    }
    let login = line.prefix.and_then(|p| p.split('!').next()).unwrap_or("").to_string();
    let user = line
        .tags
        .get("display-name")
        .filter(|d| !d.is_empty())
        .cloned()
        .unwrap_or(login);
    let ts = line
        .tags
        .get("tmi-sent-ts")
        .and_then(|t| t.parse::<i64>().ok())
        .unwrap_or_else(now_ms);
    let emotes = line
        .tags
        .get("emotes")
        .map(|e| emote_names(e, &text))
        .unwrap_or_default();
    let badges = line.tags.get("badges").cloned().unwrap_or_default();
    Some((
        channel,
        ChatMessage {
            ts,
            user,
            text,
            emotes,
            is_mod: line.tags.get("mod").map(|m| m == "1").unwrap_or(false) || badges.contains("broadcaster"),
            is_subscriber: line.tags.get("subscriber").map(|m| m == "1").unwrap_or(false),
        },
    ))
}

pub struct TwitchChat {
    pub state: watch::Receiver<SourceState>,
    stop: watch::Sender<bool>,
}

impl TwitchChat {
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }
}

/// Connect anonymously and stream messages for `channels` to `out`.
pub fn spawn(channels: Vec<String>, out: mpsc::Sender<StreamChat>, url: Option<String>) -> TwitchChat {
    let (state_tx, state) = watch::channel(SourceState::Connecting);
    let (stop, mut stop_rx) = watch::channel(false);
    let url = url.unwrap_or_else(|| TWITCH_IRC_WS.to_string());
    tokio::spawn(async move {
        let mut attempt = 0u32;
        loop {
            if *stop_rx.borrow() {
                break;
            }
            let reason = match session(&url, &channels, &out, &state_tx, &mut stop_rx).await {
                Ok(()) => break,
                Err(e) => e,
            };
            if out.is_closed() || *stop_rx.borrow() {
                break;
            }
            attempt += 1;
            tracing::warn!("twitch chat disconnected ({reason}); reconnect #{attempt}");
            let _ = state_tx.send(SourceState::Reconnecting {
                attempt,
                reason: reason.clone(),
            });
            let wait = Duration::from_millis((500u64 << attempt.min(6)).min(30_000));
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = stop_rx.changed() => {}
            }
        }
        let _ = state_tx.send(SourceState::Stopped);
    });
    TwitchChat { state, stop }
}

async fn session(
    url: &str,
    channels: &[String],
    out: &mpsc::Sender<StreamChat>,
    state: &watch::Sender<SourceState>,
    stop: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let (ws, _) = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(url))
        .await
        .map_err(|_| "connect timeout".to_string())?
        .map_err(|e| e.to_string())?;
    let (mut tx, mut rx) = ws.split();
    let nick = format!("justinfan{}", 10_000 + (now_ms() % 80_000));
    for l in [
        "CAP REQ :twitch.tv/tags twitch.tv/commands".to_string(),
        "PASS SCHMOOPIIE".to_string(),
        format!("NICK {nick}"),
    ] {
        tx.send(Message::text(l)).await.map_err(|e| e.to_string())?;
    }
    // JOIN in batches (Twitch limits joins per message/second).
    for chunk in channels.chunks(20) {
        let list: Vec<String> = chunk
            .iter()
            .map(|c| format!("#{}", c.trim_start_matches('#').to_lowercase()))
            .collect();
        tx.send(Message::text(format!("JOIN {}", list.join(","))))
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut last_rx = tokio::time::Instant::now();
    let mut keepalive = tokio::time::interval(Duration::from_secs(60));
    loop {
        tokio::select! {
            m = rx.next() => {
                let m = match m {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => return Err(e.to_string()),
                    None => return Err("socket closed".into()),
                };
                last_rx = tokio::time::Instant::now();
                let text = match m {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => return Err("server closed".into()),
                    _ => continue,
                };
                for raw in text.split("\r\n").filter(|l| !l.is_empty()) {
                    let Some(line) = parse_line(raw) else { continue };
                    match line.command {
                        "PING" => {
                            let arg = line.trailing.unwrap_or("tmi.twitch.tv");
                            tx.send(Message::text(format!("PONG :{arg}"))).await.map_err(|e| e.to_string())?;
                        }
                        "001" => { let _ = state.send(SourceState::Connected); }
                        "RECONNECT" => return Err("server requested reconnect".into()),
                        "NOTICE" if line.trailing.is_some_and(|t| t.contains("Login") || t.contains("auth")) => {
                            return Err(format!("notice: {}", line.trailing.unwrap_or("")));
                        }
                        "PRIVMSG" => {
                            if let Some((ch, msg)) = to_chat(&line) {
                                // Bounded channel: if the engine lags, drop rather than block the socket.
                                if out.try_send(StreamChat { stream_id: format!("twitch:{ch}"), msg }).is_err() && out.is_closed() {
                                    return Ok(());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ = keepalive.tick() => {
                if last_rx.elapsed() > Duration::from_secs(360) {
                    return Err("no traffic for 6 minutes".into());
                }
                tx.send(Message::text("PING :pulsecut")).await.map_err(|e| e.to_string())?;
            }
            _ = stop.changed() => {
                let _ = tx.send(Message::Close(None)).await;
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "@badge-info=;badges=moderator/1;color=#FF0000;display-name=Some\\sUser;emotes=25:0-4,12-16/1902:6-10;mod=1;subscriber=0;tmi-sent-ts=1642696567751;user-id=1 :someuser!someuser@someuser.tmi.twitch.tv PRIVMSG #SomeChannel :Kappa Keepo Kappa";

    #[test]
    fn parses_privmsg_with_tags_and_emotes() {
        let l = parse_line(LINE).unwrap();
        assert_eq!(l.command, "PRIVMSG");
        let (ch, m) = to_chat(&l).unwrap();
        assert_eq!(ch, "somechannel");
        assert_eq!(m.user, "Some User");
        assert_eq!(m.ts, 1_642_696_567_751);
        assert_eq!(m.emotes, vec!["Kappa", "Kappa", "Keepo"]);
        assert!(m.is_mod);
        assert_eq!(m.text, "Kappa Keepo Kappa");
    }

    #[test]
    fn handles_unicode_offsets_and_actions() {
        let text = "héllo Kappa";
        assert_eq!(emote_names("25:6-10", text), vec!["Kappa"]);
        assert!(emote_names("25:6-99", text).is_empty(), "out of range ignored");
        let l = parse_line(":u!u@u PRIVMSG #c :\u{1}ACTION waves\u{1}").unwrap();
        assert_eq!(to_chat(&l).unwrap().1.text, "waves");
    }

    #[test]
    fn ping_and_numeric() {
        let l = parse_line("PING :tmi.twitch.tv").unwrap();
        assert_eq!((l.command, l.trailing), ("PING", Some("tmi.twitch.tv")));
        let l = parse_line(":tmi.twitch.tv 001 justinfan123 :Welcome, GLHF!").unwrap();
        assert_eq!(l.command, "001");
    }
}
