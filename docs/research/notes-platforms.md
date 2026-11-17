# PulseCut platform research notes (as of 2026-11-17)

Legend: **[V]** verified against official docs fetched today; **[M]** from memory / third-party, NOT verified; **[!]** changed/deprecated/surprising.

---
## 1. OBS WebSocket v5

Source: https://github.com/obsproject/obs-websocket/blob/master/docs/generated/protocol.md (fetched raw, 5798 lines) **[V]**. Latest example Hello in docs: obsStudioVersion 30.2.2, obsWebSocketVersion 5.5.2, rpcVersion 1. The protocol enum list now includes a `Canvases` EventSubscription, so master is newer than 5.5.x (exact version not checked; always call `GetVersion`).

### Handshake
1. Connect WS (default port 4455 **[M]**, path `/`; subprotocol `obswebsocket.json` or `obswebsocket.msgpack` **[M]**).
2. Server sends `Hello` (op 0): `{obsStudioVersion, obsWebSocketVersion, rpcVersion, authentication?: {challenge, salt}}`. No `authentication` key => no password required.
3. Client sends `Identify` (op 1): `{rpcVersion, authentication?: string, eventSubscriptions?: int}`; `eventSubscriptions` default = `All` (all categories except high-volume ones, which must be opted in).
4. Server replies `Identified` (op 2). Bad auth => close code `AuthenticationFailed`.
- Auth string **[V]**: `secret = base64(sha256(password + salt))`; `authentication = base64(sha256(secret + challenge))` (binary SHA256 digests, then base64).
- Subscriptions are bit flags: `Outputs = 1<<6` (64). Replay buffer / record / stream events all live in Outputs. Use `Identify.eventSubscriptions = 64` (plus `General`=1 if you want `ExitStarted`) to reduce noise; `InputVolumeMeters` etc. are high-volume and off by default.
- Requests: op 6 `Request {requestType, requestId, requestData}` -> op 7 `RequestResponse {requestStatus{result,code,comment}, responseData}`. Events: op 5.

### Requests (all `Added in v5.0.0` unless noted) **[V]**
| Request | Fields | Notes |
|---|---|---|
| StartReplayBuffer / StopReplayBuffer | none | Fails if output not configured/already running. |
| ToggleReplayBuffer | -> outputActive | |
| GetReplayBufferStatus | -> `outputActive` (bool only) | |
| SaveReplayBuffer | none | Returns immediately; file written asynchronously. Wait for `ReplayBufferSaved`. |
| GetLastReplayBufferReplay | -> `savedReplayPath` | Polling fallback to the event. |
| GetRecordStatus | -> outputActive, outputPaused, outputTimecode, outputDuration (ms), outputBytes | |
| StartRecord | none (no path returned) | |
| StopRecord | -> `outputPath` | Final file path. |
| **SplitRecordFile** | none | **Added in v5.5.0** (ships with OBS 30.2). Needs "automatic file splitting"/ a split-capable recording; see below. |
| CreateRecordChapter | `?chapterName` | v5.5.0; only Hybrid MP4 supports chapters (OBS 30.2 note). Interesting for marking highlights in a continuous recording. |
| GetStreamStatus | -> outputActive, outputReconnecting, outputTimecode, outputDuration, outputCongestion, outputBytes, outputSkippedFrames, outputTotalFrames | Use `outputDuration` to align stream time with clip timestamps. |
| GetRecordDirectory | -> `recordDirectory` | |
| GetProfileParameter / SetProfileParameter | `parameterCategory, parameterName, parameterValue` (null deletes); Get returns `parameterValue`, `defaultParameterValue` | Values are strings. Complexity rating 4/5 in docs. |

### Events **[V]**
- `StreamStateChanged {outputActive, outputState}` (v5.0.0)
- `RecordStateChanged {outputActive, outputState, outputPath}`; `outputPath` is non-null only when the record has stopped (v5.0.0)
- `RecordFileChanged {newOutputPath}` - "record output has started writing to a new file, e.g. when a file split happens" (**v5.5.0**)
- `ReplayBufferStateChanged {outputActive, outputState}`
- `ReplayBufferSaved {savedReplayPath}` (v5.0.0)
- `outputState` values: `OBS_WEBSOCKET_OUTPUT_{UNKNOWN,STARTING,STARTED,STOPPING,STOPPED,RECONNECTING,RECONNECTED,PAUSED,RESUMED}`.

### Does SaveReplayBuffer include footage AFTER the call? No.
The replay buffer is a rolling in-memory ring of already-encoded packets covering the last N seconds (or up to a size cap). `SaveReplayBuffer` flushes the packets currently in the ring to a file; it does not arm a "keep recording for X more seconds" mode. Nothing in the protocol docs offers post-roll **[V: no such field]**. Practical consequences **[M on internals]**:
- Saved length is roughly <= configured seconds, cut at a keyframe boundary, so actual start can be a bit earlier than N sec back (GOP-dependent; set keyframe interval 1-2 s in OBS for tighter cuts).
- Moment of trigger = end of clip. To get post-roll, PulseCut must DELAY the `SaveReplayBuffer` call by `post_roll` seconds after the detected peak, and set buffer length >= pre_roll + post_roll + safety (e.g. 60-90 s).
- Overlapping triggers: each save produces a separate file; a second save within the window re-includes the same footage. De-dupe/merge in PulseCut (merge peaks within window into one longer clip by saving once, later).
- Saves are async: await `ReplayBufferSaved` (match by order; the event has no request id) and/or call `GetLastReplayBufferReplay`. Do not trust the file is complete until the event arrives; additionally poll file size stability.
- Replay buffer must be running (`StartReplayBuffer`); with "Stop replay buffer on stream end" semantics varying - check `GetReplayBufferStatus` before every save. OBS must have the replay buffer enabled in Settings > Output (Simple: "Enable Replay Buffer"; Advanced: Replay Buffer tab) **[M]**.
- Saved file container follows the recording format setting; fragmented/Hybrid MP4 is safer than MKV->MP4 remux for FFmpeg consumption (MKV is also fine). **[M]**

### Configuring max seconds
- Profile config (basic.ini) keys **[M - not in the protocol doc; verify with `GetProfileParameter` on a real install]**: section `SimpleOutput` and section `AdvOut`, keys `RecRB` (bool, enable), `RecRBTime` (seconds, default 20), `RecRBSize` (MB cap). So via protocol: `SetProfileParameter{parameterCategory:"SimpleOutput", parameterName:"RecRBTime", parameterValue:"90"}` (or category `AdvOut`). Which section applies depends on `Output.Mode` (`Simple`/`Advanced`) which can be read via `GetProfileParameter{category:"Output", name:"Mode"}`.
- UNCERTAIN: whether a running replay buffer picks up a changed value. Assume it requires `StopReplayBuffer` -> set -> `StartReplayBuffer`. Test empirically; surface a "restart replay buffer" step in PulseCut setup.
- Alternative that avoids OBS-side config quirks: PulseCut tells the user to set it manually; verify via `GetProfileParameter` and warn if too short.

### File splitting (automatic recording split) in OBS 30+
- Automatic file splitting by time/size exists in Settings > Output > Recording (Advanced mode) since OBS 28 (my recollection; web search only confirmed "OBS 28" via a third-party video blurb, official release notes not fetched) **[M]**. Config keys in `AdvOut`: `RecSplitFile`, `RecSplitFileType` (Time/Size/Manual), `RecSplitFileTime` (min), `RecSplitFileSize` (MB) **[M]**.
- `SplitRecordFile` request + `RecordFileChanged` event arrive in obs-websocket 5.5.0 (OBS 30.2, mid-2024) **[V for protocol]**. Whether `SplitRecordFile` requires "Manual" split mode vs. any setting: forum thread said the hotkey only works when automatic splitting is enabled **[M, low confidence]**. Test.
- Design implication: "continuous recording + `SplitRecordFile` every 30-60 s + `RecordFileChanged` to learn file names" gives a disk-backed rolling buffer you can cut from at any time with post-roll, no ring-buffer limits. Cost: disk usage (prune old segments yourself).

### Rust crate `obws`
- Latest: **0.15.0**, published 2026-03-13 (crates.io API **[V]**). 25 versions total. Repo moved to https://forge.dnaka91.rocks/dnaka91/obws (GitHub repo is a redirect stub **[V]**). Docs: https://docs.rs/obws/0.15.0/obws/
- Client API **[V from docs.rs]**: `client.replay_buffer()`: `status()->bool, toggle, start, stop, save, last_replay()->String`. `client.recording()`: `status()->RecordStatus, toggle, start, stop()->String (path), toggle_pause, pause, resume, split_file(), create_chapter(Option<&str>)`. => covers v5.5 requests. `streaming().status()` and `config()`/`profiles().set_parameter` presumably exist **[M, not checked this session]**; check `client.profiles()` before relying on it.
- Features (from crates.io, partial): `events`, `tls`, `builder` (bon). `events` feature gives `client.events()` stream (`Event::ReplayBufferSaved{path}` naming not checked). Tokio-based async. Check MSRV in Cargo.toml.
- Fallback: speak raw v5 over `tokio-tungstenite` + `sha2` + `base64` - ~150 lines, avoids crate lag behind protocol.

---
## 2. Twitch chat

### Anonymous IRC over WebSocket
Source: https://dev.twitch.tv/docs/chat/irc/ **[V]** (pages fetched today).
- Endpoint: `wss://irc-ws.chat.twitch.tv:443` **[V]**; non-TLS WebSocket is decommissioned **[V]**. Raw IRC TLS: `irc.chat.twitch.tv:6697`.
- Capabilities: `CAP REQ :twitch.tv/tags twitch.tv/commands` (+ optionally `twitch.tv/membership` for JOIN/PART, usually unneeded). `tags` requires `commands` as well **[V]**. Server answers ACK/NAK.
- Anonymous login `PASS SCHMOOPIIE` / `NICK justinfan12345` (any `justinfanNNNN`) then `JOIN #channel` **[M]**: the IRC doc page fetched today does not mention justinfan; it is long-standing community-documented behavior. Read-only: sends are ignored. Risk: Twitch could drop it, but no deprecation notice found.
- Tags: IRCv3 `@k=v;k=v ...` (values `\s`, `\:` escaped per IRCv3) **[V]**. Parse only known tags. Useful: `emotes`, `badges`, `tmi-sent-ts` (ms epoch; use for chat-rate timing), `id`, `user-id`, `display-name`, `room-id`, `bits`, `first-msg`.
- `emotes` tag **[V]**: `emotes=25:0-4,12-16/1902:6-10` = `/`-separated `<id>:<start>-<end>[,<start>-<end>...]`, zero-based inclusive character offsets into the message text (unicode code points, so count code points not bytes in Rust - use `chars()`); for `/me` messages (`\x01ACTION ...`) positions are relative to text after `\x01ACTION `. Resolve IDs via Get Global/Channel Emotes (needs token); emote image CDN `https://static-cdn.jtvnw.net/emoticons/v2/{id}/default/dark/1.0` **[M]**. For hype detection you can count emote-heavy messages without resolving.
- Keepalive: respond to `PING :tmi.twitch.tv` with `PONG :tmi.twitch.tv`. Handle `RECONNECT` command. **[M]**
- Rate limits **[V]**: join attempts 20/10 s per user (verified bots 2000/10 s); IRC auth attempts 20/10 s; max 100 concurrent channel joins per account (May 2024 change). Message-send limits (20/30 s, etc.) irrelevant for read-only. For anonymous connections limits apply per IP **[M]**.
- Status: **not deprecated.** Twitch "recommends" EventSub over IRC (more limitations, harder to parse) but gives no sunset date **[V: irc/ and irc-migration pages]**. IRC is still the only way to read chat with zero auth.

### EventSub `channel.chat.message`
- Needs a user access token with `user:read:chat` from the chatting user (IRC needed `chat:read`) **[V: irc-migration page]**. With an App Access Token the subscription additionally needs `user:bot` from the chatting user and either mod status or `channel:bot` from the broadcaster **[V]**. Forum reports: WebSocket transport requires a *user* token (app token => "subscription missing proper authorization") **[M, forum]**.
- Therefore **no anonymous EventSub**. A desktop app would need Twitch app registration (client id), OAuth (Device Code flow suits desktop: https://dev.twitch.tv/docs/authentication/getting-tokens-oauth/ **[M]**), then EventSub WebSocket `wss://eventsub.wss.twitch.tv/ws` **[M]**.
- **Recommendation:** Default to anonymous IRC (zero setup); optionally add EventSub login later for richer data (reliable emote fragments, cheers, subs). Abstract chat source behind a trait.

---
## 3. YouTube Live chat

Sources: https://developers.google.com/youtube/v3/live/docs/liveChatMessages/list, .../streamList, https://developers.google.com/youtube/v3/determine_quota_cost, revision history **[V]**.

- Find chat id: `videos.list?part=liveStreamingDetails&id=VIDEO_ID` -> `liveStreamingDetails.activeLiveChatId` (1 unit) **[V: guide text]**. Works for any public live video id (no ownership needed). Alternatively `liveBroadcasts.list` (`mine=true` or `broadcastStatus=active`; 1 unit) returns `snippet.liveChatId` but requires OAuth scope `youtube.readonly`/`youtube`/`youtube.force-ssl` **[V]** and only covers the user's own broadcasts.
- `liveChatMessages.list` (GET `https://www.googleapis.com/youtube/v3/liveChat/messages`): params `liveChatId, part (id,snippet,authorDetails), pageToken, maxResults (200-2000, default 500), hl, profileImageSize`; response has `nextPageToken`, `pollingIntervalMillis` (obey it; typically ~5-10 s but can be lower), `offlineAt`. Cost **5 units/call [V]**. Polling at 5 s = 12 calls/min = 60 units/min = 3,600 units/h; a 3-hour stream ~10,800 units > 10,000/day default. **[!]** Polling does not scale; respect actual `pollingIntervalMillis` (often larger) and budget.
- `liveChatMessages.streamList`: "server-streaming connection" with low latency; first returns recent history, then pushes; resume with last `nextPageToken`; cost **5 units per call (per connection, not per message) [V]**; same params (`liveChatId, part, hl, maxResults, pageToken, profileImageSize`). Docs recommend it as most efficient. Transport is **gRPC** (the guide at https://developers.google.com/youtube/v3/live/streaming-live-chat builds a gRPC client from `stream_list.proto`) **[V via search snippet]**. In Rust: `tonic` + `prost` with the proto from the guide; the demo takes "API_KEY_OR_OAUTH_TOKEN" as argument => API key accepted **[V via search snippet; medium confidence]**. Whether the stream can run for hours at 5 units total: cost described per call; an open stream could reconnect periodically - measure.
- API key for public streams: The current `list` and `streamList` doc pages show no scope table (so no mandatory OAuth listed) **[V]**; community practice is that API key works for public live chats **[M]**; the streamList demo explicitly accepts an API key **[V via search]**. Live chat disabled/members-only/restricted chats need OAuth. Treat as: API key = default path; OAuth optional fallback. Verify with a real key before shipping.
- Default quota: 10,000 units/day for all non-bucketed endpoints; resets midnight Pacific **[V]**.
- Message types of interest: `snippet.type` (`textMessageEvent`, `superChatEvent`, `newSponsorEvent`...), `snippet.publishedAt` (RFC3339, use for timing), `snippet.displayMessage`. Emotes appear inline as text (":emoji:") or `:_name:` shortcodes **[M]**.
- Alternative without quota: scraping the InnerTube chat endpoint (what pytchat does) - unofficial, brittle, ToS gray. Don't.

---
## 4. Kick

Source: https://docs.kick.com/events/* **[V]**.
- Official public API (OAuth 2.1 style at `id.kick.com`): `/oauth/authorize` (PKCE **S256 required**), `/oauth/token` (authorization_code, refresh_token, client_credentials), `/oauth/revoke`, `/oauth/token/introspect` **[V]**. Scope names not listed on the page fetched; `events:subscribe` confirmed on the subscription page **[V]**.
- Events are **webhook-only**: `POST https://api.kick.com/public/v1/events/subscriptions` with `method: "webhook"` (the only allowed value; no websocket transport documented) and `events:[{name,version}]`; user token => needs `events:subscribe` and broadcaster is inferred; app token => must pass `broadcaster_user_id` **[V]**. Limits: 10,000 subs per event type; **`chat.message.sent` capped at 1,000 for unverified apps [V]**.
- Implication: A desktop app has no public HTTPS endpoint. Options: (a) relay service owned by PulseCut (Cloud function receives webhooks, app polls/streams from it) - extra infrastructure, privacy; (b) user tunnel (ngrok/cloudflared) - heavy; (c) unofficial Pusher. Webhook URL is configured in the Kick app settings page (not documented in the pages fetched) **[uncertain]**.
- `chat.message.sent` v1 payload **[V]**: `message_id, replies_to{message_id,content,sender}, broadcaster{...}, sender{is_anonymous,user_id,username,is_verified,profile_picture,channel_slug,identity{username_color,badges[{text,type,count?}]}}, content, emotes[{emote_id,positions[{s,e}]}], created_at`. Headers: `Kick-Event-Type: chat.message.sent`, `Kick-Event-Version: 1`, `Kick-Event-Subscription-Id`.
- Signature verification **[V: https://docs.kick.com/events/webhook-security]**: headers `Kick-Event-Message-Id` (ULID, idempotency key), `Kick-Event-Message-Timestamp` (RFC3339), `Kick-Event-Signature` (base64). Signed string = `{message_id}.{timestamp}.{raw_body}` (raw bytes, never re-serialised). RSA PKCS#1 v1.5 + SHA-256 against Kick's PEM (SPKI "PUBLIC KEY") fetched at runtime from `https://api.kick.com/public/v1/public-key` (do not hardcode; refresh on failure). Rust: `rsa` crate `VerifyingKey::<Sha256>` + `pkcs8`/`DecodePublicKey`.
- Unofficial Pusher WebSocket **[M, third-party sources: docs.rs/kick-api, Apify actor]**: connect to Pusher (`wss://ws-us2.pusher.com/app/<app key>?...`, key not reproduced here - take from community lib), subscribe channel `chatrooms.{chatroom_id}.v2`, event `App\Events\ChatMessageEvent`, payload `data` is a JSON *string* (double-encoded). chatroom id comes from Kick's internal `/api/v2/channels/{slug}` (frequently Cloudflare-challenged). No auth for public chat; sub-only rooms need login. Unsupported, can break any time; Rust crate `kick-api` wraps it. Recommendation: ship Kick as "experimental/community-protocol" plugin behind a trait; official webhook path as the roadmap.

---
## 5. YouTube upload (videos.insert)

Sources: https://developers.google.com/youtube/v3/docs/videos/insert, .../guides/using_resumable_upload_protocol, .../revision_history **[V]**.
- **[!] Two big 2025-2026 changes (revision history):**
  - Dec 4, 2025: upload quota cost dropped from ~1,600 units to ~100 units. June 1, 2026: granular quota - **`videos.insert` now has its own bucket: 100 calls/day default, 1 unit per call**, `search.list` likewise; everything else shares 10,000 units/day **[V: determine_quota_cost + insert page "Quota impact: 100 calls per day"]**.
  - **Nov 2, 2026 (two weeks ago): "Videos uploaded through videos.insert from unverified API projects are no longer restricted to private viewing mode"**, reversing the July 28 2020 policy. If `privacyStatus` omitted, defaults to public (some uploads may still default private per creator account settings) **[V]**. Many older blog posts are therefore outdated. Still set `privacyStatus` explicitly (`private`/`unlisted` default for review-before-publish UX). Note: an OAuth app in "Testing" status with unverified consent screen still shows a warning and issues refresh tokens that expire in 7 days **[M]** - this is Google OAuth policy, separate from the API restriction. `youtube.upload` is a sensitive scope -> verification needed for >100 users **[M]**.
- Endpoint `POST https://www.googleapis.com/upload/youtube/v3/videos`; scopes (any): `youtube.upload` (least privilege, recommended), `youtube`, `youtube.force-ssl`, `youtubepartner` **[V]**. Max 256 GB; MIME `video/*` or `application/octet-stream` **[V]**.
- Resumable protocol **[V]**: (1) `POST .../upload/youtube/v3/videos?uploadType=resumable&part=snippet,status[,contentDetails]` with `Authorization: Bearer`, `Content-Type: application/json; charset=UTF-8`, `X-Upload-Content-Length`, `X-Upload-Content-Type: video/*`, JSON metadata body. (2) `200 OK` returns `Location:` session URI. (3) `PUT` bytes to session URI (single request, or chunks with `Content-Range: bytes a-b/total`; chunks multiples of 256 KB; intermediate responses `308`). (4) On interruption: `PUT` empty body with `Content-Range: bytes */total` -> 308 with `Range` header says what was received; resume from there. Retry 5xx with exponential backoff. Final 200/201 returns the video resource. Rust: `reqwest` manual (30 lines) beats generated clients.
- `status.publishAt` (RFC3339) schedules a public release; requires `privacyStatus: "private"` **[M]**. `status.selfDeclaredMadeForKids` should be set explicitly (required for compliance) **[M]**. `status.containsSyntheticMedia` field exists **[V list]**. New: if `paidProductPlacementDetails.hasPaidProductPlacement` omitted, video is not declared as free of paid placement **[V revision history]**.
- Shorts: no API flag. YouTube classifies a video as a Short if it is square/vertical and **<= 3 minutes** (raised from 60 s in Oct 2024) **[M: only third-party sources; YouTube Help page not fetched]**. `#Shorts` in title/description is a helper, not a requirement. Output 1080x1920, 9:16, H.264/AAC.
- Desktop OAuth **[M, standard Google docs: https://developers.google.com/identity/protocols/oauth2/native-app]**: loopback redirect `http://127.0.0.1:<random port>` (custom URI schemes and OOB are blocked/deprecated - OOB flow shut down 2023), `code_challenge` S256 PKCE, `access_type=offline` for refresh token, client secret for "Desktop app" type is not confidential but still sent at token exchange. Store refresh token in macOS Keychain (`keyring` crate).

---
## 6. Instagram Reels

Source: https://developers.facebook.com/docs/instagram-platform/instagram-api-with-instagram-login/content-publishing **[V]** (examples use Graph `v27.0`; use `<LATEST_API_VERSION>`).
- Two API families: **Instagram API with Instagram Login** (graph.instagram.com / host `graph.instagram.com`, permissions `instagram_business_basic`, `instagram_business_content_publish`) vs **Facebook Login** (graph.facebook.com; `instagram_basic`, `instagram_content_publish`, `pages_read_engagement`; account linked to a Facebook Page) **[V]**. Instagram Login is simpler (no Page needed) **[M]**, but note the doc still says professional account connected to a Page for the Facebook route and Page Publishing Authorization (PPA) blocks publishing until completed **[V]**.
- Account: Instagram **professional** (Business or Creator) account **[V]**. App must be in Live mode with advanced access / app review for `*_content_publish` for non-tester users **[M]**.
- Flow **[V]**: (1) `POST /<IG_ID>/media` `media_type=REELS`, `video_url=<publicly reachable URL>` (+ `caption`, `share_to_feed`, `cover_url`/`thumb_offset`, etc. **[M]**) -> container id. (2) Poll `GET /<container_id>?fields=status_code` -> `IN_PROGRESS|FINISHED|ERROR|EXPIRED|PUBLISHED`; docs: check once/minute for up to 5 min. (3) `POST /<IG_ID>/media_publish?creation_id=<container>`.
- **Resumable upload (no public hosting)**: create container with `upload_type=resumable` (omit video_url), then `POST https://rupload.facebook.com/ig-api-upload/<API_VERSION>/<CONTAINER_ID>` with headers `Authorization: OAuth <token>`, `offset: 0`, `file_size: <bytes>` and the raw file as body (or `file_url: <url>` header for hosted file) **[V]**. So yes, a local file can be uploaded directly without public hosting. **Caveat [!]:** the fetched doc text says resumable upload is limited to apps using "Facebook Login for Business" - ambiguous whether it applies to Instagram Login; test with a real account before designing around it. Fallback: temporary public host (pre-signed URL on S3/R2/GCS with short expiry) **[M]**.
- Rate limit: docs page contradicts itself: 100 vs 50 API-published posts per rolling 24 h; carousel = 1 post. Query `GET /<IG_ID>/content_publishing_limit` for live quota **[V]**.
- Reels specs not on that page; from memory **[M]**: MP4/MOV, H.264/HEVC, AAC, 9:16 recommended (0.01:1 to 10:1 accepted), 3 s-15 min (older 90 s limit raised), <= 1 GB (resumable upto 300 MB? unverified), 23-60 fps, max 1920 px width.
- App Review required for production use by other people's accounts; development-mode works for app's own roles. That is a heavy gate for a distributed desktop app: consider "export + reveal in Finder + user posts manually" as the default for IG.

---
## 7. TikTok Content Posting API

Official docs (developers.tiktok.com/doc/content-posting-api-*) returned HTTP 503 / JS-only to every fetch today, so everything below is **[M]** + third-party summaries; re-verify.
- Two modes: **Direct Post** (`POST /v2/post/publish/video/init/`, scope `video.publish`) and **Upload to inbox/draft** (`POST /v2/post/publish/inbox/video/init/`, scope `video.upload`; user finishes in the TikTok app). Query creator constraints first: `POST /v2/post/publish/creator_info/query/` (allowed privacy levels, max duration, comment/duet/stitch flags). Status: `POST /v2/post/publish/status/fetch/` with `publish_id`.
- `source_info.source`: `FILE_UPLOAD` (get `upload_url`, PUT chunks) or `PULL_FROM_URL` (TikTok pulls from a URL under a domain/URL prefix you verified in the developer portal).
- FILE_UPLOAD chunks: chunk_size 5 MB-64 MB (final chunk may be up to 128 MB), up to 1000 chunks, files < 5 MB sent whole; `Content-Range: bytes a-b/total`, `Content-Type: video/mp4`; upload_url valid ~1 h; video <= 4 GB; duration limit per creator_info (commonly up to 10 min, newer up to 60 min) **[M, from memory, verify]**.
- Audit gate **[M, consistent across 5 third-party sources, e.g. https://www.postzen.dev/blog/tiktok-content-posting-api-errors, https://www.upload-post.com/tiktok-api/]**: unaudited clients can only post `SELF_ONLY` (private), only to accounts that are private at post time (error `unaudited_client_can_only_post_to_private_accounts`), capped (~5 users per 24 h). Audit/app review needed for public Direct Post. Inbox/draft upload is reportedly not subject to the private-only rule while awaiting audit.
- OAuth: Login Kit v2, `authorize` + PKCE for desktop (S256), redirect must be registered (loopback may not be accepted; custom scheme/`localhost` support unverified) **[M]**; access token ~24 h, refresh token ~365 d.
- Recommendation: Implement as **Upload-to-inbox (video.upload)** first; Direct Post later after audit; or export-only.

---
## 8. whisper.cpp CLI (`whisper-cli`)

Source: https://github.com/ggml-org/whisper.cpp (examples/cli/cli.cpp, README.md, models/README.md, fetched raw) **[V]**. Binary renamed `main` -> `whisper-cli` **[!]** (old `main` is deprecated).
- Input must be 16-bit 16 kHz WAV (mono): `ffmpeg -i in.mp4 -ar 16000 -ac 1 -c:a pcm_s16le out.wav` **[V]**. (FFmpeg-enabled build can read other formats; default build cannot.)
- Word timestamps **[V]**:
  - `-ml 1` (`--max-len 1`): segment max length in characters = 1 => one segment per word/token group (README: "to obtain word-level timestamps"). Combine with `-sow` (`--split-on-word`) so splits occur on word boundaries not tokens; otherwise sub-word fragments appear. Output is `[start --> end] word`. Also `-ml 16` etc for subtitle-width lines.
  - `-wt N` word timestamp prob threshold (default 0.01).
  - `-oj` writes `<out>.json`; `-ojf` (`--output-json-full`) adds per-token array `tokens[]: {text, id, p, t_dtw, offsets?, timestamps?}`. JSON has `transcription[]` each with `timestamps{from,to}` ("00:00:01,230") and `offsets{from,to}` in **milliseconds** (`t*10`). Token-level timestamps in `-ojf` are available even without `-ml 1` - best for word timing with no segment hacks (token-merge into words yourself: tokens beginning with a space start a new word). Token `t_dtw` is -1 unless `--dtw` used.
  - `-owts`: generates a bash script (`.wts`) that renders karaoke-style ffmpeg video; not useful for us, but it shows word timing. `-ovtt/-osrt/-ocsv` other outputs; `-of <prefix>` output file base; `-np` no prints; `-nt` no timestamps.
  - `--dtw MODEL` (`-dtw`): token-level timestamps via Dynamic Time Warping using alignment heads preset named after the model (`tiny.en, base.en, small.en, medium.en, large.v3, ...`); must match the loaded model; experimental; README cautions to not combine with flash attention **[M]** (`-fa` default is on in newer builds? check; use `-nfa` if present, unverified).
- Models (disk) **[V]**: tiny/tiny.en 75 MiB; base/base.en 142 MiB; small/small.en 466 MiB; medium 1.5 GiB; large-v3 2.9 GiB; `large-v3-turbo-q5_0` 547 MiB; `large-v3-turbo` 1.5 GiB. Quantized `-q5_0`/`-q8_0` available. Memory at runtime: tiny ~273 MB, base ~388 MB, small ~852 MB **[V]**. Download: `./models/download-ggml-model.sh base.en`. Recommendation: `base.en` default (good speed on Apple Silicon w/ Metal), `small.en` option; Core ML encoder (`generate-coreml-model.sh`) ~3x faster on ANE **[V: README describes it]**; Metal is on by default on macOS **[M]**.
- Short-clip tuning: `-t 4..8`, `-nth` no-speech threshold (default 0.6), `-bs 1 -bo 1` for speed (greedy), `-mc 0` to avoid context carry-over hallucination on short clips **[M]**, `-sns` suppress non-speech tokens, `-l en`.
- VAD **[V]** (new in 2025): `--vad` + `-vm <ggml-silero-v6.2.0.bin>` (download via `./models/download-vad-model.sh silero-v6.2.0`, ~864 KB). Options: `-vt` threshold, `-vspd` min speech ms, `-vsd` min silence ms, `-vmsd` max speech s, `-vp` speech pad ms, `-vo` samples overlap. Only detected speech is sent to Whisper (faster, fewer hallucinations on silence/music); timestamps are re-mapped to original timeline **[M, expected but verify with a test file]**.
- Rust integration: either shell out to `whisper-cli` (simple, process isolation, what the notes assume) or `whisper-rs` crate (bindings; version not checked).

---
## 9. FFmpeg

### Segment muxer for rolling buffer **[V: https://ffmpeg.org/ffmpeg-formats.html#segment]**
```
ffmpeg -i <rtmp/srt/udp in> -c copy -f segment -segment_time 5 -segment_format mp4 \
  -segment_format_options movflags=+faststart -reset_timestamps 1 -strftime 1 \
  -segment_list live.csv -segment_list_type csv -segment_list_flags live \
  -segment_wrap 120 -segment_start_number 0 "seg_%Y%m%d_%H%M%S.mp4"
```
- `-segment_time N`: target duration (default 2); with `-c copy`, cuts only at keyframes, so set source GOP (OBS keyframe interval 1-2 s) or re-encode with `-force_key_frames "expr:gte(t,n_forced*5)"`. `-segment_times` explicit list; `-segment_atclocktime 1` aligns to wall clock (+ `-segment_clocktime_offset`).
- `-segment_wrap N`: wrap segment index after N (rolling ring of N files; file names must use `%d`, not strftime; `-strftime 1` names by time and then you prune manually - they don't combine for wrapping) **[M on exclusivity]**. For analysis/cuts prefer time-named files + your own pruner (Rust) because wrapped names lose chronology.
- `-reset_timestamps 1`: each segment's timestamps start at 0 (good for independent playback; bad for correlating - then use the csv list). `-segment_list <file> -segment_list_type csv` writes `filename,start_time,end_time` lines (seconds, relative to input start; `ext` is deprecated alias) **[V]**; `ffconcat` list type produces concat-demuxer-compatible lists **[V]**; `m3u8` also available. `-segment_list_size N`, `-segment_list_flags live`.
- `-segment_time_delta`, `-break_non_keyframes 1` (don't use with copy for players), `-segment_format mkv/mpegts` (mpegts/mkv more robust to abrupt kill than mp4) **[M]**. If the app crashes mid-segment MP4 is unplayable; use MPEG-TS or fragmented MP4 (`-movflags +frag_keyframe+empty_moov`) **[M]**.
- If OBS is the source via ReplayBuffer, you don't need this; use this only for the "OBS -> local SRT/RTMP -> ffmpeg" ingest alternative.

### Accurate cutting across segments
1. Pick the segments overlapping [t0,t1] from the list, write an `ffconcat` file:
```
ffconcat version 1.0
file 'seg_a.mp4'
inpoint 3.2         # seconds within this file (after reset_timestamps)
file 'seg_b.mp4'
file 'seg_c.mp4'
outpoint 1.8
```
`ffmpeg -f concat -safe 0 -i list.ffconcat -c copy out.mp4` is fast but with `-c copy` `inpoint` snaps to the previous keyframe (not frame-accurate) **[M, concat demuxer docs: https://ffmpeg.org/ffmpeg-formats.html#concat]**. Note: concat demuxer docs say `inpoint` with stream copy may be inexact and `outpoint` is exclusive; files must share codec params.
2. Frame-accurate path: re-encode the clip (vertical crop/scale + captions need re-encode anyway): `ffmpeg -f concat -safe 0 -i list.ffconcat -ss <t0_rel> -t <dur> -c:v h264_videotoolbox ...` (put `-ss` after `-i` for accuracy; decode from the segment start so it is exact).
3. Even simpler for fewer than ~3 segments: concat with `-c copy` to a temp file (cheap), then single accurate trim+render pass. Always `-avoid_negative_ts make_zero` when copying; `-fflags +genpts` if PTS errors.
4. For OBS replay-buffer files you only have one file - plain `-ss/-to` on it.

### `h264_videotoolbox` (macOS) **[M: ffmpeg-codecs page fetched today had no VideoToolbox section; verify with `ffmpeg -h encoder=h264_videotoolbox`]**
- Rate control: `-b:v 8M` (avg bitrate; default is poor); `-maxrate`/`-bufsize` loosely honored; `-q:v 1-100` constant-quality mode on Apple Silicon in FFmpeg >= 7.x (higher = better) - behavior varies; `-constant_bit_rate 1` (newer FFmpeg, macOS 13+) **[M]**.
- `-profile:v high|main|baseline`, `-level 4.2`, `-coder cabac`, `-allow_sw 0/1`, `-require_sw`, `-realtime 1` (low latency), `-prio_speed 1` (prefer speed over quality), `-pix_fmt yuv420p`, `-g 60` (GOP), `-bf 0` to disable B-frames if needed, `-tag:v avc1`, `-movflags +faststart` for MP4. Add `-an`/`-c:a aac -b:a 160k`.
- Typical clip render: `ffmpeg -y -f concat -safe 0 -i list.ffconcat -vf "crop=ih*9/16:ih,scale=1080:1920,subtitles=cap.ass" -c:v h264_videotoolbox -b:v 8M -profile:v high -pix_fmt yuv420p -c:a aac -b:a 160k -movflags +faststart out.mp4`. `subtitles=`/libass requires an FFmpeg build with `--enable-libass` (Homebrew has it).
- Quality/size: VideoToolbox is less efficient than x264 at equal bitrate; fine for short social clips. Fall back to `libx264 -preset veryfast -crf 20` if unavailable. `hevc_videotoolbox` exists but social uploads prefer H.264.
- Licensing: shipping FFmpeg binary inside app (LGPL/GPL build) requires compliance; consider requiring user-installed ffmpeg or LGPL-only static build **[M]**.

---
## Decision-relevant open items to verify with real accounts
1. OBS `RecRBTime` category names and whether runtime changes apply without restarting the buffer.
2. YouTube `streamList` + API key (gRPC) and actual quota consumption over a long stream.
3. Instagram resumable upload eligibility under Instagram Login.
4. TikTok official numbers (docs unreachable today).
5. obws `profiles().set_parameter` existence and event type names.
