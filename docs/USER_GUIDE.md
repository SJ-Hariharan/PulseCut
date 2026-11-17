# User guide

## 1. Install

**macOS**: install the native tools, then the app.

```bash
brew install ffmpeg whisper-cpp
```

```bash
scripts/package-macos.sh release
```

Copy `target/dist/PulseCut.app` to Applications. Put a whisper model (e.g. `ggml-base.en.bin`, 142 MB) in `~/Library/Application Support/app.PulseCut.PulseCut/models/` (or next to the executable in `models/`). Without a model PulseCut still clips, just without captions. Optional: `ggml-tiny.en.bin` alongside it makes voice commands faster.

**Windows** (experimental, see PLATFORM_SUPPORT.md): install FFmpeg (`winget install Gyan.FFmpeg`) and whisper.cpp, build with `scripts/package-windows.ps1`.

## 2. Streamer mode (OBS)

1. OBS → **Settings → Output → Replay Buffer**: enable it and set *Maximum Replay Time* to at least the value shown in PulseCut's Settings → Capture (42 s with defaults; more if you raise pre/post-roll).
2. OBS → **Tools → WebSocket Server Settings**: enable, note the port and password.
3. PulseCut → **Settings → Capture & OBS**: choose *OBS replay buffer*, tick *Connect to OBS*, enter host/port, paste the password and press **Save to keychain**, then **Save & apply**.
4. Dashboard shows *OBS WebSocket ● connected*, streaming state and the replay-buffer length. A warning appears if the buffer is too short.
5. Clip with **Clip now**, the global hotkey (default **Cmd/Ctrl+Shift+K**), the voice command ("clip it", enable it in Settings → Audio & Voice), or `curl -X POST http://127.0.0.1:47120/api/clip` (Stream Deck "website" action).

PulseCut waits until the post-roll has elapsed, saves the replay, moves it into its library and edits it. Check from a terminal with `pulsecut-cli obs-check`.

## 3. Autonomous mode (chat-driven)

1. Settings → **Platforms**: list Twitch channels (first one = your channel). No Twitch login is needed — chat is read anonymously.
2. Settings → **Detection**: *Autonomous clipping* on; adjust *sensitivity*; set *chat reaction delay* to your stream latency + reaction time (low-latency Twitch ≈ 4–7 s).
3. **Live Monitor** shows messages/s, unique chatters, the Hype Map (stacked signal contributions; white line = score, red = threshold) and each detection with its reasons.

**Watching several streams:** add streamers on the Live Monitor page (or Twitch channels in Settings). Chat for each is analysed; each one automatically gets a `twitch:<name>` video source, and the selection scheduler decides which ones are buffered (*max simultaneous buffers*): favorites and priority first, trending streams only as a fallback, with a minimum watch time, switch cooldown and hysteresis. See the README section *Using PulseCut as a Twitch clipper*. Only record streams you have rights to clip.

YouTube Live chat: add the live video ID and save a YouTube Data API key (Settings → Platforms). Polling is quota-aware (≈ 5.5 h/day per key at the default 10 s interval).

Kick: official events arrive as webhooks to `http://127.0.0.1:<api port>/webhooks/kick`; you need a public tunnel to that port and Kick's public key saved in Settings. Signatures are verified.

## 4. Reviewing and editing

* **Clip Library**: preview (silent proxy; *Open in player* for audio), the detection reasons, coverage, editable title/caption/description/hashtags. **Approve**, **Reject**, **Edit** or delete.
* **Editor**: the automatic plan with its decision trail. Change format, layout (blurred fill / center / smart crop / facecam split with region sliders), captions style/position, trim, hook/watermark/outro, and each effect's timing, intensity, sound and text; add effects at the playhead; **Re-render** (uses the raw footage).
* **Assets**: preview the 13 built-in CC0 sounds, import your own sounds/images/fonts — a license or usage-rights statement is required — or drop a creator pack folder (with `pack.json`) into the assets folder and press *Reload packs*.

## 5. Publishing <a name="publishing"></a>

Nothing is uploaded until you approve a clip (or enable gated auto-publish).

**YouTube Shorts**
1. In Google Cloud Console create a project, enable *YouTube Data API v3*, configure the OAuth consent screen (add yourself as a test user) and create an **OAuth client ID → Desktop app**.
2. PulseCut Settings → Platforms: paste the client ID, save the client secret to the keychain, choose privacy (default **unlisted**), enable YouTube, Save.
3. Publishing → **Connect** (browser sign-in; PulseCut listens on a temporary 127.0.0.1 port).
4. Library → approved clip → **Publish to YouTube Shorts**. Progress and retries appear in Publishing. Interrupted uploads resume.

Quotas: YouTube currently gives a separate bucket of about 100 `videos.insert` calls/day; PulseCut caps itself at 90/day.

**Instagram Reels** (professional account required): save a Graph API token, your IG user id, and a *public media host* — a folder that your own web server/bucket/tunnel serves over https. Instagram downloads the clip from there; PulseCut deletes the hosted copy after publishing. Hosting may cost money; local clipping never needs it.

**TikTok** (experimental): uploads to your TikTok inbox as a draft; finish posting in the TikTok app. Requires your own TikTok developer app.

**Auto-publish** (Settings → Platforms): off by default; when on, only autonomous clips with confidence ≥ the threshold, full coverage and under the daily cap are published.

## 6. CLI

```
pulsecut-cli demo                     # end-to-end acceptance run (no accounts)
pulsecut-cli daemon                   # headless engine with saved settings
pulsecut-cli chat --twitch a b c      # watch live chat + detector scores
pulsecut-cli obs-check                # OBS connection / replay buffer check
pulsecut-cli render in.mp4 --moment 42 --kind fail --out clip.mp4
pulsecut-cli eval [--tune --out w.json]
pulsecut-cli encoders                 # test hardware/software encoders
echo SECRET | pulsecut-cli set-secret obs-websocket
pulsecut-cli clips | publish <id> | youtube-connect | settings
```

## 7. Storage and privacy

Library: `~/Library/Application Support/app.PulseCut.PulseCut/library/<date>/` (clip, cover, thumbnail, SRT, JSON decision trail). Raw captures in `…/raw/`. Retention and the low-disk guard are in Settings → Storage. Everything is processed locally; there is no telemetry.

## 8. Troubleshooting

| Symptom | Fix |
|---|---|
| "captured footage does not contain the moment" | Increase OBS replay buffer length (see Dashboard warning) |
| OBS "rejected the connection" | Wrong password; re-save it in Settings |
| No captions | Install whisper.cpp and a model; check Dashboard → System → Whisper |
| Hotkeys unavailable | Another app owns the combination; choose another in Settings → Hotkeys |
| macOS asks for Desktop/Documents access | PulseCut reads its library/settings; allow, or keep the library outside protected folders |
