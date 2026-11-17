//! Clip metadata: title, short caption, description, hashtags and a hook line.
//!
//! The deterministic generator uses what actually happened — the strongest spoken line, what
//! chat said, the moment type and the detection reason — instead of keyword spam. An optional
//! local OpenAI-compatible endpoint (e.g. llama.cpp's `llama-server`) can rewrite the title;
//! any failure or timeout falls back to the template result. No paid API is ever required.

use pulsecut_core::{ClipMetadata, HighlightEvent, MomentKind, TriggerSource};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetaInput {
    pub event: Option<HighlightEvent>,
    pub kind: MomentKind,
    pub trigger: Option<TriggerSource>,
    /// Transcript of the clip (plain text).
    pub transcript: String,
    pub creator: String,
    pub stream_title: String,
    pub game: String,
    pub default_hashtags: Vec<String>,
    pub source_stream: String,
    pub recorded_at: String,
}

/// Generated metadata plus the hook line used on the video and thumbnail.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeneratedMeta {
    pub meta: ClipMetadata,
    pub hook: String,
    pub thumbnail_text: String,
}

const FILLER: &[&str] = &["um", "uh", "like", "you know", "i mean"];

/// Pick the most quotable line from the transcript: short, exclamatory or emotional, not filler.
pub fn best_quote(transcript: &str) -> Option<String> {
    let mut best: Option<(f32, String)> = None;
    for raw in transcript.split_inclusive(['.', '!', '?']) {
        let s = raw.trim();
        let words: Vec<&str> = s.split_whitespace().collect();
        if words.len() < 2 || words.len() > 9 {
            continue;
        }
        let lower = s.to_lowercase();
        if FILLER.iter().any(|f| lower == *f) {
            continue;
        }
        let mut score = 0.0f32;
        if s.ends_with('!') {
            score += 2.0;
        }
        if s.ends_with('?') {
            score += 1.0;
        }
        for kw in [
            "no way",
            "what",
            "oh my god",
            "let's go",
            "lets go",
            "no no",
            "why",
            "how",
            "bro",
            "dude",
            "insane",
            "crazy",
        ] {
            if lower.contains(kw) {
                score += 1.5;
            }
        }
        score += 1.0 - ((words.len() as f32 - 5.0).abs() / 5.0);
        if s.chars().filter(|c| c.is_uppercase()).count() > s.len() / 2 {
            score += 0.5;
        }
        if best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, s.trim_end_matches(['.', ',']).to_string()));
        }
    }
    best.filter(|(s, _)| *s >= 1.0).map(|(_, q)| q)
}

fn tidy_chat(p: &str) -> Option<String> {
    let t = p.trim();
    if t.len() < 2 || t.len() > 24 || !t.chars().any(|c| c.is_alphabetic()) {
        return None;
    }
    Some(t.to_uppercase())
}

fn kind_tags(kind: MomentKind) -> &'static [&'static str] {
    match kind {
        MomentKind::Funny => &["#funny", "#funnymoments"],
        MomentKind::Fail => &["#fail", "#gamingfails"],
        MomentKind::Hype => &["#highlights", "#hype"],
        MomentKind::Clutch => &["#clutch", "#highlights"],
        MomentKind::Shock => &["#unexpected", "#wtf"],
        MomentKind::Wholesome => &["#wholesome"],
        MomentKind::Generic => &["#clips"],
    }
}

fn hashtagify(s: &str) -> Option<String> {
    let t: String = s.chars().filter(|c| c.is_alphanumeric()).collect();
    (t.len() >= 2 && t.len() <= 30).then(|| format!("#{}", t.to_lowercase()))
}

fn clamp_title(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for w in s.split_whitespace() {
        if out.chars().count() + w.chars().count() + 1 > max - 1 {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out.push('…');
    out
}

pub fn generate(input: &MetaInput) -> GeneratedMeta {
    let kind = input.event.as_ref().map(|e| e.kind).unwrap_or(input.kind);
    let quote = best_quote(&input.transcript);
    let chat = input
        .event
        .as_ref()
        .and_then(|e| e.top_phrases.iter().find_map(|p| tidy_chat(p)));
    let who = if input.creator.trim().is_empty() {
        None
    } else {
        Some(input.creator.trim().to_string())
    };
    let game = Some(input.game.trim().to_string()).filter(|g| !g.is_empty());

    // Title: quote-led when we have a good line, otherwise kind-specific with real context.
    let title = match (&quote, kind) {
        (Some(q), MomentKind::Funny) => format!("\"{q}\" — chat could not handle this"),
        (Some(q), MomentKind::Fail) => format!("\"{q}\" — it did not go to plan"),
        (Some(q), MomentKind::Clutch) => format!("\"{q}\" — the clutch of the stream"),
        (Some(q), MomentKind::Hype) => format!("\"{q}\""),
        (Some(q), MomentKind::Shock) => format!("\"{q}\" — nobody saw this coming"),
        (Some(q), _) => format!("\"{q}\""),
        (None, k) => {
            let subject = who.clone().unwrap_or_else(|| "The streamer".into());
            let base = match k {
                MomentKind::Funny => format!("{subject} had chat in tears"),
                MomentKind::Fail => format!("{subject}'s plan went horribly wrong"),
                MomentKind::Hype => format!("{subject} actually pulled this off"),
                MomentKind::Clutch => format!("{subject} clutched the impossible"),
                MomentKind::Shock => "Nobody expected this to happen".to_string(),
                MomentKind::Wholesome => format!(
                    "The most wholesome moment on {}'s stream",
                    who.clone().unwrap_or("the".into())
                ),
                MomentKind::Generic => format!("{subject}'s best moment of the stream"),
            };
            match &chat {
                Some(c) if k != MomentKind::Generic => format!("{base} (chat: \"{c}\")"),
                _ => base,
            }
        }
    };
    let title = clamp_title(&title, 90);

    let hook = match (&quote, kind) {
        (Some(q), _) if q.chars().count() <= 26 => q.clone(),
        (_, MomentKind::Fail) => "This did not go to plan".into(),
        (_, MomentKind::Funny) => "Wait for the reaction".into(),
        (_, MomentKind::Clutch) => "Watch this clutch".into(),
        (_, MomentKind::Hype) => "Watch this".into(),
        (_, MomentKind::Shock) => "Nobody saw this coming".into(),
        _ => "Wait for it".into(),
    };
    let thumbnail_text = match (&quote, &chat) {
        (Some(q), _) if q.split_whitespace().count() <= 4 => q.clone(),
        (_, Some(c)) => c.clone(),
        _ => hook.clone(),
    };

    let mut tags: Vec<String> = vec![];
    let mut push = |t: String| {
        let tl = t.to_lowercase();
        if !tags.iter().any(|x| x.to_lowercase() == tl) && tags.len() < 6 {
            tags.push(t);
        }
    };
    for t in kind_tags(kind) {
        push(t.to_string());
    }
    if let Some(g) = game.as_deref().and_then(hashtagify) {
        push(g);
    }
    if let Some(c) = who.as_deref().and_then(hashtagify) {
        push(c);
    }
    for t in &input.default_hashtags {
        let t = t.trim();
        if t.starts_with('#') && t.len() > 1 {
            push(t.to_string());
        }
    }

    let caption = match (&chat, kind) {
        (Some(c), _) => format!(
            "Chat: \"{c}\" {}",
            tags.iter().take(3).cloned().collect::<Vec<_>>().join(" ")
        ),
        (None, _) => format!(
            "{} {}",
            hook,
            tags.iter().take(3).cloned().collect::<Vec<_>>().join(" ")
        ),
    };

    let mut description = title.clone();
    description.push_str("\n\n");
    if let Some(ev) = &input.event {
        let why = ev
            .reasons
            .iter()
            .filter(|r| r.signal != "timing")
            .take(2)
            .map(|r| r.detail.clone())
            .collect::<Vec<_>>()
            .join("; ");
        if !why.is_empty() {
            description.push_str(&format!("Why this was clipped: {why}.\n"));
        }
    }
    if !input.source_stream.is_empty() {
        description.push_str(&format!("From: {}", input.source_stream));
        if !input.recorded_at.is_empty() {
            description.push_str(&format!(" ({})", input.recorded_at));
        }
        description.push('\n');
    }
    description.push('\n');
    description.push_str(&tags.join(" "));

    GeneratedMeta {
        meta: ClipMetadata {
            title,
            caption,
            description,
            hashtags: tags,
            generator: "template".into(),
        },
        hook,
        thumbnail_text,
    }
}

#[derive(Deserialize)]
struct ChatResp {
    choices: Vec<ChatChoice>,
}
#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMsg,
}
#[derive(Deserialize)]
struct ChatMsg {
    content: String,
}

/// Optional: ask a *local* OpenAI-compatible server for a better title/hook. Returns the
/// template result unchanged on any failure. The endpoint must be loopback (privacy).
pub async fn refine_with_local_llm(base: GeneratedMeta, input: &MetaInput, url: &str, model: &str) -> GeneratedMeta {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return base;
    };
    let local = matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1" | "[::1]"));
    if !local {
        tracing::warn!("local LLM url {url} is not loopback; refusing to send clip data off-device");
        return base;
    }
    let ev = input.event.as_ref();
    let prompt = format!(
        "You write titles for short gaming/livestream clips. Moment type: {}. Streamer: {}. Game: {}. \
         What chat said: {:?}. Transcript: \"{}\". Template title: \"{}\".\n\
         Reply with JSON only: {{\"title\": \"<= 70 chars, specific to what happened, no hashtags, no clickbait lies\", \
         \"hook\": \"<= 26 chars, on-screen opener\"}}",
        ev.map(|e| e.kind.label()).unwrap_or("moment"),
        input.creator,
        input.game,
        ev.map(|e| e.top_phrases.clone()).unwrap_or_default(),
        input.transcript.chars().take(600).collect::<String>(),
        base.meta.title
    );
    let body = serde_json::json!({
        "model": if model.is_empty() { "local" } else { model },
        "temperature": 0.4,
        "max_tokens": 120,
        "messages": [{"role": "user", "content": prompt}],
    });
    let endpoint = format!("{}/v1/chat/completions", url.trim_end_matches('/'));
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(12)).build() {
        Ok(c) => c,
        Err(_) => return base,
    };
    let resp = match client.post(&endpoint).json(&body).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            tracing::info!("local LLM returned {}; using template metadata", r.status());
            return base;
        }
        Err(e) => {
            tracing::info!("local LLM unavailable ({e}); using template metadata");
            return base;
        }
    };
    let Ok(cr) = resp.json::<ChatResp>().await else {
        return base;
    };
    let Some(content) = cr.choices.first().map(|c| c.message.content.clone()) else {
        return base;
    };
    let json_part = content
        .find('{')
        .and_then(|a| content.rfind('}').map(|b| &content[a..=b]))
        .unwrap_or("");
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json_part) else {
        return base;
    };
    let mut out = base;
    if let Some(t) = v
        .get("title")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| t.len() >= 8)
    {
        out.meta.title = clamp_title(t.trim_matches('"'), 90);
        out.meta.description = format!(
            "{}\n{}",
            out.meta.title,
            out.meta.description.split_once('\n').map(|x| x.1).unwrap_or("")
        );
        out.meta.generator = format!("local-llm:{}", if model.is_empty() { "default" } else { model });
    }
    if let Some(h) = v
        .get("hook")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|h| (3..=30).contains(&h.len()))
    {
        out.hook = h.to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: MomentKind, phrases: &[&str]) -> HighlightEvent {
        let mut e = HighlightEvent::manual("twitch:test", TriggerSource::Chat, 0);
        e.kind = kind;
        e.top_phrases = phrases.iter().map(|s| s.to_string()).collect();
        e.reasons = vec![pulsecut_core::Reason {
            signal: "chat.reactions".into(),
            detail: "38 reaction messages in 5 s".into(),
            contribution: 0.6,
        }];
        e
    }

    #[test]
    fn quote_selection() {
        let t = "so I was going to the left side. NO WAY he just did that! um. okay let's keep going";
        assert_eq!(best_quote(t).as_deref(), Some("NO WAY he just did that!"));
        assert_eq!(best_quote("uh. um."), None);
    }

    #[test]
    fn titles_are_contextual() {
        let g = generate(&MetaInput {
            event: Some(ev(MomentKind::Fail, &["noo", "rip"])),
            transcript: "Okay this is easy. Wait why is it moving!".into(),
            creator: "Sam".into(),
            game: "Rocket League".into(),
            default_hashtags: vec!["#shorts".into()],
            ..Default::default()
        });
        assert!(g.meta.title.contains("Wait why is it moving!"), "{}", g.meta.title);
        assert!(g.meta.hashtags.contains(&"#fail".to_string()));
        assert!(g.meta.hashtags.contains(&"#rocketleague".to_string()));
        assert!(g.meta.hashtags.len() <= 6);
        assert!(g.meta.description.contains("Why this was clipped"));
        assert_eq!(g.meta.generator, "template");
    }

    #[test]
    fn no_transcript_uses_chat_and_creator() {
        let g = generate(&MetaInput {
            event: Some(ev(MomentKind::Funny, &["kekw kekw", "im dead"])),
            creator: "Sam".into(),
            ..Default::default()
        });
        assert!(g.meta.title.starts_with("Sam had chat in tears"), "{}", g.meta.title);
        assert!(g.meta.title.contains("KEKW KEKW"));
        assert!(!g.hook.is_empty());
    }

    #[test]
    fn long_titles_are_clamped() {
        let t = clamp_title(&"word ".repeat(40), 50);
        assert!(t.chars().count() <= 50);
        assert!(t.ends_with('…'));
    }

    #[tokio::test]
    async fn llm_refuses_non_loopback_and_falls_back() {
        let base = generate(&MetaInput::default());
        let out = refine_with_local_llm(base.clone(), &MetaInput::default(), "https://api.example.com", "").await;
        assert_eq!(out.meta.title, base.meta.title);
        let out = refine_with_local_llm(base.clone(), &MetaInput::default(), "http://127.0.0.1:9", "").await;
        assert_eq!(out.meta.generator, "template");
    }
}
