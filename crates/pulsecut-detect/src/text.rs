//! Chat text normalization and the reaction vocabulary.

/// Reaction categories. Index into `[u32; N_CATEGORIES]` arrays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Laugh = 0,
    Hype = 1,
    Shock = 2,
    Fail = 3,
    Wholesome = 4,
    ClipRequest = 5,
}

pub const N_CATEGORIES: usize = 6;

impl Category {
    pub const ALL: [Category; N_CATEGORIES] = [
        Category::Laugh,
        Category::Hype,
        Category::Shock,
        Category::Fail,
        Category::Wholesome,
        Category::ClipRequest,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Category::Laugh => "laugh",
            Category::Hype => "hype",
            Category::Shock => "shock",
            Category::Fail => "fail",
            Category::Wholesome => "wholesome",
            Category::ClipRequest => "clip",
        }
    }
}

/// Lowercase, collapse runs of 3+ identical characters to 2 ("LOOOOOL" → "lool"),
/// turn punctuation into spaces (keeping emoji and `<3`), squeeze whitespace.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev: Option<char> = None;
    let mut run = 0;
    for ch in text.chars().flat_map(|c| c.to_lowercase()) {
        let ch = if ch.is_alphanumeric() || ch == '<' || (ch as u32) > 0x2000 {
            ch
        } else {
            ' '
        };
        if Some(ch) == prev {
            run += 1;
            if run >= 2 {
                continue;
            }
        } else {
            run = 0;
        }
        if ch == ' ' && (out.is_empty() || out.ends_with(' ')) {
            prev = Some(ch);
            continue;
        }
        out.push(ch);
        prev = Some(ch);
    }
    out.trim_end().to_string()
}

/// Word-level vocabulary (matched against whitespace tokens of the normalized text).
const WORDS: &[(&str, Category)] = &[
    // laugh
    ("lul", Category::Laugh),
    ("lulw", Category::Laugh),
    ("lol", Category::Laugh),
    ("lool", Category::Laugh),
    ("lmao", Category::Laugh),
    ("lmfao", Category::Laugh),
    ("kekw", Category::Laugh),
    ("kek", Category::Laugh),
    ("omegalul", Category::Laugh),
    ("xd", Category::Laugh),
    ("xdd", Category::Laugh),
    ("haha", Category::Laugh),
    ("hahaha", Category::Laugh),
    ("ahah", Category::Laugh),
    ("icant", Category::Laugh),
    ("pepelaugh", Category::Laugh),
    ("😂", Category::Laugh),
    ("🤣", Category::Laugh),
    ("dead", Category::Laugh),
    // hype
    ("pog", Category::Hype),
    ("pogg", Category::Hype),
    ("pogchamp", Category::Hype),
    ("poggers", Category::Hype),
    ("pogu", Category::Hype),
    ("letsgo", Category::Hype),
    ("lfg", Category::Hype),
    ("w", Category::Hype),
    ("ww", Category::Hype),
    ("huge", Category::Hype),
    ("insane", Category::Hype),
    ("goated", Category::Hype),
    ("clutch", Category::Hype),
    ("ez", Category::Hype),
    ("gg", Category::Hype),
    ("ggs", Category::Hype),
    ("🔥", Category::Hype),
    ("hype", Category::Hype),
    ("clap", Category::Hype),
    ("peepoclap", Category::Hype),
    ("cracked", Category::Hype),
    // shock
    ("omg", Category::Shock),
    ("wtf", Category::Shock),
    ("what", Category::Shock),
    ("noway", Category::Shock),
    ("monkas", Category::Shock),
    ("holy", Category::Shock),
    ("😱", Category::Shock),
    ("😮", Category::Shock),
    ("wow", Category::Shock),
    ("yo", Category::Shock),
    // fail
    ("f", Category::Fail),
    ("rip", Category::Fail),
    ("unlucky", Category::Fail),
    ("noo", Category::Fail),
    ("l", Category::Fail),
    ("throw", Category::Fail),
    ("throwing", Category::Fail),
    ("sadge", Category::Fail),
    ("bruh", Category::Fail),
    ("pepehands", Category::Fail),
    ("biblethump", Category::Fail),
    ("💀", Category::Fail),
    ("oof", Category::Fail),
    // wholesome
    ("<3", Category::Wholesome),
    ("❤", Category::Wholesome),
    ("❤️", Category::Wholesome),
    ("aww", Category::Wholesome),
    ("cute", Category::Wholesome),
    ("wholesome", Category::Wholesome),
];

/// Multi-word phrases matched as substrings of the normalized text.
const PHRASES: &[(&str, Category)] = &[
    ("lets go", Category::Hype),
    ("no way", Category::Shock),
    ("wait what", Category::Shock),
    ("clip it", Category::ClipRequest),
    ("clip that", Category::ClipRequest),
    ("someone clip", Category::ClipRequest),
    ("clipped", Category::ClipRequest),
    ("im dead", Category::Laugh),
    ("so bad", Category::Fail),
];

/// Emote-ish tokens that also count as emotes when the platform didn't tag them.
pub fn is_known_emote(token: &str) -> bool {
    matches!(
        token,
        "lul"
            | "lulw"
            | "kekw"
            | "omegalul"
            | "pogchamp"
            | "poggers"
            | "pogu"
            | "monkas"
            | "sadge"
            | "pepehands"
            | "biblethump"
            | "pepelaugh"
            | "peepoclap"
            | "kappa"
            | "pog"
    ) || token.chars().any(|c| (c as u32) >= 0x1F300)
}

/// Count reaction categories in a message (each category at most once per message).
pub fn classify(norm: &str, extra: &[String]) -> [bool; N_CATEGORIES] {
    let mut hit = [false; N_CATEGORIES];
    for tok in norm.split(' ') {
        if tok.is_empty() {
            continue;
        }
        // Collapse repeated-syllable laughs: "hahahaha" / "ahahah".
        if tok.len() >= 4 && tok.chars().all(|c| c == 'h' || c == 'a') && tok.contains("ha") {
            hit[Category::Laugh as usize] = true;
            continue;
        }
        if let Some((_, c)) = WORDS.iter().find(|(w, _)| *w == tok) {
            hit[*c as usize] = true;
        }
    }
    for (p, c) in PHRASES {
        if norm.contains(p) {
            hit[*c as usize] = true;
        }
    }
    for e in extra {
        if !e.is_empty() && norm.split(' ').any(|t| t == e) {
            hit[Category::Hype as usize] = true;
        }
    }
    hit
}

/// Messages that carry no excitement signal and are commonly mass-posted: bot commands,
/// poll votes ("1", "2"), giveaway entries.
pub fn is_low_signal(raw: &str, norm: &str) -> bool {
    let t = raw.trim_start();
    if t.starts_with('!') {
        return true;
    }
    if !norm.is_empty() && norm.len() <= 2 && norm.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    false
}

pub const KNOWN_BOTS: &[&str] = &[
    "nightbot",
    "streamelements",
    "streamlabs",
    "moobot",
    "fossabot",
    "soundalerts",
    "wizebot",
    "sery_bot",
    "botrixoficial",
    "kofistreambot",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(normalize("LOOOOOL!!!"), "lool");
        assert_eq!(normalize("  no   WAY!! "), "no way");
        assert_eq!(normalize("<3 <3"), "<3 <3");
    }

    #[test]
    fn classifies() {
        let c = classify(&normalize("KEKW KEKW"), &[]);
        assert!(c[Category::Laugh as usize]);
        let c = classify(&normalize("LETS GOOOO"), &[]);
        assert!(c[Category::Hype as usize], "{:?}", normalize("LETS GOOOO"));
        let c = classify(&normalize("hahahahaha"), &[]);
        assert!(c[Category::Laugh as usize]);
        let c = classify(&normalize("someone clip that"), &[]);
        assert!(c[Category::ClipRequest as usize]);
        let c = classify(&normalize("how was your day"), &[]);
        assert!(c.iter().all(|x| !x));
    }

    #[test]
    fn low_signal() {
        assert!(is_low_signal("!enter", "enter"));
        assert!(is_low_signal("2", "2"));
        assert!(!is_low_signal("W", "w"));
    }
}
