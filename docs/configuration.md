# Configuration

PulseCut keeps its settings in one TOML file. Most settings are also editable in the app (**Settings**). Changes made in the app are written back to the file. You can edit the file by hand while PulseCut is closed.

Secrets (passwords, API keys, OAuth tokens) are never written to the settings file. They go to the OS keychain.

## Where settings live

The file is `settings.toml` in the application config folder. PulseCut finds the folder with the `directories` crate, using the application identity `app` / `PulseCut` / `PulseCut`.

| Platform | Settings file | Library, models, raw captures | Caches (work and segment buffers) |
|---|---|---|---|
| macOS | `~/Library/Application Support/app.PulseCut.PulseCut/settings.toml` | `~/Library/Application Support/app.PulseCut.PulseCut/` (`library/`, `raw/`, `models/`, `assets/`, `logs/`) | `~/Library/Caches/app.PulseCut.PulseCut/` |
| Windows | `settings.toml` in the `config` folder under `PulseCut\PulseCut` in the roaming application data folder (`%APPDATA%`) | the `data` folder under the same tree | the `cache` folder under the local application data folder (`%LOCALAPPDATA%`) |

The Windows folder layout is what the `directories` crate produces. Check the exact path on first run; the app writes the file there whenever you save settings.

The database (`pulsecut.sqlite`) lives in the data folder next to the library.

### Overriding the root with `PULSECUT_HOME`

Set `PULSECUT_HOME` to a folder to move everything under it. This is useful for tests and portable installs.

```
$PULSECUT_HOME/config/settings.toml
$PULSECUT_HOME/data/        (library/, raw/, models/, assets/, logs/, pulsecut.sqlite)
$PULSECUT_HOME/cache/       (work/, segments/)
```

`pulsecut-cli` also accepts a `--home <folder>` option for the same purpose.

### Other environment variables

| Variable | Effect |
|---|---|
| `PULSECUT_HOME` | Root folder for config, data and cache (see above). |
| `PULSECUT_KEEP_RENDER` | Used only by the render integration test. Set it to an existing folder and the test copies its rendered clip and thumbnail there for manual inspection. It has no effect on the app. |

## Settings file sections

Every section is optional. A partial file works: any missing key uses its default. Values are clamped to the ranges shown when the file is loaded, so out-of-range numbers are corrected rather than rejected. Chat channel names are lowercased and a leading `#` is removed.

Defaults below are the values in `crates/pulsecut-core/src/settings.rs`.

### `[obs]`: OBS Studio connection

| Key | Default | Notes |
|---|---|---|
| `enabled` | `false` | Turns on the OBS replay-buffer backend. |
| `host` | `"127.0.0.1"` | OBS WebSocket host. |
| `port` | `4455` | OBS WebSocket port. |
| `use_password` | `true` | Read the password from the keychain entry `obs-websocket`. |
| `auto_start_replay_buffer` | `true` | Start OBS's Replay Buffer when PulseCut connects. |
| `replay_buffer_seconds` | `120` | Replay-buffer length PulseCut assumes for OBS. The live value is read from the OBS profile when readable, and PulseCut warns if it is shorter than the clip window needs. |

### `[capture]`: how footage is kept

| Key | Default | Range / notes |
|---|---|---|
| `backend` | `"obs_replay_buffer"` | `"obs_replay_buffer"` or `"segment_buffer"`. |
| `pre_roll_secs` | `20.0` | Seconds kept before the moment. 0–600. |
| `post_roll_secs` | `10.0` | Seconds kept after the moment. 0–600. |
| `save_margin_secs` | `2.0` | Extra wait after post-roll before an OBS save, to absorb encoder and mux latency. 0–30. |
| `segment_source` | `""` | FFmpeg input for the home stream when `backend = "segment_buffer"` (for example `rtmp://`, `srt://`, a file, or `lavfi:testsrc` for development). |
| `segment_secs` | `2` | Length of each segment file. 1–30. |
| `segment_ring_secs` | `180` | Length of the rolling buffer. 30–3600. It is raised automatically to fit the replay window plus 20 s. |
| `segment_copy_codec` | `true` | Copy the source codec when possible, so buffering does not re-encode. |
| `extra_sources` | `[]` | Extra capture sources, each a table with `stream_id` and `input` (see the clipper workflow below). |
| `max_parallel_buffers` | `2` | How many extra streams are recorded at the same time. The hottest streams win. 0–8. |
| `max_clip_secs` | `60.0` | Longest clip PulseCut produces. 5–180. |

Example:

```toml
[[capture.extra_sources]]
stream_id = "twitch:somechannel"
input = "https://example.com/some-hls-playlist.m3u8"
```

### `[detection]`: when a moment counts as a clip

| Key | Default | Range / notes |
|---|---|---|
| `autonomous_enabled` | `true` | Clip automatically from chat and audio signals. |
| `sensitivity` | `0.5` | 0 (only very large moments) to 1 (clip a lot). |
| `cooldown_secs` | `45.0` | Minimum gap between detections. 0–3600. |
| `chat_reaction_delay_secs` | `6.0` | Estimated delay from the on-stream moment to the chat reaction (stream latency plus reaction time). 0–60. |
| `use_chat` | `true` | Use chat activity as a signal. |
| `use_audio` | `true` | Use the captured stream's audio as a signal. |
| `weights_file` | `""` | Path to a weights file from `pulsecut-cli eval --tune`. Empty uses the built-in tuned weights. |
| `max_clips_per_hour` | `20` | Upper bound on detections per hour. 1–120 in the app. |

### `[chat]`: what to watch

| Key | Default | Notes |
|---|---|---|
| `twitch_channels` | `[]` | Twitch channels to watch, read-only and anonymous. The first one is the home channel. Every channel also becomes a video source, see the clipper workflow. |
| `youtube_targets` | `[]` | YouTube live video IDs or channel handles. Chat needs a YouTube Data API key in the keychain. |
| `kick_channels` | `[]` | Kick channel slugs. Kick events arrive by webhook and need a public tunnel to the local API port. |
| `custom_reactions` | `[]` | Extra reaction words, merged with the built-in list. |
| `follow_hottest` | `true` | Move capture focus to the hottest channel when watching several. |

### `[audio]`: audio inputs

| Key | Default | Notes |
|---|---|---|
| `mic_enabled` | `false` | Analyse the microphone. Opt-in, and needs the macOS microphone permission. |
| `mic_device` | `""` | Input device name. Empty uses the system default. |
| `stream_audio_enabled` | `true` | Analyse the audio track of the captured stream. |

### `[voice]`: voice commands

| Key | Default | Notes |
|---|---|---|
| `enabled` | `false` | Listen for trigger phrases. Needs `[audio] mic_enabled` as well. |
| `phrases` | `["clip it", "clip that"]` | Case-insensitive, punctuation ignored. |
| `max_chunk_secs` | `3.0` | Maximum seconds of speech sent to whisper per check. |

### `[hotkeys]`: global shortcuts

| Key | Default | Notes |
|---|---|---|
| `enabled` | `true` | Register global hotkeys. |
| `clip` | `"CmdOrCtrl+Shift+K"` | Clip now. |
| `mark` | `"CmdOrCtrl+Shift+M"` | Drop a marker on the timeline without capturing. |

### `[edit]`: rendering

| Key | Default | Range / notes |
|---|---|---|
| `format` | `"vertical9x16"` | Aspect format. Other values: `"landscape16x9"`, `"square1x1"`. |
| `long_edge` | `1920` | Long edge in pixels. 320–3840. |
| `layout` | `"blur_fill"` | Layout mode, for example `"center_crop"` or `"smart_crop"`. |
| `facecam` | unset | Facecam rectangle (normalised) for the facecam split layout. |
| `captions` | `true` | Burn in captions when a whisper model is available. |
| `caption_style` | built-in default | Caption look and position. |
| `effects_enabled` | `true` | Allow meme effects from recipes. |
| `effects_intensity` | `0.6` | 0 never adds effects, 1 adds them whenever a recipe matches. 0–1. |
| `sound_effects` | `true` | Add the built-in sound effects. |
| `sfx_volume` | `0.7` | 0–2. |
| `normalize_audio` | `true` | Loudness-normalise the output. |
| `watermark_text`, `intro_text`, `outro_text` | `""` | Text overlays. Empty means none. |
| `fps` | `30` | 15–60. |
| `hardware_encode` | `true` | Use the hardware H.264 encoder when a test encode at startup passes. Otherwise libx264. |
| `also_landscape` | `false` | Also export a 16:9 version. |
| `font_path` | `""` | Font file for captions and text. Empty uses a system font (Arial Black on macOS). |

### `[meta]`: titles, captions and hashtags

| Key | Default | Notes |
|---|---|---|
| `creator_name` | `""` | Used in generated text. |
| `default_hashtags` | `["#shorts", "#gaming"]` | Added to every clip. |
| `local_llm_url` | `""` | Optional OpenAI-compatible endpoint, for example a local `llama-server`. Must be a loopback address. Empty uses templates only. |
| `local_llm_model` | `""` | Model name sent to that endpoint. |
| `thumbnail_text` | `true` | Put text on the thumbnail. |

### `[publish]`: uploads

Nothing is uploaded unless you approve a clip, or auto-publish is on.

| Key | Default | Range / notes |
|---|---|---|
| `auto_publish` | `false` | Publish clips without approval. Only autonomous clips qualify; manual clips never auto-publish. |
| `auto_publish_min_confidence` | `0.85` | Minimum detection confidence for auto-publish. 0–1. |
| `auto_publish_max_per_day` | `3` | Daily auto-publish limit. |
| `youtube_enabled` | `false` | Enable YouTube Shorts. |
| `youtube_client_id` | `""` | Your Google Cloud "Desktop app" OAuth client ID. The client secret goes to the keychain. |
| `youtube_privacy` | `"unlisted"` | Privacy for uploads. |
| `youtube_category_id` | `"20"` | YouTube category. |
| `instagram_enabled` | `false` | Enable Instagram Reels. |
| `instagram_user_id` | `""` | Instagram professional account ID. The Graph token goes to the keychain. |
| `instagram_public_base_url` | `""` | Public HTTPS address that serves `media_host_dir`. |
| `media_host_dir` | `""` | Local folder that your own web server, bucket or tunnel serves publicly. |
| `tiktok_enabled` | `false` | Enable TikTok uploads (drafts to the TikTok inbox). |
| `tiktok_client_key` | `""` | Your TikTok developer app key. The client secret goes to the keychain. |
| `max_attempts` | `5` | Attempts per upload before it is marked failed. 1–20. |

Hard caps applied by the publishing worker, regardless of settings (`crates/pulsecut-engine/src/lib.rs`):

| Platform | Maximum per day |
|---|---|
| YouTube | 90 (YouTube gives about 100 `videos.insert` calls a day) |
| Instagram | 45 (Instagram allows 50 posts per 24 hours) |
| TikTok | 15 |

Uploads over the cap wait and retry later.

### `[storage]`: library and disk

| Key | Default | Range / notes |
|---|---|---|
| `library_dir` | `""` | Library root. Empty uses the default data folder (`library/`). |
| `retention_days` | `30` | Days to keep clips. 0–365 in the app. |
| `min_free_gb` | `5.0` | Autonomous capture pauses when free disk space drops below this. |
| `delete_raw_after_render` | `false` | Delete raw captures after a successful render. |

### `[tools]`: external programs

Leave these empty to let PulseCut find them. See [setup.md](setup.md#1-install-the-tools-homebrew) for the search order.

| Key | Default | Notes |
|---|---|---|
| `ffmpeg` | `""` | Full path to `ffmpeg`. |
| `ffprobe` | `""` | Full path to `ffprobe`. |
| `whisper_cli` | `""` | Full path to `whisper-cli`. |
| `whisper_model` | `""` | Full path to a ggml model file. Empty searches the model folders. |

### `[privacy]`

| Key | Default | Notes |
|---|---|---|
| `local_crash_logs` | `true` | Keep crash logs on this machine. |
| `telemetry` | `false` | PulseCut has no telemetry backend. This flag exists but nothing sends data. |

### `[resources]`: API, render queue and memory

| Key | Default | Range / notes |
|---|---|---|
| `local_api_port` | `47120` | Port of the local control API on `127.0.0.1`. `0` disables it. |
| `max_render_jobs` | `1` | Concurrent FFmpeg render jobs. 1–8. |
| `ffmpeg_threads` | `0` | Threads per FFmpeg job. `0` uses the FFmpeg default. |
| `render_queue` | `8` | Pending render slots. When full, the lowest-scoring events are dropped. 1–256. |
| `min_free_mem_mb_for_whisper` | `700` | Transcription is skipped when free memory is below this, in MB. |

#### Local API

When `local_api_port` is not `0`, PulseCut listens on `127.0.0.1` only. The API has three routes:

| Route | Method | Purpose |
|---|---|---|
| `/api/clip` | `POST` | Clip now. Same as the hotkey. Used by Stream Deck "website" buttons and scripts. |
| `/api/status` | `GET` | JSON status. |
| `/webhooks/kick` | `POST` | Verified Kick chat webhooks. Needs a public tunnel and the Kick public key in the keychain. |

```bash
curl -X POST http://127.0.0.1:47120/api/clip
```

The API starts after the engine has finished starting, so it can take up to about a minute to answer after launch. See [troubleshooting.md](troubleshooting.md#local-api-not-reachable-for-the-first-minute).

### `[startup]`

| Key | Default | Notes |
|---|---|---|
| `launch_at_login` | `false` | Install a LaunchAgent (`~/Library/LaunchAgents/app.pulsecut.PulseCut.plist`) on macOS. |
| `start_minimized` | `false` | Start without a window. |
| `auto_connect_obs` | `false` | Connect to OBS at launch. |

## Secrets in the keychain

Secrets are stored under the service name `app.pulsecut.PulseCut` in the macOS Keychain (or Windows Credential Manager). You enter them in **Settings** with **Save to keychain**; the field then shows that a value is stored, and never shows the value again.

| Keychain entry | What it holds | Set in |
|---|---|---|
| `obs-websocket` | OBS WebSocket password | Settings → Capture & OBS |
| `youtube-api-key` | YouTube Data API key (YouTube chat) | Settings → Platforms |
| `youtube-oauth` | YouTube sign-in tokens | Publishing → Connect (written automatically) |
| `youtube-client-secret` | OAuth client secret for the Desktop app client | Settings → Platforms |
| `instagram-token` | Instagram Graph API token | Settings → Platforms |
| `tiktok-oauth` | TikTok sign-in tokens | Publishing → Connect (written automatically) |
| `tiktok-client-secret` | TikTok client secret | Settings → Platforms |
| `kick-public-key` | Kick app public key (PEM) | Settings → Platforms |

From a terminal, you can save a secret without typing it into the shell history:

```bash
printf '%s' "$OBS_PASSWORD" | target/release/pulsecut-cli set-secret obs-websocket
```

## Clipper workflow: Twitch and YouTube

This workflow turns a list of streams into clips with no OBS involved. It needs yt-dlp (`brew install yt-dlp`).

1. **Add channels.** Put Twitch channel names in `[chat] twitch_channels`, or add them in Settings → Platforms. The first channel is the home channel.

2. **Each channel becomes a source.** Every entry in `twitch_channels` automatically becomes a video source named `twitch:<channel>`, unless you already defined a source with that name in `[[capture.extra_sources]]`. This is done by `effective_sources` in `crates/pulsecut-engine/src/multistream.rs`. You do not need to write any source entries for Twitch. The first channel is the home channel: if OBS is enabled (or a rolling-buffer `segment_source` is set) it keeps that capture instead. For a pure clipper setup, turn OBS off in Settings so every listed channel is recorded from Twitch.

3. **Pick how many to record.** `capture.max_parallel_buffers` (default `2`) sets how many streams are recorded at the same time. Every 15 seconds PulseCut ranks the sources by their current hype score and records the top ones, switching focus with hysteresis so buffers do not flap. Set it to `0` to turn off multi-stream recording.

4. **Inputs are resolved with yt-dlp.** A source input of the form `twitch:<channel>` or `youtube:<value>` is turned into a direct stream URL with `yt-dlp -g` (best quality up to 1080p, 45-second timeout) each time a buffer starts. Accepted `youtube:` values are:

   * `youtube:@handle`: the channel's live page.
   * `youtube:<video id>`: the watch page for that video.
   * `youtube:<url>`: any URL starting with `http`, used as given.

   Anything else is passed to FFmpeg unchanged.

5. **Clips are written to a ready-to-post folder.** After a clip renders, PulseCut writes `<library>/ready-to-post/<clip name>/` (`write_post_package` in `crates/pulsecut-engine/src/pipeline.rs`). It contains:

   * `video.mp4`: a hard link to the clip when the file system allows it, otherwise a copy.
   * `thumbnail.jpg`
   * `post.txt`: title, caption, hashtags, description, a credit line naming the source channel, and the file list.
   * `post.json`: the same fields as structured data.

   Use this folder to post by hand, or as the public media folder for Instagram.

6. **Review, then publish.** Approve clips in the Clip Library. For unattended posting, enable `[publish] auto_publish` and set `auto_publish_min_confidence` and `auto_publish_max_per_day`. Auto-publish only applies to autonomous clips with confidence at or above the minimum, and only up to the daily limit, and only for platforms that are enabled.

Only record streams you have the right to clip.

## Starting from a fresh file

Delete `settings.toml` (or move it aside) and start PulseCut. It will create a new file with all defaults. Keychain entries are not affected.
