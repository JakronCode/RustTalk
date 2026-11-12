# RustTalk

Lightweight voice chat in Rust — a Discord-style client with a TeamSpeak-style server.
**Local first:** no online accounts, no cloud. Client: Windows (egui), server: Linux (Debian, systemd).

![Status](https://img.shields.io/badge/status-MVP-green)

---

## Features

### Working

- **Echo cancellation** (WebRTC AEC3, pure Rust): speakers instead of headphones work
- **Encryption:** control connection over TLS (the server's own certificate, pinned by the client on first connect like SSH; a changed certificate is refused with a warning), voice and screen audio encrypted with ChaCha20-Poly1305 (per-session key)
- **Voice:** push-to-talk (optionally **global**, also while RustTalk is in the background; keys or side mouse buttons) + voice activity (VAD using RMS *and* RNNoise speech probability), Opus 64 kbit/s 48 kHz/20 ms over UDP with in-band FEC, jitter buffer with FEC/PLC, sinc resampler for any microphone sample rate; click "Voice Connected" for connection stats (ping graph of the last 4 minutes, jitter, inbound/outbound packet loss)
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
- **Client UI:** dark mode with a **custom primary color** (presets or picker), **cozy / compact density**, procedural logo, saved servers, settings in a separate window, mic level meter, avatars with talking ring, user panel with mic and headset device / volume dropdowns, per-user local mute and **per-user volume** 0–200 % (right click, remembered by name), **input volume**, UI scaling
- **Auto-reconnect:** on connection loss the client retries with backoff (1 s … 30 s) and rejoins the last voice channel with the same mute state
- **Video (H.264 30 fps over TCP, OpenH264):** webcam with device selection + live preview, share a screen or a single window via a picker ("Applications" / "Entire Screen") **with computer audio** (WASAPI loopback, your RustTalk voices are filtered out); camera and screen at the same time. **Watching is opt-in:** other people's streams show as a "Watch Stream" / "Watch Camera" card, and the server only sends video to those who clicked it ("Stop watching" in the tile's corner) - no forced bandwidth. Click a tile for fullscreen with a member bar (green dot while talking). Quality levels "Best" (1440p screen / 1080p camera, up to ~10 Mbit/s), "High" (1080p / 720p, ~6 Mbit/s) and "Balanced" (720p / 540p, ~2.5 Mbit/s); monitors are captured with DXGI Desktop Duplication, pictures scaled with SIMD, OpenH264 with its assembly (~25 ms per 1440p frame from capture to H.264 instead of ~180 ms); viewers who join late or lose frames ask the sender for a keyframe
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
| Network | `tokio`; TCP (control, length-prefixed `bincode`) + UDP (audio + ping, custom 28-byte header) |
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

- **TCP:** login, channels, chat (text + images), events, video frames (camera/screen), channel order, server settings, chat edits/replies, GIF search, roles/moderation/invites (protocol v15, TLS)
- **UDP:** Opus audio plus small ping/pong control packets (20 ms frames, 28-byte header with token, sequence, counter; payload ChaCha20-Poly1305); the server opens each packet with the sender's key and seals it again per receiver, and drops replayed packets. Pongs carry two per-session packet counters (kept in RAM only) for the connection stats
- **Lobby:** everyone lands here when connecting; talking is blocked on the server; join log

Details (German): [Konzept.md](Konzept.md) · test log: [test.md](test.md) · roadmap: [ROADMAP.md](ROADMAP.md)

---

## Setup

### Development quick start (Linux/macOS)

```bash
make            # list all targets
make setup      # check build tools, fetch crates
make setup-server && make server   # local dev server on 127.0.0.1:7331, data in .dev/server
make client     # client with its own config in .dev/client
make test       # all tests, serial like CI
```

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

### Client (Linux) – minimal build

Build and run from the repository root:

```sh
cargo build --release --locked -p rusttalk-client
./target/release/rusttalk
```

Use a release build for live video: debug builds leave CPU-heavy frame resizing
and pixel conversion unoptimized.

Enter the server address and username, connect, then join a voice channel
(the lobby has no voice). Chat, microphone/speaker audio, voice activation,
and push-to-talk while the window is focused are available. Webcam preview
and sharing use V4L2. The Screen button opens the native desktop portal picker;
the selected monitor or window is captured through PipeWire. Camera and screen
can be shared together. Background push-to-talk uses the Wayland GlobalShortcuts
portal; enable it in Settings > Keybinds. Desktop notifications include a channel
open action. Screen audio is separate work and is not included here.

Settings > Keybinds also provides global **Toggle mute** and **Toggle deafen**
shortcuts on Windows and Wayland. Both start unassigned. Click a selector and
press a key or side mouse button, optionally holding Ctrl, Alt, or Shift (for
example, Ctrl+F8); Clear removes it. Modifier combinations are matched exactly. Each press toggles once, and
shortcuts are suspended while recording a key or disconnected. Deafen keeps the
microphone blocked until undeafened and restores the previous mute state.

F8 is the default push-to-talk key. The selector in Voice & Audio and Keybinds
sets the same saved binding. Selecting a key on Linux enables background PTT.
On Hyprland with Lua configuration, RustTalk assigns that key through a runtime
binding and removes it when disconnected or closed. It restores its binding after
a desktop configuration reload and refuses keys already assigned to other desktop
actions. No fixed RustTalk keybinding is needed in the Hyprland config.

Other Wayland desktops use the portal's shortcut confirmation UI and may choose a
different trigger. Settings shows the desktop-reported binding or any portal error.
A GlobalShortcuts portal backend is required; X11 global polling is not supported.

Release AppImages support Settings > Updates > Install update & restart. The
updater chooses the matching architecture, requires SHA-256 verification, then
atomically replaces the writable AppImage file. Settings and voice models stay
outside the image. Source builds remain manual updates.

Spoken join/leave names are available via Settings > Notifications > Sounds.
Download the natural voices once (~410 MB), enable "Say names", and choose a
voice and English or German. Synthesis then runs locally, offline; no system
speech service is required.

Settings are saved to `$XDG_CONFIG_HOME/RustTalk/client_config.toml`, or
`~/.config/RustTalk/client_config.toml` by default. Native build requirements
include a C/C++ compiler, CMake, pkg-config, ALSA and Opus development libraries,
the display libraries used by eframe, and GStreamer core/app/video development
libraries. Capture needs the GStreamer PipeWire, V4L2, video conversion and camera
decoder plugins, plus a running `xdg-desktop-portal` backend for the compositor.
On Arch, install `gstreamer`, `gst-plugins-base`, `gst-plugins-good`,
`gst-plugin-pipewire`, and the appropriate portal backend. Tested with GStreamer
1.28 and PipeWire 1.6. Older PipeWire sources use portal session closure detection.
On Arch, headers come with the library packages. The Windows client remains in `Client/Windows`; its shared
UI and networking code are also used by Linux.

### Client (Windows) – build from source

Requirements: Rust (MSVC toolchain incl. C++ build tools), **CMake** (for Opus; the one in Visual Studio / Build Tools is enough, `cargo.ps1` finds it) and **NASM** for OpenH264's SIMD assembly (`winget install NASM.NASM`; without it video encoding still works but is ~5x slower - `cargo.ps1` finds a winget install by itself). Builds started from a cargo `target` folder (and debug builds) run **portable** – no install screen, settings in the working directory.

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

The [build workflow](.github/workflows/build.yml) tests and builds both client and
server on native Windows x64, Windows ARM64, Linux x64, and Linux ARM64 runners.
Pull requests and main/master pushes run the same build used by releases.
Integration tests run serially because their server fixtures share TLS files.
The [release workflow](.github/workflows/release.yml) verifies package versions
and publishes only after the entire matrix succeeds.

Linux clients are `rusttalk-linux-x86_64.AppImage` and
`rusttalk-linux-aarch64.AppImage`, with checksums. Built on Ubuntu 24.04, they bundle
capture plugins and libraries, but require a compatible host (glibc 2.39 or newer),
Wayland, PipeWire, a desktop portal backend, and host GPU drivers. Packaging runs
`--check-runtime` inside the image to verify the capture elements load, then tests
bad-checksum rejection, self-replacement, and relaunch against a local release server.

Windows clients are `rusttalk-windows.exe` (x64; existing updater name retained)
and `rusttalk-windows-arm64.exe`. Bare servers use
`rusttalk-server-linux-{x86_64,aarch64}`, `rusttalk-server-windows.exe`, and
`rusttalk-server-windows-arm64.exe`, each with `.sha256`. Linux server tarballs
also include configuration and deployment scripts. `rusttalk-server update`
selects the matching OS and architecture.

To package a locally built Linux release, run `bash scripts/package-linux.sh`.
Artifacts go into `dist/`. Packaging tools use versioned releases and verified
GitHub asset digests; the AppImage runtime is pinned too. Images built on newer
local distributions inherit that distribution's minimum glibc requirement.

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
├── shared/            # protocol (v15), framing, secure (TLS pinning + packet encryption), UDP packet format
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
