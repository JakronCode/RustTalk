# AGENTS.md

Guidance for AI coding agents (and humans) working on RustTalk. `CLAUDE.md` points here, so this is the single source of truth.

## What RustTalk is

Lightweight, local-first voice chat written in Rust: a Discord-style client with a TeamSpeak-style self-hosted server. No online accounts, no cloud, no telemetry. Whoever runs a server owns the data on it.

- **Server** (`Server/`): single Rust binary plus `server.toml`, runs on Linux (also builds for Windows) under a hardened systemd unit. One port, 7331 TCP+UDP. State in RAM plus SQLite (`rusqlite`, WAL).
- **Client** (`Client/Windows/`): egui/eframe app. Windows is the main target; the same crate also builds on Linux (`src/linux/`, AppImage, PipeWire/GStreamer capture).
- **Shared** (`shared/`): protocol (`protocol.rs`, currently v14), framing, `secure.rs` (TLS pinning and UDP packet encryption), UDP packet format, release/update helpers.
- **Transport**: TCP carries the control channel (TLS, length-prefixed `bincode`): login, channels, chat, images, video frames, moderation. UDP carries Opus audio only, sealed with ChaCha20-Poly1305 using a per-session key delivered over TLS.
- **Identity**: each client holds a secret identity. The server stores only its hash. Roles (Member / Moderator / Admin), bans and invites hang on the identity, not the username.
- **Updates**: client and server check GitHub releases. The server never installs anything by itself (the service user cannot write its own binary). Updates are SHA-256 verified.

More detail: `README.md` (features, setup, release process), `Konzept.md` (concept, German), `ROADMAP.md`, `test.md` (test log).

Repository layout:

```
shared/            protocol, framing, secure, UDP packet format
Server/src/        main, config, state, db, tcp, udp, tls, moderation, gif, update
Server/tests/      integration tests (real server process + synthetic clients)
Server/deploy/     systemd unit + install script
Client/Windows/    egui client (app/, audio/, linux/, net, media, video, update, ...)
packaging/ scripts/  Linux AppImage packaging
.github/workflows/ build.yml (test + build matrix on main), test.yml (PRs: tests + clippy), release.yml
```

## Issue board first (avoid duplicate work)

The board is the GitHub issue tracker of this repository (`JakronCode/RustTalk`). It is how agents and humans avoid doing the same work twice.

Before starting any task that changes code, config, docs or CI:

1. **Search the board.** Look through open issues and open pull requests for the same or overlapping work, by keyword and by affected file or area. Also check recently closed issues and open branches.
2. **Read what you find.** Check assignees, linked PRs, labels and recent comments. If someone (or another agent) is already on it, do not start. Report the existing issue to the user instead, and offer to help on that issue.
3. **If it overlaps partially**, tell the user which issue it overlaps with and how, and let them decide: extend the existing issue, split it, or proceed separately.
4. **If it is new, ask the user before starting:** "Do you want me to create an issue for this first?" Do not create issues, comments or assignments on your own and do not skip the question. If the user says yes, create the issue (clear title, problem or goal, scope, whether it touches the protocol, the wire format or security) and reference it in your branch name, commits and PR (`Closes #N`). If the user says no, proceed and do not create one.
5. **Pick up existing issues the same way**: tell the user which issue you are taking and, if they agree and you have the permission, assign it or comment that work has started so others see it.

Tools: use `gh` for GitHub. On this developer's machine `~/.local/bin/gh` is a wrapper that can hang; if it does, use the real binary under `~/.local/share/mise/installs/gh/latest/gh_*/bin/gh` wrapped in `timeout`. If you cannot reach the board at all (no network, no login), say so and ask the user how to proceed instead of silently skipping the check.

## Privacy and security (non-negotiable)

RustTalk is meant to be as private and secure as a vault. Every feature, fix and dependency is judged against that. When privacy or security conflicts with convenience, features or speed, privacy and security win. If a requested change weakens them, say so and propose a safer alternative before writing code.

### Privacy principles

- **Local first, no third parties by default.** No analytics, telemetry, crash reporting, tracking IDs, ads or accounts. Do not add any outbound connection that is not essential and not already documented. The only existing ones are GitHub release checks, the optional GIPHY lookup (done by the server, so clients never contact GIPHY and the key never leaves the server) and the one-time voice model download from Hugging Face (pinned revision, SHA-256 checked). Any new outbound call must be opt-in or clearly disclosed, and listed in `README.md`.
- **Data minimisation.** Collect, send and store the least possible. Prefer not storing something over storing it protected. Give everything stored a retention limit (chat already keeps at most 10,000 messages per channel).
- **Server owner and users stay in control.** Features that expose data (presence, IP addresses, message content, who joined when) need an explicit setting, sensible defaults that expose less, and a way to delete or turn them off.
- **No data in logs.** Never log message content, passwords, tokens, keys, invite codes, identity secrets or full IP addresses beyond what moderation strictly needs. Same for panics and error messages shown to users.
- **Secrets stay on the machine that owns them.** The client identity secret never leaves the client. The server stores only hashes. Server keys, the GIPHY key and `admin_password` never go to clients, logs or the repo.

### Security principles

- **Treat every network input as hostile.** Both directions: the server must not trust clients, and the client must not trust the server beyond what TLS pinning proves. Validate every length, count, index, enum and string at the boundary. Bound all allocations, frame sizes, queue depths, rates and per-user resource use. No unbounded `Vec` growth, no `unwrap`/`expect`/indexing on data from the network, no panics that one peer can trigger (a panic in a task is a denial of service).
- **Authorise on the server, every time.** Role and permission checks happen server-side for every action (kick, ban, move, role changes, invites, settings, edit/delete messages). Never rely on the client hiding a button. New protocol messages need an explicit permission decision.
- **Encryption is mandatory.** Control traffic goes over TLS with the pinned server certificate, media over ChaCha20-Poly1305 with unique nonces (counter plus direction; never reuse a key/nonce pair). Never add a plaintext fallback, a downgrade path, a "skip verification" switch, or custom crypto. Use the vetted crates already in the tree (`rustls`, `chacha20poly1305`, `sha2`, `ring`) and the OS RNG.
- **Secrets and comparisons.** Compare secrets, tokens, password hashes and MACs in constant time. Passwords and secrets are not stored in plaintext where avoidable (the existing plaintext `password`/`admin_password` in `server.toml` is known debt: do not copy that pattern into anything new). Rate-limit and back off on authentication attempts, invites and anything guessable.
- **Least privilege.** The server runs as an unprivileged user under a hardened systemd unit (`NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`, minimal `ReadWritePaths`). Keep it that way and tighten it when you can (more sandboxing is welcome, less is not). The service must never be able to replace its own binary.
- **Safe persistence.** SQL uses parameterised statements only, never string formatting. Key and secret files are created with mode 0600 and never world-readable. No secrets in config examples.
- **Safe updates and downloads.** Anything downloaded (updates, models, GIFs) is size-limited, from a pinned or verified source, SHA-256 checked before use, and written atomically. Never execute or load unverified downloads. Treat media from other users (images, GIFs, video) as hostile input for the decoders.
- **Supply chain.** Keep dependencies few, maintained and pinned via `Cargo.lock` (build with `--locked`). Justify every new crate in the PR (why, maintenance status, license, what it pulls in, `unsafe` use). Prefer pure-Rust over C where it is practical. Do not add build scripts or network access at build time without a very good reason. Run `cargo audit` or `cargo deny` when you touch dependencies, if available. Pin GitHub Actions, and keep workflow permissions minimal.
- **No secrets in the repository.** Never commit keys, certificates with private keys, tokens, `.env` files, databases or real server addresses and credentials.
- **Fail closed.** On a parse error, a certificate mismatch, a failed checksum, an unknown message or a permission doubt: refuse and report, never guess or continue. A changed pinned certificate stays a hard stop with a clear warning.

### Protocol changes

Changing `shared/src/protocol.rs` or the wire format means bumping `PROTOCOL_VERSION`, updating both sides, extending the integration tests and updating the protocol version in `README.md`. Old clients must be refused cleanly rather than half-working.

### Before you finish a change that touches the network, auth, crypto, storage, updates or parsing

1. Write down (in the PR description) the abuse case: what could a malicious client, server, network attacker or other user do with this? How is it bounded?
2. Add tests for the bad paths (oversized, malformed, unauthorised, replayed, truncated input), not just the happy path.
3. For crypto, auth or update-channel changes, ask for an independent review before merging.

### Known issues to fix, do not copy

- `Server/tls-cert.der`, `Server/tls-key.der`, `Client/Windows/testdata/tls-cert.der` and `Client/Windows/testdata/tls-key.der` are tracked in git. A private key in the repo must not be used by any real server. Never add more key material to git. Untrack them, add them to `.gitignore`, generate test certificates at test time, and rotate any key that was ever deployed.
- `password` and `admin_password` in `server.toml` are plaintext.

## Working in this repo

- **Read the task runner first.** There is no Makefile. The scripts that carry the project's flags are `Server/Test_Scripts/run_tests.sh`, `Server/Test_Scripts/e2e_local.sh`, `Client/Windows/Test_Scripts/*.ps1` and `scripts/package-linux.sh`. CI is `.github/workflows/test.yml` for pull requests and `.github/workflows/build.yml` for main and releases. Use `--locked`.
- **Run tests serially where required.** Integration tests share TLS files and run with a single thread in CI.
- **Quality gate before saying "done":** `cargo test --locked` for the touched crates (`bash Server/Test_Scripts/run_tests.sh` for the server), `cargo clippy` clean for new code, `cargo fmt`. Report failures honestly, with the output.
- **Releases:** bump the version in `Client/Windows/Cargo.toml`, `Server/Cargo.toml` and `shared/Cargo.toml` together, then tag `vX.Y.Z`. The release workflow checks the versions match.
- **Platform split:** Windows-only code stays behind `cfg(windows)`, Linux-only code in `Client/Windows/src/linux/`. Don't break either target. UI and networking code is shared.
- **Conventions:** Rust 2021. User-visible text and docs in English; some existing comments and `Konzept.md` are German, so match the file you are editing. Keep the README feature list, protocol version and known limitations accurate when behaviour changes.
- **Comments:** keep them sparse. One line is usually enough, and only where the code can't say it (the why, not the what). No multi-paragraph comment blocks, no commented-out code, no restating the code.
- **Small, focused changes.** One concern per branch and PR, with the issue number in it.
- **Don't act outward without asking:** no pushing, tagging, releasing, publishing, or posting to the issue tracker unless the user asked for it (see the issue rules above for creating issues).
