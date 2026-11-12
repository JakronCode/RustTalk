# RustTalk – Roadmap

Checklist of planned features, by priority. Status as of 2026-11-11 (v0.6.0, protocol v12). All categories A-D done.

## A – Quick wins (WIP spots already visible in the UI)

- [x] **A1 Per-user volume** – "User Volume" in the user right-click menu (0–200 %), stored locally per username
- [x] **A2 Auto-reconnect** – on connection loss: retry with backoff, rejoin the last channel
- [x] **A3 Input volume** – manual microphone gain before the processing chain (Settings > Voice & Audio)
- [x] **A4 Server messages** – join/leave messages (announcement text channel), rules (lobby), AFK notice, MOTD (Server Settings > Messages)

## B – Core feature gaps

- [x] **B1 Global push-to-talk** – PTT also works while the window is not focused (opt-in, keys + Mouse4/5)
- [x] **B2 GIFs** – Discord-style GIF picker, one GIPHY key per server (searched + cached by the server), animated GIFs in chat
- [x] **B3 Notifications** – sounds for join/leave/message/mute, taskbar flash
  - [x] Desktop pop-up notifications (Windows toasts)
- [x] **B4 Chat features** – edit/delete messages, replies, @mentions, search
- [x] **B5 Installer + updater** – the release exe installs itself per user (Start menu, Apps & features), settings in AppData, in-app updates from GitHub releases, release workflow

## C – Moderation & permissions

- [x] **C1 Roles & permissions** – identities (secret per client, hash on the server), Member / Moderator / Admin, open server until the first admin, `admin_password` in `server.toml`
- [x] **C2 Kick / ban / move users** – right-click menu, bans by identity + IP with duration and reason, ban list, audit log
- [x] **C3 Invite links** – `rusttalk://host:port/CODE` (uses/expiry, replaces the server password), URL handler registered by the installer

## D – Larger projects

- [ ] Keine Portable mehr haben, sprich installieren und dann persistenten client haben
	-> dazu natürlich auch die update function die sich die akutelleste version vom repo zieht
- [x] **D1 Echo cancellation (AEC)** – WebRTC AEC3 (pure Rust port `aec3`), switchable in Voice & Audio
- [x] **D2 Share audio with screen** – WASAPI loopback, own RustTalk playback removed by a second AEC3, Opus music mode
- [x] **D3 Encryption** – TLS (self-signed server certificate, pinned on first use) + ChaCha20-Poly1305 for audio packets
- [x] **D4 More efficient video** – H.264 (OpenH264) instead of MJPEG, ~10x less bandwidth, keyframe requests
