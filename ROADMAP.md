# RustTalk – Roadmap

Checklist of planned features, by priority. Status as of 2026-11-11 (v0.7.1, protocol v12). All categories A-D done.

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

- [x] **D1 Echo cancellation (AEC)** – WebRTC AEC3 (pure Rust port `aec3`), switchable in Voice & Audio
- [x] **D2 Share audio with screen** – WASAPI loopback, own RustTalk playback removed by a second AEC3, Opus music mode
- [x] **D3 Encryption** – TLS (self-signed server certificate, pinned on first use) + ChaCha20-Poly1305 for audio packets
- [x] **D4 More efficient video** – H.264 (OpenH264) instead of MJPEG, ~10x less bandwidth, keyframe requests

## E – Feedback round (2026-11-12)

Bugs:

- [x] **E1 Streams end when you leave** – camera/screen share stop on every channel switch (not only in the lobby) and do not come back when you return; the server ends "sharing" on a move too; late frames no longer bring a stopped stream's tile back
- [x] **E2 Creating voice channels** – "+" next to a category did not react (hover-only button vanished on click); the dialog now picks the type that is already in the category (Voice under "Voice Channels"); "+" only for admins
- [x] **E3 Switching audio devices** – input/output changes apply right away (audio restarts in a few ms, falls back to the previous device on error) instead of only on the next connect

Improvements (priority order):

- [ ] **E4 Better video quality** – webcam and screen share: higher bitrate/resolution presets, frame pacing, maybe hardware encoder
- [ ] **E5 Focused video view** – a video/details view where you pick whose streams you watch and hide others (opt-in watching)
- [ ] **E6 GUI overhaul** – go through all screens for details (spacing, states, empty states, consistency)

Ideas:

- [ ] **E7 "Fort Knox" connection security** – audit and harden: certificate pinning UX (fingerprint check on first connect), forward secrecy for audio keys (rotate per session/interval), replay protection for UDP, rate limits, optional end-to-end encryption between clients
- [ ] **E8 Relay server** (idea from StarDebris) – a separate forwarding server for people who cannot reach the voice server directly (NAT, blocked ports) or to hide the server's IP; clients connect to the relay, the relay passes traffic through without being able to read it
- [ ] **E9 Zero-knowledge server** (idea from StarDebris) – the server only relays and stores data it cannot read: end-to-end encrypted audio/video/chat with keys only on the clients (channel keys exchanged between members), server sees only metadata (who is in which channel); trade-off: server-side features like chat search, GIF proxy and history for new members get harder
