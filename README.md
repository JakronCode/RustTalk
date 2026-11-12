# RustTalk

Lightweight voice chat in Rust — a Discord-style client with a TeamSpeak-style server.
**Local first:** no online accounts, no cloud. Client: Windows (egui), server: Linux (Debian, systemd).

![Status](https://img.shields.io/badge/status-MVP-green)

---

## Features

### Working

- **Echo cancellation** (WebRTC AEC3, pure Rust): speakers instead of headphones work
- **Encryption:** control connection over TLS (the server's own certificate, pinned by the client on first connect like SSH; a changed certificate is refused with a warning), voice and screen audio encrypted with ChaCha20-Poly1305 (per-session key)
- **Voice:** push-to-talk (optionally **global**, also while RustTalk is in the background; keys or side mouse buttons) + voice activity (VAD using RMS *and* RNNoise speech probability), Opus 64 kbit/s 48 kHz/20 ms over UDP with in-band FEC, jitter buffer with FEC/PLC, sinc resampler for any microphone sample rate
- **Automatic gain control** (AGC + limiter): quiet microphones are boosted, loud ones tamed — can be turned off
- **AI noise suppression** (RNNoise via `nnnoiseless`) — can be turned off
- **Channel types:** Lobby (exactly one, join log, no talking/video), Category (grouping), Voice (voice + media chat), Text (chat); order via **drag & drop** (stored on the server)
- **Discord-style UI:** server menu, voice panel, user panel, settings window with categories, channel settings (partly WIP)
- **Text chat** with history, images (PNG/JPEG/GIF, **animated**), unread markers, **replies**, **edit/delete** own messages, **@mentions** (highlight, sound, autocomplete), **search** per channel
- **GIF picker (GIPHY), Discord style:** trending on open, search while typing, two-column grid, click = send. One GIPHY key per **server** (Server Settings > Integrations) – members enter nothing; the server searches and caches, the key never leaves it
- **Name announcements:** "Alex joined" after the join/leave sound - Windows voices or 10 natural offline voices (Supertonic 3 via ONNX Runtime, English/German). The installed app downloads the natural voices once in the background (~410 MB from Hugging Face, SHA-256 checked) and switches to one in the Windows language; can be turned off
- **Notifications:** Windows pop-ups (toasts) for new messages while RustTalk is in the background (click opens the channel, text can be hidden), sounds for join/leave/messages/mute, taskbar flash
- **Installer + updater:** the downloaded exe installs itself per user; in-app updates from the GitHub releases
- **Roles & moderation (TeamSpeak-style identities):** every client has a secret identity (the server only stores its hash); roles Member / Moderator / Admin hang on it, not on the name. Right-click a user: move, kick, ban (identity + IP, with duration and reason), set role. Server Settings: roles, bans, **invites** (`rusttalk://host:port/CODE`, limited uses/expiry, replace the server password), **audit log**. Without any admin the server stays open (everyone may do everything); the first admin is made via "Become admin" or `admin_password` in `server.toml`
- **SQLite persistence** (server): channels (incl. order), chat history (retention 10k per channel) and the lobby log survive restarts
- **Client UI:** dark mode, procedural logo, saved servers, settings in a separate window, mic level meter, avatars with talking ring, per-user local mute and **per-user volume** 0–200 % (right click, remembered by name), **input volume**, UI scaling
- **Auto-reconnect:** on connection loss the client retries with backoff (1 s … 30 s) and rejoins the last voice channel with the same mute state
- **Video (H.264 ~25 fps over TCP, OpenH264):** webcam with device selection + live preview, share a screen or a single window via a picker ("Applications" / "Entire Screen") **with computer audio** (WASAPI loopback, your RustTalk voices are filtered out); camera and screen at the same time. Click a tile for fullscreen with a member bar (green dot while talking). Quality levels "High" (1080p, ~4 Mbit/s) and "Balanced" (720p, ~2 Mbit/s); viewers who join late or lose frames ask the sender for a keyframe
- **Server settings** (in the app): name, icon, welcome message and rules in the lobby, **join/leave messages** in an announcement text channel, AFK notice template, message of the day, **auto AFK mover** (muted users are moved after N minutes, default 10) — roles/bans/invites as WIP
- **Report a Bug** button that opens a pre-filled GitHub issue
- **Server:** single binary + `server.toml`, systemd service, one port (7331 TCP+UDP), optional password, keepalive/timeouts for dead connections

### Not yet (WIP)

- Toggle-mute/deafen hotkeys and more — see [ROADMAP.md](ROADMAP.md)

---

## Tech stack

| Area | Choice |
|---|---|
| Language | Rust (edition 2021), Cargo workspace |
| GUI | `eframe`/`egui` 0.31 + `egui-phosphor` (icons), custom theme |
| Audio | `cpal` (I/O), `audiopus` (Opus), `nnnoiseless` (RNNoise), `ringbuf` (lock-free) |
| Network | `tokio`; TCP (control, length-prefixed `bincode`) + UDP (audio, custom 20-byte header) |
| Server DB | `rusqlite` (bundled SQLite, WAL) |
| Media | `image`, `rfd` (file dialog), `nokhwa` (camera, Media Foundation), `xcap` (screens/windows), `ureq` (GIF downloads) |
| Server GIF search | `ureq` + `serde_json` (GIPHY API, cached) |
| Hotkeys | `device_query` (global push-to-talk by polling, no keyboard hook) |
| Server ops | systemd unit (hardened: `ProtectSystem=strict` + `ReadWritePaths`) |

## Architecture (short)

```
Client (Windows)                          Server (Debian)
┌──────────────┐   TCP (control, bincode)  ┌──────────────┐
│ egui UI      │ ◄───────────────────────► │ TCP listener │
│ cpal capture │ ── Opus/UDP (audio) ────► │ UDP relay    │
│ cpal play    │ ◄───────────────────────  │ State (RAM)  │
└──────────────┘                           │ SQLite (DB)  │
                                            └──────────────┘
```

- **TCP:** login, channels, chat (text + images), events, video frames (camera/screen), channel order, server settings, chat edits/replies, GIF search, roles/moderation/invites (protocol v12, TLS)
- **UDP:** Opus audio only (20 ms frames, 28-byte header with token, sequence, counter; payload ChaCha20-Poly1305); the server opens each packet with the sender's key and seals it again per receiver
- **Lobby:** everyone lands here when connecting; talking is blocked on the server; join log

Details (German): [Konzept.md](Konzept.md) · test log: [test.md](test.md) · roadmap: [ROADMAP.md](ROADMAP.md)

---

## Setup

### Server (Debian/Linux)

Requirements: Rust (1.85+), systemd.

```bash
# build
cargo build --release -p rusttalk-server

# install + start (as root, from the repo root)
sudo bash Server/deploy/install_service.sh
```

The script installs to `/opt/rusttalk/server/` (binary + `server.toml`), creates the user `rusttalk` and enables the systemd service `rusttalk-server`. Configuration: [Server/server.toml](Server/server.toml) (bind address, password, `persist_state`, `db_path`, initial channels).

With persistence enabled, the channels in `server.toml` are only used on the very first start (empty database); after that the database is the source of truth.

Open port **7331** (TCP+UDP) in the firewall if nftables/ufw is active.

Prebuilt binaries are attached to the [GitHub releases](https://github.com/JakronCode/RustTalk/releases) (`rusttalk-server-linux-x86_64.tar.gz`).

**Updates:** the server looks for a newer release on start and once a day (`check_updates` in `server.toml`, on by default) and shows admins "Server update available" in the client. It never installs anything by itself - the service user cannot write its own program. To update, run on the server:

```bash
sudo /opt/rusttalk/server/bin/rusttalk-server update            # download, SHA-256 check, test-start, swap, restart the service
/opt/rusttalk/server/bin/rusttalk-server update --check         # only look
```

The previous version stays next to it as `rusttalk-server.old` (to go back: `sudo mv rusttalk-server.old rusttalk-server && sudo systemctl restart rusttalk-server`). `--no-restart` skips the restart; `rusttalk-server --version` prints the version. Works the same for a Windows server (`rusttalk-server.exe update`, restart it yourself).

### Client (Windows) – install

Download `rusttalk-windows.exe` from the [latest release](https://github.com/JakronCode/RustTalk/releases/latest) and run it. It installs itself for your user (no admin rights): `%LOCALAPPDATA%\Programs\RustTalk`, Start menu entry (optional desktop shortcut), settings in `%APPDATA%\RustTalk`. Uninstall via Windows **Apps & features**.

**Updates:** the installed app checks GitHub for a newer release on start (Settings > About > Updates) and installs it on one click (download, SHA-256 check, restart).

Command line (for admins/scripts): `rusttalk-windows.exe --install [--desktop-shortcut] [--no-start]`, `rusttalk.exe --uninstall [--quiet] [--remove-settings]`, `rusttalk.exe --update`.

### Client (Windows) – build from source

Requirements: Rust (MSVC toolchain incl. C++ build tools). Builds started from a cargo `target` folder (and debug builds) run **portable** – no install screen, settings in the working directory.

> ⚠️ **WDAC note (this development machine):** WDAC blocks cargo build scripts under `Documents` (os error 4551). Therefore **always build/test via the wrapper**, which mirrors the workspace to `%LOCALAPPDATA%\Programs\RustTalk-src`:

```powershell
# build
powershell -ExecutionPolicy Bypass -File Client\Windows\Test_Scripts\cargo.ps1 -Retries 12 build --release -p rusttalk-client

# run
& "$env:LOCALAPPDATA\Programs\RustTalk-src\target\release\rusttalk.exe"
```

On machines without such policies a plain `cargo build --release -p rusttalk-client` is enough (the exe is then in `target\release\`).

On start: enter the server address (e.g. `10.69.40.101`) and a username, then connect. "Save server" adds it to your saved servers.

### Releasing

Bump the version in `Client/Windows/Cargo.toml`, `Server/Cargo.toml` and `shared/Cargo.toml`, then push a tag:

```bash
git tag v0.4.0 && git push origin v0.4.0
```

The [Release workflow](.github/workflows/release.yml) checks that the tag matches the versions and attaches `rusttalk-windows.exe` (+ `.sha256`, needed by the updater), the Linux server package and the bare server binaries `rusttalk-server-linux-x86_64` / `rusttalk-server-windows.exe` (+ `.sha256`, for `rusttalk-server update`) to the GitHub release.

### Tests

```powershell
# Windows (everything)
powershell -ExecutionPolicy Bypass -File Client\Windows\Test_Scripts\cargo.ps1 -Retries 12 test --workspace
# Windows: E2E against the live server
powershell -ExecutionPolicy Bypass -File Client\Windows\Test_Scripts\e2e_server.ps1
```

```bash
# Linux: server tests + local E2E smoke test
bash Server/Test_Scripts/run_tests.sh
bash Server/Test_Scripts/e2e_local.sh
```

## Project structure

```
├── shared/            # protocol (v12), framing, secure (TLS pinning + packet encryption), UDP packet format
├── Server/            # Linux server (+ test_client CLI, integration tests, deploy/, Test_Scripts/)
│   └── src/           #   main, config, state (RAM), db (SQLite), tcp, udp
└── Client/Windows/    # egui client
    ├── src/           #   net, audio/ (capture, encoder + RNNoise, dsp, playback, jitter, resample),
    │                  #   media (images/GIPHY), video (camera/screen/window), config, logo,
    │                  #   app/ (mod, theme, connect, sidebar, views, settings, server_settings, channel_settings, media_ui)
    └── testdata/      #   speech sample for the FEC test
```

## Third-party licenses

- **Natural voices:** the Supertonic 3 model is by Supertone Inc. and licensed under the [BigScience OpenRAIL-M license](https://huggingface.co/Supertone/supertonic-3/blob/main/LICENSE). RustTalk does not ship it: the app downloads it straight from Hugging Face (pinned revision) and puts the `LICENSE` next to it in `%APPDATA%\RustTalk\voices\supertonic-3`. Whoever uses the voices agrees to the license's use restrictions (Attachment A) - among others no unlawful use, no impersonating people without consent, no harassment or defamation, no harming minors. RustTalk only uses them to read out user names.
- **ONNX Runtime** (MIT, Microsoft) is linked statically via the `ort` crate (MIT/Apache-2.0).

## Known limitations

- The server certificate is self-signed: compare the fingerprint in the server log (`TLS-Fingerprint: ...`) with Settings > Advanced on first connect if you want to rule out a man in the middle
- GIFs need one free GIPHY API key per server (developers.giphy.com), entered under Server Settings > Integrations
- Video runs over the TCP control connection (H.264): fine in a LAN, on weak Wi-Fi pick "Balanced"
- Natural voices use `ort` 2.0 release candidate `=2.0.0-rc.15` (no stable 2.0 exists yet; pinned exactly so builds don't change on their own). Switch to 2.0.0 once it is out
