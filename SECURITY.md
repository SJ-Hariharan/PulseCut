# Security Policy

## Supported versions

| Version | Supported |
|---|---|
| 0.1.x | Yes |
| Earlier pre-release builds | No |

Security fixes land on the `main` branch and are released as the next 0.1.x patch version.

## Reporting a vulnerability

Please report vulnerabilities privately using GitHub private vulnerability reporting:

1. Open the repository's **Security** tab.
2. Click **Report a vulnerability**.
3. Describe the issue and, if possible, include a proof of concept.

Please do not open a public issue, pull request, or discussion for a suspected vulnerability.

### What to include

* The PulseCut version (or commit) and your operating system and version
* The component affected (for example the local control API, the Kick webhook receiver, the OAuth flow, or secret storage)
* Steps to reproduce, and the expected and actual results
* The impact you believe the issue has, and any suggested fix
* Whether you plan to disclose the issue publicly, and when

Do not include real tokens, client secrets, OBS passwords, or channel credentials in your report. Replace them with placeholders.

### What to expect

PulseCut is maintained by volunteers, so responses are best effort. The maintainer aims to acknowledge a report within a reasonable time, confirm whether it is a vulnerability, and keep you informed while a fix is prepared. Once a fix is released, reporters who want credit will be acknowledged in the release notes. Please allow time for a fix before public disclosure.

## How PulseCut handles secrets

These statements describe the current 0.1.x code.

* **OS credential store.** OBS WebSocket password, YouTube API key, YouTube OAuth tokens and client secret, Instagram Graph token, TikTok OAuth tokens and client secret, and the Kick public key are stored in the OS credential store (macOS Keychain or Windows Credential Manager) under the service name `app.pulsecut.PulseCut`, accessed via the `keyring` crate.
* **Not in settings.** The settings file (`settings.toml`) does not hold secrets. The OBS password is read from the keychain entry when the "use password" option is enabled.
* **Not shown again.** The Settings screen shows a "stored in keychain" placeholder instead of the stored value, and secret fields are masked while being entered.
* **Installed-app OAuth with PKCE.** Google and TikTok connections use the authorization code flow with PKCE (`code_challenge_method=S256`). The verifier is random, and the challenge is its SHA-256 hash.
* **CSRF protection.** A random `state` value is sent with the authorization request. The redirect is rejected if the returned `state` does not match.
* **One-shot loopback listener.** The redirect is received by a temporary listener bound to `127.0.0.1` on an ephemeral port. It handles the callback, is aborted after the code is received, and the flow times out after 5 minutes.
* **Token refresh.** Access tokens are refreshed automatically when they are close to expiry. If there is no refresh token, PulseCut asks the user to reconnect the account.

## Network surface

PulseCut exposes two HTTP routes, both served on one listener bound to `127.0.0.1`.

* **Local control API.** Binds to `127.0.0.1` only, on port `47120` by default. Setting the port to `0` disables it. It provides `POST /api/clip` (trigger a clip), `GET /api/status` (read status), and no routes for changing settings or reading secrets. The API has **no authentication**; any process running on the same machine can call it.
* **Kick webhook receiver.** `POST /webhooks/kick` accepts only `chat.message.sent` deliveries (other event types return 204). The delivery must carry a valid RSA PKCS#1 v1.5 SHA-256 signature, base64-encoded in the `Kick-Event-Signature` header, over the string `{message-id}.{timestamp}.{body}`, verified against the Kick public key stored in the keychain. Invalid signatures return 401. If the public key is not configured, the endpoint returns 503. Deliveries whose timestamp is more than 10 minutes away from the local clock are rejected to limit replay.

## Hardening notes for users

* **Do not expose the local API through a tunnel.** The local control API has no authentication and shares its port with the Kick webhook route. Do not forward the whole port (for example with ngrok, Cloudflare Tunnel, or a router port forward). If you need Kick webhooks to reach your machine, forward only the `/webhooks/kick` path if your tunnel supports path-based routing, and keep the Kick signature check enabled.
* **Keep the Kick public key configured.** Without it the webhook receiver refuses deliveries, which is the intended fail-safe.
* **The Instagram media folder is public.** Clips placed in the media host folder are served from the public base URL you configure, so anyone with the link can download them. Only use a folder you are happy to publish, and remove files once the upload is complete.
* **Protect your OS account.** The keychain entries are protected by your OS login. Keep your account password, screen lock, and disk encryption enabled.
* **Redact logs before sharing.** Logs and bug reports can contain channel names and file paths. Remove tokens, channel credentials, and personal paths before posting them publicly.

## Known gaps

* The local control API has no authentication (see above). It is intended for use on the same machine only.
* The Kick webhook age check is applied only when the `Kick-Event-Message-Timestamp` header parses as an RFC 3339 timestamp. A delivery with an unparseable timestamp still has to carry a valid signature, but it skips the 10-minute window. Valid deliveries are not separately de-duplicated by the webhook handler, so a captured, validly signed delivery replayed within the window could be accepted again.
* No `cargo audit` or `cargo deny` gate runs in CI yet.
