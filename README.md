# RustTalk

Fresh client configurations include `public.rusttalk.com:7331` as **rusttalk official server** in saved servers. Joining is manual and uses the same certificate-trust prompt as any other server. Existing saved-server lists, including empty lists, are preserved.


Lightweight voice chat in Rust — a Discord-style client with a TeamSpeak-style server.
**Local first:** no online accounts, no cloud. Client: Windows (egui), server: Linux (Debian, systemd).
Website: [rusttalk.com](https://rusttalk.com) (source in [website/](website/))

![Status](https://img.shields.io/badge/status-MVP-green)

---

## Features

### Working

- **Echo cancellation** (WebRTC AEC3, pure Rust): speakers instead of headphones work
- **Encryption:** control connection over TLS (the server's own certificate: on first connect the client shows its fingerprint and asks before sending anything, then pins it like SSH; a changed certificate is refused with a warning), voice and screen audio encrypted with ChaCha20-Poly1305 (per-session key)
- **Voice:** push-to-talk (optionally **global**, also while RustTalk is in the background; keys or side mouse buttons) + voice activity (VAD using RMS *and* RNNoise speech probability), Opus 64 kbit/s 48 kHz/20 ms over UDP with in-band FEC, jitter buffer with FEC/PLC, sinc resampler for any microphone sample rate; click "Voice Connected" for connection stats (ping graph of the last 4 minutes, jitter, inbound/outbound packet loss)
- **Automatic gain control** (AGC + limiter): quiet microphones are boosted, loud ones tamed — can be turned off
- **AI noise suppression** (RNNoise via `nnnoiseless`) — can be turned off
- **Microphone privacy:** connecting to the lobby does not open the microphone. Capture starts only after the server confirms a voice-channel join and is released on leaving voice or disconnecting. Opening Settings → Voice & Audio intentionally opens a local level preview outside voice; closing settings or changing pages releases it. Notification sounds remain available in the lobby through output-only playback.
- **Channel types:** Lobby (exactly one, join log, no talking/video), Category (grouping), Voice (voice + media chat), Text (chat); order via **drag & drop** (stored on the server)
- **Discord-style UI:** server menu, voice panel, user panel, settings window with categories, channel settings (partly WIP)
- **Chat markdown:** headers, emphasis, inline code, quotes, lists, and fenced code blocks with offline syntax highlighting and copy buttons. Shift+Enter inserts a newline; Enter sends. HTML, markdown images and `[text](url)` links stay inert text; only bare web addresses become clickable (see Links).
- **Links in chat:** bare `http://` and `https://` addresses outside code are clickable; nothing is fetched, previewed or downloaded until you open one. Unless a warning-free HTTPS origin is already trusted, a click shows the site (international names also in their real `xn--` form) and the full canonical address, with **Cancel** as the default; **Open in browser** hands it to your system browser (no shell involved). Trusted warning-free HTTPS links open directly. Links with credentials (`user@host`), backslashes, control or invisible/direction-changing characters, other schemes (`javascript:`, `file:`, ...) or over 2,048 bytes stay plain text. Warnings, never skipped by trust: likely downloads of programs, scripts, installers, disk images or archives (also through percent-encoding, odd case, trailing dots or query values such as `?file=setup.exe`), plain http, bare IP addresses, this computer or the local network (`localhost`, private and link-local ranges, `.local`, single-label names), international names, and entries of your threat feed (which also needs an extra confirmation). "Always open links to ..." trusts one exact origin (protocol, name and port; `https://example.com` does not cover `http://` or `sub.example.com`), is offered only for clean https links, is stored locally (at most 200 sites, 90 days each) and can be removed in Settings > Advanced > Links
- **Optional threat feed (local, opt-in):** Settings > Advanced > Links can import a list of known malicious URLs or hosts that you downloaded yourself, e.g. the [URLhaus](https://urlhaus.abuse.ch/api/) plain-text URL list or hostfile (free abuse.ch account and Auth-Key; not-for-profit use is free, commercial use may need a Spamhaus subscription, see the [abuse.ch terms](https://abuse.ch/terms-of-use/)). Plain URL lists, hosts files and URLhaus CSV are read, up to 32 MiB / 250,000 entries. RustTalk reads the file from disk on start or **Reload**; it never downloads the feed, never sends chat links to anyone, and ships no feed or provider key. The dialog shows the feed name, size and age (out of date after 7 days, by file date) and says when no feed is loaded or it failed. A configured feed that fails to load forces confirmation even for trusted sites and cannot be remembered away. Matching is exact URL (http and https alike; fragment, trailing host dot and harmless percent-encoding differences ignored) or exact host (subdomains are not matched). Not being listed never means a link is safe
- **Text chat** with history, images (PNG/JPEG/GIF, **animated**), unread markers, **replies**, **edit/delete** own messages, **@mentions** (highlight, sound), **search** per channel
- **@mention popup:** typing `@` or `@na` at the cursor opens a member list above the composer (avatar and name, scrolls past six rows). Arrow keys move the highlight, Tab or Enter completes the name (Enter does not send), a click picks a row, Esc closes the list. Only the `@query` before the cursor is replaced, text after it stays; names with spaces and non-Latin letters work, and the 2,000-character limit is respected. Suggestions are the members already shown in the sidebar (no extra data or lookups); there are no `@game`/`@time` commands or presence states
- **Emoji:** Discord-style picker beside the composer (search, categories, ~1,350 emoji; click inserts at the cursor or over the selection, Shift+click picks several). Shortcodes such as `:rofl:`, `:clown:` and `:thumbsup:` become emoji when a message is sent or an edit is saved; unknown shortcodes and anything in inline code or code blocks stay as typed. Emoji are plain Unicode text (no protocol change) drawn with a bundled monochrome Noto Emoji font, so they look the same on Windows and Linux without system fonts or downloads. Not supported: skin-tone variants, flags and other multi-code-point emoji in the picker (received ones and matching typed shortcodes show as their parts), color emoji, custom emoji and stickers
- **GIF picker (GIPHY), Discord style:** trending on open, search while typing, two-column grid, click = send. One GIPHY key per **server** (Server Settings > Integrations) – members enter nothing; the server searches and caches, the key never leaves it
- **Name announcements:** "Alex joined" after the join/leave sound - Windows voices or 10 natural offline voices (Supertonic 3 via ONNX Runtime, English/German). The installed app downloads the natural voices once in the background (~410 MB from Hugging Face, SHA-256 checked) and switches to one in the Windows language; can be turned off
- **Notifications:** Windows pop-ups (toasts) for new messages while RustTalk is in the background (click opens the channel, text can be hidden), sounds for join/leave/messages/mute, taskbar flash on Windows (disabled on Linux to prevent focus stealing)
- **Installer + updater:** the downloaded exe installs itself per user; in-app updates from the GitHub releases
- **Roles & moderation (TeamSpeak-style identities):** every client has a secret identity (the server only stores its hash); roles Member / Moderator / Admin hang on it, not on the name. Right-click a user: move, kick, ban (identity + IP, with duration and reason), set role. Server Settings: roles, bans, **invites** (`rusttalk://host:port/CODE`, limited uses/expiry, replace the server password), **audit log**. Without any admin the server stays open (everyone may do everything); the first admin is made via "Become admin" or `admin_password` in `server.toml`
- **SQLite persistence** (server): channels (incl. order), chat history (retention 10k per channel) and the lobby log survive restarts
- **Client UI:** dark mode with a **custom primary color** (presets or picker), **cozy / compact density**, procedural logo, saved servers, settings in a separate window, mic level meter, avatars with talking ring, user panel with mic and headset device / volume dropdowns, per-user local mute and **per-user volume** 0–200 % for voice and shared audio separately (right click, remembered by name), a **volume mixer** that slides in next to the voice chat (icon in its top right corner: one row per incoming stream with level meter and mute, plus output and mic), **input volume**, UI scaling
- **Auto-reconnect:** on connection loss the client retries with backoff (1 s … 30 s) for chat only. Joining voice again requires an explicit action; microphone, camera and sharing never resume automatically. A wrong password, ban, version mismatch or certificate question stops the retries
- **Several servers at once:** every server you connect to stays connected for lobby chat, unread markers and notifications, each with its own connection, certificate, chat and errors; one failing server does not affect the others. Voice runs on one server at a time: viewing another server keeps voice where it is, and joining voice on a different server first leaves the old voice channel (camera, screen and audio sharing stop and do not restart). Hardware mute and deafen stay global; their state is sent only to the voice server, not to background chat communities. Connecting to a saved server asks "Reconnect on start" or "Just now"; only a remembered server is connected again when RustTalk starts (lobby and chat only, never microphone, camera or sharing), until you disconnect it, and a disconnect is remembered across restarts. Saving a server, a bundled address or a successful login alone never connects automatically, and a server whose certificate is not trusted yet always asks first. At most 8 servers are open at once and at most 2 automatic connection attempts run at a time; servers over the limit are shown as not connected instead of being dropped. The server switcher is the server-name dropdown in the sidebar: saved and open servers with a Connected or Disconnected status, a plain "New messages" label, a voice icon and a connect/disconnect icon each, above the usual server and app actions. A disconnected server sends no messages or notifications
- **Multi-server resource limits:** each connection has bounded incoming and outgoing queues with asynchronous backpressure, cancellation and write deadlines. Each server retains at most 2,000 chat lines per channel, 8 MiB of chat text accounting and 32 MiB of image payload, with caps on users, channels and admin data. These are payload limits, not a total process memory guarantee; decoded images and media need additional memory. Desktop multi-server support ships first; mobile integration is separate.
- **Audio sharing (Windows and Linux):** join a voice channel and click **Audio** (music note) beside Camera and Screen. All three controls become icons when the sidebar is narrow. Choose **System-wide audio** or one or more applications playing audio (up to 8, mixed into one stream), then click **Share audio**. While sharing, the Audio button reopens the dialog: **Stop sharing**, or tick and untick apps and click **Apply** to change the mix. Everyone in the channel sees a music note on the sharer (member list and stage tile), lit while sound is coming through. A tiny X on the active Screen and Audio buttons (on hover) stops that share at once; fullscreen keeps its stop control visible. Discovery does not record audio. Microphone mute and push to talk remain independent. Leaving or switching voice channels, disconnecting, changing devices or capture failure stops sharing; starting again requires an explicit action. A closed or stale application selection never falls back to system sound.
  - **Linux:** requires `libpulse` and PulseAudio or `pipewire-pulse`. Applications are listed by playback stream (an app can expose several streams); the selected stream alone is monitored on its actual output. System-wide capture uses the default output monitor, independently of RustTalk's playback-device choice, including when sharing one window. Streams use server-enforced `DONT_MOVE` and verified monitor ownership. **Include system audio with screen** starts whole-output capture only after video produces a valid frame. Restart after changing the system default output.
  - **Windows:** system-wide capture uses WASAPI loopback on the selected output. Application capture uses Windows process-tree loopback, available on Windows 10 build 20348 and newer, and captures that process plus its children across outputs. Protected content or apps without active audio sessions may be unavailable. Process identity is rechecked before capture; exiting the selected process stops sharing.
  - Audio reuses encrypted Opus transport and per-user mute; receivers set its volume apart from the voice. The existing echo canceller reduces RustTalk playback during system sharing but cannot guarantee complete removal.
- **Video (H.264 30 fps over TCP, OpenH264):** webcam with device selection + live preview, share a screen or a single window via a picker ("Applications" / "Entire Screen") **with computer audio** (WASAPI loopback, your RustTalk voices are filtered out); camera and screen at the same time. **Watching is opt-in:** other people's streams show as a "Watch Stream" / "Watch Camera" card, and the server only sends video to those who clicked it ("Stop watching" in the tile's corner) - no forced bandwidth. Click a tile for fullscreen with a member bar (green dot while talking). Quality levels "Best" (1440p screen / 1080p camera, up to ~10 Mbit/s), "High" (1080p / 720p, ~6 Mbit/s) and "Balanced" (720p / 540p, ~2.5 Mbit/s); monitors are captured with DXGI Desktop Duplication, pictures scaled with SIMD, OpenH264 with its assembly (~25 ms per 1440p frame from capture to H.264 instead of ~180 ms); viewers who join late or lose frames ask the sender for a keyframe
- **Stage layouts (local view only):** the Grid and Focus buttons and a "..." menu in the bottom right of the stage (shown with two or more tiles) switch the tile area between **Grid** (all tiles equal), **Focus** (one stream or person large, the others in a strip beside or below it) and **Presenter** (one person's screen large with their camera at a quarter of the width, starting in the bottom left; drag the camera and it snaps to the closest corner, click it for fullscreen, hover it for a stop-watching button, the camera stays on top). Hovering a tile shows a focus button (top right) that makes it the big one; clicking the tile itself still opens it fullscreen (or starts watching). By default the layout is **automatic**: Presenter while you watch someone's screen and they also share a camera, otherwise Grid; picking a layout by hand turns that off ("Automatic layout" in the "..." menu turns it back on). The "..." menu has **Presenter** (a toggle: again goes back to the layout before), **Only camera and screen shares** (hides everyone without video) and, in Focus and Presenter, who is shown; people who can't be presenter (no screen share or camera) are greyed out with the reason. Picking someone there starts watching their streams. **Remove background** (in the Presenter menu) cuts the presenter out of their camera picture so only the person sits on the screen share - person segmentation with the embedded MediaPipe Selfie Segmentation model (Apache-2.0, ~0.5 MB, SHA-256 checked, no download) on ONNX Runtime, in its own thread, about 2 ms per run and 3 ms per 720p frame in a release build. Only you see the layout and the cut-out; nothing about it is sent. The automatic switch, the mode, the background switch, the video-only switch and the camera corner are saved in `client_config.toml`
- **Server settings** (in the app): name, icon, welcome message and rules in the lobby, **join/leave messages** in an announcement text channel, AFK notice template, message of the day, **auto AFK mover** (muted users are moved after N minutes, default 10) — roles/bans/invites as WIP
- **Report a Bug** button that opens a pre-filled GitHub issue
- **Server:** single binary + `server.toml`, systemd service or hardened Docker image (amd64/arm64), one port (7331 TCP+UDP), optional password, keepalive/timeouts for dead connections

### Not yet (WIP)

- Toggle-mute/deafen hotkeys and more — see [ROADMAP.md](ROADMAP.md)

---

## Tech stack

| Area | Choice |
|---|---|
| Language | Rust (edition 2021), Cargo workspace |
| GUI | `eframe`/`egui` 0.31 + `egui-phosphor` (icons), custom theme, `emojis` (emoji names and shortcodes) + bundled Noto Emoji font |
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

- **TCP:** login, channels, chat (text + images), events, video frames (camera/screen), channel order, server settings, chat edits/replies, GIF search, roles/moderation/invites (protocol v17, TLS)
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

Prebuilt binaries are attached to the [GitHub releases](https://github.com/rusttalkcrew/RustTalk/releases) (`rusttalk-server-linux-x86_64.tar.gz`).

#### Update the systemd server (copy and paste)

The server looks for a newer release on start and once a day (`check_updates` in `server.toml`, on by default) and shows admins "Server update available" in the client. It never installs anything by itself - the service user cannot write its own program.

For the standard install at `/opt/rusttalk/server/`, paste this on the server from any directory (bash or fish):

```bash
sudo /opt/rusttalk/server/bin/rusttalk-server update --no-restart &&
sudo systemctl restart rusttalk-server.service &&
systemctl --no-pager --full status rusttalk-server.service &&
/opt/rusttalk/server/bin/rusttalk-server --version
```

The updater selects the latest release for your architecture, downloads it, verifies SHA-256, checks that the binary starts with `--version`, and replaces it. Each command runs only if the previous one succeeds. The restart briefly disconnects users. Configuration, database and TLS identity files stay in place; update clients too when the protocol version changes. No manual `wget`, `chmod` or deletion of the running binary is needed.

Check for an update without installing:

```bash
/opt/rusttalk/server/bin/rusttalk-server update --check
```

After an update, the previous binary stays at `/opt/rusttalk/server/bin/rusttalk-server.old`. To restore that binary (this does not roll back database changes):

```bash
sudo mv /opt/rusttalk/server/bin/rusttalk-server.old /opt/rusttalk/server/bin/rusttalk-server &&
sudo systemctl restart rusttalk-server.service &&
systemctl --no-pager --full status rusttalk-server.service
```

If the service fails to start, inspect `sudo journalctl -u rusttalk-server.service -n 50 --no-pager`. For custom installs, adjust the binary path and service name. Without `--no-restart`, the updater restarts the standard service automatically if it is running. Windows servers use `rusttalk-server.exe update`; restart them yourself.

### Server (Docker)

The official image is `ghcr.io/rusttalkcrew/rusttalk-server` (linux/amd64 and linux/arm64), built from [Server/Dockerfile](Server/Dockerfile) by the release workflow. It is distroless: no shell, no package manager, the program owned by root and the server running as user 65532. Pulling it contacts GitHub's container registry (`ghcr.io`); the server itself makes no connections beyond the ones it makes without Docker (the GitHub release check).

Setup with the [compose example](Server/deploy/docker-compose.yml) (commands work in bash and fish):

```bash
mkdir rusttalk && cd rusttalk
curl -fsSLO https://raw.githubusercontent.com/rusttalkcrew/RustTalk/main/Server/deploy/docker-compose.yml
touch admin_password.txt && chmod 600 admin_password.txt && openssl rand -base64 32 > admin_password.txt
sudo chown 65532:65532 admin_password.txt && sudo chmod 0400 admin_password.txt
docker compose up -d
docker compose logs rusttalk        # shows the TLS fingerprint for your users
```

- **Admin password:** the container refuses to start without it. It is read from the file named by `RUSTTALK_ADMIN_PASSWORD_FILE` (the compose secret above): one line, at least 16 characters and at most 1024 bytes. It never goes into `server.toml` or the logs. Compose mounts the file as it is (it ignores `uid`/`mode` outside Swarm), so the container user 65532 must be able to read it: the `chown` above, or keep it yours and grant read access with `setfacl -m u:65532:r admin_password.txt`. Read it back with `sudo cat admin_password.txt`.
- **Admin login limits:** failed password claims back off per source IP (1 second up to 60 seconds), including reconnects. At most 1024 sources are remembered for five minutes, with a global budget of ten attempts per second. Successful login clears that source's failures. Users behind the same NAT share the limit.
- **No open mode in Docker:** unlike a bare server, a container never lets the first visitor take over. Nobody is admin until someone enters the admin password under Server Settings > Roles.
- **Ports:** publish 7331 **TCP and UDP**. TCP carries login and chat, UDP carries voice: without the UDP mapping (or with UDP blocked by a firewall) people can log in but nobody hears anyone.
- **Hardening:** the example runs with a read-only root filesystem, `cap_drop: [ALL]`, `no-new-privileges` and a small `/tmp` tmpfs. Never mount the Docker socket into the RustTalk container.
- **Health and stop:** `HEALTHCHECK` runs `rusttalk-server --healthcheck`, which only checks that the TCP listener accepts a connection (no login). `docker stop` sends SIGTERM; the server closes the database cleanly and exits.

**Volume layout** (`/data`, the named volume `rusttalk_rusttalk-data`, directory mode 0700, everything owned by 65532):

| File | Content |
|---|---|
| `server.toml` | Created on the first start (mode 0600) and never overwritten. Edit it while the server is stopped |
| `rusttalk.db` (+ `-wal`/`-shm` while running) | Channels, chat history, roles, bans, invites, settings |
| `tls-cert.der`, `tls-key.der` | The server's certificate and private key (mode 0600). Clients pin it: losing it means every user sees a "certificate changed" warning |

Back up the whole volume while the server is stopped (the archive contains the private key, keep it private):

```bash
docker compose stop rusttalk
docker run --rm -v rusttalk_rusttalk-data:/data:ro -v $PWD:/backup debian:trixie-slim sh -c 'set -eu; umask 077; archive=$(mktemp /backup/.rusttalk-backup.XXXXXX); tar -czf "$archive" -C /data .; mv -f "$archive" /backup/rusttalk-data.tgz'
docker compose start rusttalk
```

Restore the same way with `tar -xzpf /backup/rusttalk-data.tgz -C /data` (mount the volume read-write; running as root keeps the 65532 ownership). If you use a host directory instead of a named volume, give it to the container user first: `sudo chown -R 65532:65532 ./data && sudo chmod 700 ./data`.

**Updates:** the server still tells admins "Server update available" in the client (`check_updates`), but `rusttalk-server update` is disabled in the container: the image is replaced instead.

```bash
docker compose pull && docker compose up -d
```

Tags: `X.Y.Z` never changes, `X.Y` (used by the example) moves to new patch releases, `latest` moves to every release, including new minor versions that may refuse older clients. To freeze an exact build, pin its digest (shown in the release workflow summary):

```bash
docker buildx imagetools inspect ghcr.io/rusttalkcrew/rusttalk-server:0.11
# then in docker-compose.yml: image: ghcr.io/rusttalkcrew/rusttalk-server:0.11@sha256:<digest>
```

Each image carries build provenance and an SBOM (`docker buildx imagetools inspect <image> --format '{{json .Provenance}}'`, or `.SBOM`).

**Automatic updates (opt-in):** [docker-compose.watchtower.yml](Server/deploy/docker-compose.watchtower.yml) adds [Watchtower](https://github.com/nicholas-fedor/watchtower) (the maintained fork; the original `containrrr/watchtower` is archived), which checks once a day and only updates containers labelled `com.centurylinklabs.watchtower.enable=true`:

```bash
docker compose -f docker-compose.yml -f docker-compose.watchtower.yml up -d
```

Know the trade-offs before you turn it on:

- Whatever the tag points to is installed without review. A compromised release or registry account would reach your server within a day. Keep the `X.Y` tag (patch releases only), never `latest`; a digest pin cannot auto-update at all.
- Watchtower needs the Docker socket, which is root on the host. Anyone who controls Watchtower or its image controls the machine. Only Watchtower gets the socket, never RustTalk.
- An update restarts the server and drops everyone connected for a moment (the schedule is 04:00).
- Watchtower contacts `ghcr.io` once a day to look for new images.

Build and test the image yourself: `make docker-build` (or `docker build -f Server/Dockerfile -t rusttalk-server .`) and `make docker-test`.

### Client (Windows) – install

Download `rusttalk-windows.exe` from the [client release](https://github.com/rusttalkcrew/RustTalk/releases) and run it. It installs itself for your user (no admin rights): `%LOCALAPPDATA%\Programs\RustTalk`, Start menu entry (optional desktop shortcut), settings in `%APPDATA%\RustTalk`. Uninstall via Windows **Apps & features**.

**Updates:** the installed app checks GitHub for a newer release on start (Settings > About > Updates). A newer release shows an "Update" notice in the top right corner; clicking it (or Install in Settings > About) asks for confirmation, then downloads it, checks its SHA-256 checksum and restarts.

Command line (for admins/scripts): `rusttalk-windows.exe --install [--desktop-shortcut] [--no-start]`, `rusttalk.exe --uninstall [--quiet] [--remove-settings]`, `rusttalk.exe --update`.

### Client (Linux) – minimal build

Build and run from the repository root (Linux audio sharing links to `libpulse`; install `libpulse-dev` on Debian/Ubuntu or `libpulse` on Arch):

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
open action. Audio-only and screen audio capture use the default output monitor through PulseAudio or pipewire-pulse (see the audio-sharing requirements above).

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

Independent stable releases use `vMAJOR.MINOR.PATCH+client.HASH` or
`vMAJOR.MINOR.PATCH+server.HASH`. Only the selected package's Cargo version must
match the core. `HASH` is a lowercase 7–40 hexadecimal prefix of the peeled tag
commit, which must be reachable from fetched `origin/main`. Numeric fields have
no leading zeros and fit unsigned 64-bit integers. `index-server` is recognized
but refused until a real package exists. Prerelease tags are currently refused.
Build metadata identifies the component/commit; it does not order versions.
Protocol version and shared crate version remain independent.

Publishing needs explicit approval. After approval, from the reviewed main commit,
for example (fish-compatible; replace the core with the selected package version):

```fish
set release_hash (git rev-parse --short=12 HEAD)
set release_tag "v0.12.0+client.$release_hash"
git tag "$release_tag"
git push origin "$release_tag"
```

Before relying on independent discovery, ship a **combined transition release**
containing the component-aware updaters. Combined stable `vX.Y.Z` remains supported
and requires client, server and shared versions to match. Only combined releases
move GitHub's global latest pointer; component releases use `make_latest: false`.
Older binaries that query `/releases/latest` remain on the last combined release
and may need manual upgrade. Choose a matching client/server platform asset from
the [releases page](https://github.com/rusttalkcrew/RustTalk/releases) if an older
latest link lacks it. Discovery retains GitHub HTTPS/TLS and mandatory SHA-256
checksums; tag metadata is not a signature or new trust guarantee.

The [build workflow](.github/workflows/build.yml) accepts `all` (default), `client`
or `server`; it explicitly tests shared alongside the selected package. Native
Windows x64/ARM64 and Linux x64/ARM64 builds must all succeed. Manual builds on the
same ref cancel older manual builds; release publication is globally serialized
without cancelling the running release. GitHub can replace an older pending run
with a newer pending run; inspect cancelled runs before scheduling them again.
PR tests do not run release packaging.

The [release workflow](.github/workflows/release.yml) rejects malformed tags,
wrong versions, non-main commits and duplicate component/core versions, including
hash-length variants and overlapping legacy tags. It verifies exact matrix assets
and checksums, creates a fresh draft and uploads without replacement before any
Docker push. Reruns refuse existing releases, including failed drafts. A partial
failure preserves that draft and any pushed image as evidence: inspect the run,
draft, tag and registry, then obtain explicit operator approval for recovery.
Never delete a draft just to rerun over published immutable aliases. Final
publication requires this run/attempt's draft and exact asset digests, and retains
automatically generated release notes.

Server and combined releases smoke-test and push linux/amd64 and linux/arm64 to
`ghcr.io/rusttalkcrew/rusttalk-server`. Immutable aliases are `vX.Y.Z`, `X.Y.Z` and
(for component tags) `vX.Y.Z-server.HASH`; `+` becomes `-` in Docker. Existing
immutable aliases cause refusal. `X.Y` and `latest` intentionally move. Client
releases never push server images. Server/combined tags also require the compose
example's image minor alias to match; update it when changing the server minor.

Run `make test-release` for offline synthetic Git/API/artifact failure tests.
These tests do not publish or exercise native builds, registry credentials,
GitHub runner permissions, Docker pushes, or platform hardware. Full native
matrix/package and image checks run only in CI; release execution needs approval.

Linux clients are `rusttalk-linux-x86_64.AppImage` and
`rusttalk-linux-aarch64.AppImage`, with checksums. Built on Ubuntu 24.04, they bundle
capture plugins and libraries, but require a compatible host (glibc 2.39 or newer),
Wayland, PipeWire, a desktop portal backend, and host GPU drivers. ALSA and
PipeWire libraries, plugins and configuration come from the host as one stack;
install the host ALSA bridge for your audio service (for example `pipewire-alsa`
on Arch). The image does not override `ALSA_PLUGIN_DIR` or `SPA_PLUGIN_DIR`.
Packaging rejects bundled ALSA/PipeWire libraries or plugins and runs
`--check-runtime` inside the image to verify the capture elements load, then tests
bad-checksum rejection, self-replacement, and relaunch against a local release server.

Windows clients are `rusttalk-windows.exe` (x64; existing updater name retained)
and `rusttalk-windows-arm64.exe`. Bare servers use
`rusttalk-server-linux-{x86_64,aarch64}`, `rusttalk-server-windows.exe`, and
`rusttalk-server-windows-arm64.exe`, each with `.sha256`. Linux server tarballs
also include configuration and deployment scripts. `rusttalk-server update`
selects the matching OS and architecture.

To package a locally built Linux release, run `make appimage`.
Run `make test-packaging` for audio packaging and launcher regression checks
without audio hardware. These checks do not verify microphone or speaker operation.
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
# Docker image: build + container smoke test (needs Docker)
make docker-test
```

## Project structure

```
├── shared/            # protocol (v17), framing, secure (TLS pinning + packet encryption), UDP packet format
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

- The server certificate is self-signed: on first connect RustTalk shows its SHA-256 fingerprint and asks before sending anything (name, identity, password). Compare it with the `TLS fingerprint: ...` line in the server log to rule out a man in the middle. Once trusted it is pinned; a changed certificate is a hard stop
- Every server you connect to receives the same secret identity over TLS (it stores only a hash), so servers that compare notes could link your accounts. Per-server identities are planned in #36
- Notifications from several servers arrive only while RustTalk runs and is connected; there is no push service, and messages sent while a server was disconnected are not all notified afterwards
- GIFs need one free GIPHY API key per server (developers.giphy.com), entered under Server Settings > Integrations
- Video runs over the TCP control connection (H.264): fine in a LAN, on weak Wi-Fi pick "Balanced"
- Link safety warnings are local heuristics plus the optional feed you import; there is no live reputation lookup. Google Safe Browsing (non-commercial only, needs an API key) and URLhaus downloads (need a personal Auth-Key) cannot ship inside RustTalk, so known-malicious detection only works with a feed you download and refresh yourself. Redirects, URL shorteners, downloads whose address carries no file name, and pages that turn malicious later are not detected. Links cannot be focused with the keyboard yet
- Natural voices use `ort` 2.0 release candidate `=2.0.0-rc.15` (no stable 2.0 exists yet; pinned exactly so builds don't change on their own). Switch to 2.0.0 once it is out

Separate application audio can be enabled with **Send separately** in the share dialog (up to eight apps). Listeners get an independent volume and mute for each app. Generic labels are the default; sharing app names with voice-channel members is opt-in. Labels are session-only. Client and server must both support protocol v17 (UDP audio header v4).
