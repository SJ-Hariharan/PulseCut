# Troubleshooting

Each entry gives the symptom as it appears (in the app, in the log, or in a terminal), the cause, and the fix. Settings are described in [configuration.md](configuration.md). Installation problems are covered in [setup.md](setup.md).

## OBS

### "OBS rejected the connection"

The message is `OBS rejected the connection (check the WebSocket password)`.

**Cause:** the password PulseCut has in the keychain does not match the OBS WebSocket server password. This is usually a stale value after you changed the password in OBS.

**Fix:**

1. In OBS, open **Tools → WebSocket Server Settings** and copy the current password.
2. In PulseCut, open **Settings → Capture & OBS**, paste the password, press **Save to keychain**, then **Save & apply**.

If the password is correct and it still fails, check that the host and port match the OBS WebSocket server (default `127.0.0.1`, port `4455`).

### "OutputNotRunning" or "SaveReplayBuffer failed" when clipping

The error text is `OBS SaveReplayBuffer failed (is the replay buffer running?)`. OBS reports `OutputNotRunning` when it is asked to save a replay buffer that is not running.

**Cause:** OBS's Replay Buffer is off. PulseCut can start it for you, but only when `[obs] auto_start_replay_buffer` is on and PulseCut is connected.

**Fix:** In OBS, click **Start Replay Buffer** in the Controls panel, or enable **Settings → Output → Replay Buffer**. Then clip again.

### "captured footage does not contain the moment"

The message is `captured footage does not contain the moment (missing N ms); increase the replay buffer length`.

**Cause:** the replay buffer did not hold enough footage from before the moment. Either:

* the buffer has not been running long enough yet (for example, just after you started streaming or the buffer), or
* the buffer length in OBS is shorter than the clip window needs.

The clip window is pre-roll plus post-roll plus the save margin, plus the chat reaction delay when chat detection is on. With the defaults that is about 42 seconds. The Dashboard shows a warning when the OBS buffer is shorter than this.

**Fix:**

* Wait for the buffer to fill, then clip again.
* In OBS, raise **Settings → Output → Replay Buffer → Maximum Replay Time** to at least the value PulseCut shows in **Settings → Capture**. Restart the buffer after changing it.
* Or lower `pre_roll_secs` or `post_roll_secs` in Settings.

## Local API

### Local API not reachable for the first minute

Calls to `http://127.0.0.1:47120/api/clip` (or `/api/status`) fail for up to about a minute after PulseCut starts.

**Cause:** the local API starts only after the engine has finished starting up (database, assets, encoder checks, and so on). If a macOS Keychain prompt is waiting for your answer, startup can pause until you respond.

**Fix:** Look for a pending Keychain dialog and click **Always Allow** (or **Allow**). Then wait and retry. If the API still does not answer after a few minutes, check the PulseCut log and confirm `local_api_port` is not `0`.

## Text input

### Paste does not work in text fields

**Cause:** this build was compiled without the `clipboard` feature of eframe, so the text fields cannot read the system clipboard.

**Fix:** This is fixed in 0.1.0 and later. The workspace `Cargo.toml` enables the feature (`eframe` with `features = [... "clipboard"]`). Rebuild from the current source. If you build your own variant, keep that feature enabled.

## YouTube

### Google sign-in fails with "Error 403: access_denied"

**Cause:** the Google Cloud project is in testing mode, and your Google account is not on its list of test users.

**Fix:** In Google Cloud Console, open the OAuth consent screen for your project and add your Google account under **Test users**. Then press **Connect** in PulseCut's Publishing page again. The client must be a Desktop app OAuth client, as described in [USER_GUIDE.md](USER_GUIDE.md#publishing).

### Uploads are private, not unlisted or public

**Cause:** YouTube restricts uploads made through the API by projects that have not passed Google's API compliance audit to private videos. PulseCut sends the privacy you chose (`[publish] youtube_privacy`, default `unlisted`), but YouTube overrides it. Scheduled uploads are also private until their publish time.

**Fix:** Change the video's visibility in YouTube Studio after upload. To remove the restriction, request an audit for your Google Cloud project through the Google API Services compliance process.

## Twitch and other live sources

### Source shows "offline or unavailable"

The message is `<input>: offline or unavailable (...)`. PulseCut resolves `twitch:<channel>` and `youtube:` inputs with yt-dlp each time it starts a buffer. The message comes from one of two cases:

* **The channel is offline.** There is no stream to record. Source selection runs about every 15 seconds, so a buffer starts once the stream is live.
* **yt-dlp is missing or outdated.** If the message ends with `yt-dlp not available`, yt-dlp is missing. If the channel is live and the error mentions an extractor or format problem, yt-dlp is too old for the site.

**Fix:**

```bash
brew install yt-dlp        # if missing
brew upgrade yt-dlp        # if outdated
```

PulseCut looks in `/opt/homebrew/bin` and `/usr/local/bin` first, then `PATH`, each time it resolves a source.

## Build and binaries

### `PulseCut` and `pulsecut` binaries overwrite each other

**Cause:** macOS's default file system is case-insensitive. The GUI binary is named `PulseCut`, and the command-line tool was previously named `pulsecut`. Both were written to the same path, so each build replaced the other.

**Fix:** The command-line tool is now named `pulsecut-cli`, so the names no longer clash. Rebuild, then use the new name:

```bash
cargo build --release -p pulsecut-app -p pulsecut-cli
target/release/pulsecut-cli --help
```

If you have an older `pulsecut` binary in `target/release/` or in an app bundle, delete it and rebuild.

## Publishing to Instagram

### Instagram upload fails: media URL not reachable

**Cause:** Instagram downloads the video from a public URL. It cannot read files on your computer or a `localhost` address. PulseCut copies each clip into `[publish] media_host_dir` and gives Instagram the URL `instagram_public_base_url` followed by the file name. PulseCut refuses the upload if the base URL does not start with `https://`.

**Fix:**

1. Serve `media_host_dir` from a web server, bucket or tunnel that has a public HTTPS address.
2. Set `instagram_public_base_url` to that address (for example `https://media.example.com/pulsecut/`).
3. Open the address of a clip file in a browser on a different network (for example a phone on mobile data) to confirm it loads.

PulseCut removes the copy from the hosted folder after a successful publish. See [USER_GUIDE.md](USER_GUIDE.md#5-publishing) for the Instagram requirements.

## Still stuck

Run the diagnostics from a terminal:

```bash
target/release/pulsecut-cli obs-check    # OBS connection and replay buffer
target/release/pulsecut-cli encoders     # hardware and software encoders
```

The messages quoted on this page also appear in the app's activity log.
