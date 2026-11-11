# RustTalk – Roadmap

Checklist of planned features, by priority. Status as of 2026-11-11 (protocol v9). Categories A and B done.

## A – Quick wins (WIP spots already visible in the UI)

- [x] **A1 Per-user volume** – "User Volume" in the user right-click menu (0–200 %), stored locally per username
- [x] **A2 Auto-reconnect** – on connection loss: retry with backoff, rejoin the last channel
- [x] **A3 Input volume** – manual microphone gain before the processing chain (Settings > Voice & Audio)
- [x] **A4 Server messages** – join/leave messages (announcement text channel), rules (lobby), AFK notice, MOTD (Server Settings > Messages)

## B – Core feature gaps

- [x] **B1 Global push-to-talk** – PTT also works while the window is not focused (opt-in, keys + Mouse4/5)
- [x] **B2 Fix GIPHY** – GIF search in the composer, animated GIFs in chat
- [x] **B3 Notifications** – sounds for join/leave/message/mute, taskbar flash
  - [ ] Desktop pop-up notifications (Windows toasts) – still open
- [x] **B4 Chat features** – edit/delete messages, replies, @mentions, search

## C – Moderation & permissions

- [ ] **C1 Roles & permissions** – at minimum an admin password from `server.toml`
- [ ] **C2 Kick / ban / move users** – incl. ban list and audit log
- [ ] **C3 Invite links** – `rusttalk://` URL handler

## D – Larger projects

- [ ] **D1 Echo cancellation (AEC)** – e.g. `webrtc-audio-processing`
- [ ] **D2 Share audio with screen** – WASAPI loopback
- [ ] **D3 Encryption** – QUIC (`quinn`) or TLS
- [ ] **D4 More efficient video** – H.264 / hardware encoding instead of MJPEG
