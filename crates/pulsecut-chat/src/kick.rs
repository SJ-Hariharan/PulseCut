//! Kick official webhooks (`chat.message.sent`).
//!
//! Kick signs each delivery: the signed string is `{Kick-Event-Message-Id}.{Kick-Event-Message-Timestamp}.{raw body}`,
//! RSA PKCS#1 v1.5 with SHA-256, base64 in `Kick-Event-Signature`, verified against Kick's public
//! key (`https://api.kick.com/public/v1/public-key`). Webhooks need a publicly reachable URL, so
//! for a desktop app this requires a user-provided tunnel/relay (documented in docs/USER_GUIDE.md).

use base64::Engine;
use pulsecut_detect::ChatMessage;
use rsa::RsaPublicKey;
use rsa::pkcs1v15::{Signature, VerifyingKey};
use rsa::pkcs8::DecodePublicKey;
use rsa::sha2::Sha256;
use rsa::signature::Verifier;
use serde::Deserialize;

pub const PUBLIC_KEY_URL: &str = "https://api.kick.com/public/v1/public-key";

#[derive(Debug)]
pub enum VerifyError {
    BadKey(String),
    BadSignature,
    MissingHeader(&'static str),
    Stale,
}

pub struct KickVerifier {
    key: VerifyingKey<Sha256>,
    /// Reject deliveries older than this (replay protection).
    pub max_age_secs: i64,
}

impl KickVerifier {
    pub fn from_pem(pem: &str) -> Result<Self, VerifyError> {
        let pk = RsaPublicKey::from_public_key_pem(pem.trim()).map_err(|e| VerifyError::BadKey(e.to_string()))?;
        Ok(KickVerifier {
            key: VerifyingKey::<Sha256>::new(pk),
            max_age_secs: 600,
        })
    }

    pub fn verify(
        &self,
        message_id: &str,
        timestamp: &str,
        body: &[u8],
        signature_b64: &str,
        now_secs: i64,
    ) -> Result<(), VerifyError> {
        if let Ok(t) = chrono::DateTime::parse_from_rfc3339(timestamp)
            && (now_secs - t.timestamp()).abs() > self.max_age_secs
        {
            return Err(VerifyError::Stale);
        }
        let sig = base64::engine::general_purpose::STANDARD
            .decode(signature_b64.trim())
            .map_err(|_| VerifyError::BadSignature)?;
        let sig = Signature::try_from(sig.as_slice()).map_err(|_| VerifyError::BadSignature)?;
        let mut msg = Vec::with_capacity(body.len() + message_id.len() + timestamp.len() + 2);
        msg.extend_from_slice(message_id.as_bytes());
        msg.push(b'.');
        msg.extend_from_slice(timestamp.as_bytes());
        msg.push(b'.');
        msg.extend_from_slice(body);
        self.key.verify(&msg, &sig).map_err(|_| VerifyError::BadSignature)
    }
}

#[derive(Debug, Deserialize)]
pub struct ChatEvent {
    #[serde(default)]
    pub message_id: String,
    pub broadcaster: Option<KickUser>,
    pub sender: KickUser,
    pub content: String,
    #[serde(default)]
    pub emotes: Vec<KickEmote>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct KickUser {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub channel_slug: String,
}

#[derive(Debug, Deserialize)]
pub struct KickEmote {
    #[serde(default)]
    pub emote_id: serde_json::Value,
    #[serde(default)]
    pub positions: Vec<Pos>,
}

#[derive(Debug, Deserialize)]
pub struct Pos {
    pub s: usize,
    pub e: usize,
}

/// Parse a verified `chat.message.sent` body into (channel slug, message).
pub fn parse_chat(body: &[u8], received_ms: i64) -> Option<(String, ChatMessage)> {
    let ev: ChatEvent = serde_json::from_slice(body).ok()?;
    let slug = ev
        .broadcaster
        .as_ref()
        .map(|b| {
            if b.channel_slug.is_empty() {
                b.username.to_lowercase()
            } else {
                b.channel_slug.clone()
            }
        })
        .unwrap_or_default();
    let chars: Vec<char> = ev.content.chars().collect();
    let emotes = ev
        .emotes
        .iter()
        .flat_map(|e| e.positions.iter())
        .filter(|p| p.s <= p.e && p.e < chars.len())
        .map(|p| chars[p.s..=p.e].iter().collect())
        .collect();
    let ts = ev
        .created_at
        .as_deref()
        .and_then(|c| chrono::DateTime::parse_from_rfc3339(c).ok())
        .map(|d| d.timestamp_millis())
        .unwrap_or(received_ms);
    Some((
        slug,
        ChatMessage {
            ts,
            user: ev.sender.username,
            text: ev.content,
            emotes,
            is_mod: false,
            is_subscriber: false,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::RsaPrivateKey;
    use rsa::pkcs1v15::SigningKey;
    use rsa::pkcs8::{EncodePublicKey, LineEnding};
    use rsa::signature::{SignatureEncoding, Signer};

    #[test]
    fn verifies_kick_style_signatures() {
        let mut rng = rsa::rand_core::OsRng;
        let sk = RsaPrivateKey::new(&mut rng, 1024).unwrap();
        let pem = sk.to_public_key().to_public_key_pem(LineEnding::LF).unwrap();
        let v = KickVerifier::from_pem(&pem).unwrap();
        let body = br#"{"message_id":"m1","broadcaster":{"username":"Streamer","channel_slug":"streamer"},"sender":{"username":"fan"},"content":"emoteKEKW lol","emotes":[{"emote_id":"1","positions":[{"s":0,"e":8}]}]}"#;
        let (id, ts) = ("01HXYZ", "2026-11-17T10:00:00Z");
        let mut msg = format!("{id}.{ts}.").into_bytes();
        msg.extend_from_slice(body);
        let sig = SigningKey::<Sha256>::new(sk).sign(&msg);
        let b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
        let now = chrono::DateTime::parse_from_rfc3339(ts).unwrap().timestamp();
        assert!(v.verify(id, ts, body, &b64, now).is_ok());
        // Tampered body, wrong id, stale timestamp all fail.
        assert!(v.verify(id, ts, b"{}", &b64, now).is_err());
        assert!(v.verify("other", ts, body, &b64, now).is_err());
        assert!(matches!(
            v.verify(id, ts, body, &b64, now + 3600),
            Err(VerifyError::Stale)
        ));
        let (slug, m) = parse_chat(body, 0).unwrap();
        assert_eq!(slug, "streamer");
        assert_eq!(m.emotes, vec!["emoteKEKW"]);
    }
}
