# RustTalk

Lightweight voice chat in Rust — a Discord-style client with a TeamSpeak-style server.
**Local first:** no online accounts, no cloud. Client: Windows (egui), server: Linux (Debian, systemd).

![Status](https://img.shields.io/badge/status-MVP-green)

---

## Features

### Working

- **Voice:** push-to-talk + voice activity (VAD using RMS *and* RNNoise speech probability), Opus 64 kbit/s 48 kHz/20 ms over UDP with in-band FEC, jitter buffer with FEC/PLC, sinc resampler for any microphone sample rate
- **Automatic gain control** (AGC + limiter): quiet microphones are boosted, loud ones tamed — can be turned off
- **AI noise suppression** (RNNoise via `nnnoiseless`) — can be turned off
- **Channel types:** Lobby (exactly one, join log, no talking/video), Category (grouping), Voice (voice + media chat), Text (chat); order via **drag & drop** (stored on the server)
- **Discord-style UI:** server menu, voice panel, user panel, settings window with categories, channel settings (partly WIP)
- **Text chat** with history, images (PNG/JPEG/GIF), unread markers
- **SQLite persistence** (server): channels (incl. order), chat history (retention 10k per channel) and the lobby log survive restarts
- **Client UI:** dark mode, procedural logo, saved servers, settings in a separate window, mic level meter, avatars with talking ring, per-user local mute (right click), UI scaling
- **Video (MJPEG ~25 fps over TCP):** webcam with device selection + live preview, share a screen or a single window via a picker ("Applications" / "Entire Screen"); camera and screen at the same time. Click a tile for fullscreen with a member bar (green dot while talking). Quality levels "High" (1080p) and "Balanced" (720p)
- **Server:** single binary + `server.toml`, systemd service, one port (7331 TCP+UDP), optional password, keepalive/timeouts for dead connections

### Broken — fix in progress (WIP)

- ⛔ **GIPHY search:** the integration exists (`media.rs`, API key in Settings > Advanced) but does not work yet — the button is disabled.

---

## Tech stack

| Area | Choice |
|---|---|
| Language | Rust (edition 2021), Cargo workspace |
| GUI | `eframe`/`egui` 0.31 + `egui-phosphor` (icons), custom theme |
| Audio | `cpal` (I/O), `audiopus` (Opus), `nnnoiseless` (RNNoise), `ringbuf` (lock-free) |
| Network | `tokio`; TCP (control, length-prefixed `bincode`) + UDP (audio, custom 20-byte header) |
| Server DB | `rusqlite` (bundled SQLite, WAL) |
| Media | `image`, `rfd` (file dialog), `nokhwa` (camera, Media Foundation), `xcap` (screens/windows); WIP: `ureq` + `serde_json` (GIPHY) |
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

- **TCP:** login, channels, chat (text + images), events, video frames (camera/screen), channel order (protocol v6)
- **UDP:** Opus audio only (20 ms frames, token + sequence header); the server relays 1:1 within the voice channel
- **Lobby:** everyone lands here when connecting; talking is blocked on the server; join log

Details (German): [Konzept.md](Konzept.md) · test log: [test.md](test.md)

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

Prebuilt binaries are attached to the [GitHub releases](https://github.com/JakronCode/RustTalk/releases).

### Client (Windows)

Requirements: Rust (MSVC toolchain incl. C++ build tools).

> ⚠️ **WDAC note (this development machine):** WDAC blocks cargo build scripts under `Documents` (os error 4551). Therefore **always build/test via the wrapper**, which mirrors the workspace to `%LOCALAPPDATA%\Programs\RustTalk-src`:

```powershell
# build
powershell -ExecutionPolicy Bypass -File Client\Windows\Test_Scripts\cargo.ps1 -Retries 12 build --release -p rusttalk-client

# run
& "$env:LOCALAPPDATA\Programs\RustTalk-src\target\release\rusttalk.exe"
```

On machines without such policies a plain `cargo build --release -p rusttalk-client` is enough (the exe is then in `target\release\`).

On start: enter the server address (e.g. `10.69.40.101`) and a username, then connect. "Save server" adds it to your saved servers.

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
├── shared/            # protocol (v6), framing, UDP packet format
├── Server/            # Linux server (+ test_client CLI, integration tests, deploy/, Test_Scripts/)
│   └── src/           #   main, config, state (RAM), db (SQLite), tcp, udp
└── Client/Windows/    # egui client
    ├── src/           #   net, audio/ (capture, encoder + RNNoise, dsp, playback, jitter, resample),
    │                  #   media (images/GIPHY), video (camera/screen/window), config, logo,
    │                  #   app/ (mod, theme, connect, sidebar, views, settings, channel_settings, media_ui)
    └── testdata/      #   speech sample for the FEC test
```

## Known limitations

- No encryption/authentication apart from the optional server password (meant for LAN use)
- Push-to-talk only works while the window is focused (global hotkey: planned)
- No echo cancellation → headphones recommended
- GIPHY: **broken, fix in progress**
- Video is MJPEG over TCP: "High" needs ~5–6 MB/s per screen stream and viewer — meant for LAN (pick "Balanced" on Wi-Fi). On very large monitors (3440×1440) "High" reaches ~19 fps
