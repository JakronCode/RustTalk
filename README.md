# RustTalk

Schlanker Voice-Chat in Rust — Client im Discord-Look, Server im TeamSpeak-Stil.
**Lokal-first:** keine Online-Accounts, keine Cloud. Client: Windows (egui), Server: Linux (Debian, systemd).

![Status](https://img.shields.io/badge/status-MVP-green)

---

## Features

### Funktioniert

- **Voice:** Push-to-Talk + Sprachaktivierung (VAD mit RMS *und* RNNoise-Sprachwahrscheinlichkeit), Opus 64 kbit/s 48 kHz/20 ms über UDP mit In-Band-FEC, Jitter-Buffer mit FEC/PLC, Sinc-Resampler für beliebige Mikro-Raten
- **Automatische Lautstärke** (AGC + Limiter): leise Mikros werden angehoben, laute gedämpft – abschaltbar
- **KI-Rauschunterdrückung** (RNNoise via `nnnoiseless`), abschaltbar
- **Channel-Typen:** Spawn (1×, Join-Log, Sprechverbot), Kategorie (Gruppierung), Sprache (Voice + Media-Chat), Text (Chat)
- **Text-Chat** mit Verlauf, Bildern (PNG/JPEG/GIF), Ungelesen-Markierung
- **SQLite-Persistenz** (Server): Channels, Chat-Verlauf (Retention 10k/Channel), Spawn-Log überleben Neustarts
- **Client-UI:** Dark-Mode (fest), prozedurales Logo, Lesezeichen für Server, Einstellungen als eigenes Fenster, Mikro-Pegel-Meter, Avatare mit Talking-Ring, lokale Stummschaltung pro User (Rechtsklick)
- **Video (MJPEG ~25 fps über TCP):** Webcam mit Geräteauswahl + Live-Vorschau, Bildschirm/Fenster teilen mit Auswahldialog („Anwendungen" / „Gesamter Bildschirm"); beides gleichzeitig möglich. Klick auf eine Kachel = Vollbild mit User-Leiste (grüner Punkt beim Sprechen)
- **Server:** Single-Binary + `server.toml`, systemd-Service, ein Port (7331 TCP+UDP), optionales Passwort

### Kaputt — Fix in Arbeit (WIP)

- ⛔ **GIPHY-Suche**: Integration existiert (`media.rs`, API-Key in den Einstellungen), funktioniert aber nicht — Button deaktiviert. Wird später gefixt.

---

## Tech-Stack

| Bereich | Wahl |
|---|---|
| Sprache | Rust (Edition 2021), Cargo-Workspace |
| GUI | `eframe`/`egui` 0.31 + `egui-phosphor` (Icons), eigener Font/Theme |
| Audio | `cpal` (I/O), `audiopus` (Opus), `nnnoiseless` (RNNoise), `ringbuf` (lock-free) |
| Netzwerk | `tokio`; TCP (Steuerung, length-prefixed `bincode`) + UDP (Audio, eigener 20-Byte-Header) |
| Server-DB | `rusqlite` (bundled SQLite, WAL) |
| Medien | `image`, `rfd` (Datei-Dialog); WIP: `nokhwa`, `scrap`, `ureq`+`serde_json` |
| Server-Betrieb | systemd-Unit (gehärtet: `ProtectSystem=strict` + `ReadWritePaths`) |

## Architektur (Kurzform)

```
Client (Windows)                          Server (Debian)
┌──────────────┐   TCP (Control, bincode)  ┌──────────────┐
│ egui UI      │ ◄───────────────────────► │ TCP-Listener │
│ cpal capture │ ── Opus/UDP (Audio) ────► │ UDP-Relay    │
│ cpal play    │ ◄───────────────────────  │ State (RAM)  │
└──────────────┘                           │ SQLite (DB)  │
                                            └──────────────┘
```

- **TCP:** Login, Channels, Chat (Text + Bilder), Events, Video-Frames (Kamera/Bildschirm, v5)
- **UDP:** nur Opus-Audio (20 ms Frames, Token+Sequence-Header), Server relayed 1:1 innerhalb des Voice-Channels
- **Spawn-Channel:** alle landen dort beim Verbinden; Sprechen serverseitig blockiert; Join-Log

Details: [Konzept.md](Konzept.md) · Testprotokoll: [test.md](test.md)

---

## Setup

### Server (Debian/Linux)

Voraussetzungen: Rust (1.85+), systemd.

```bash
# bauen
cargo build --release -p rusttalk-server

# installieren + starten (als root, aus dem Repo-Root)
sudo bash Server/deploy/install_service.sh
```

Das Skript installiert nach `/opt/rusttalk/server/` (Binary + `server.toml`), legt den User `rusttalk` an und aktiviert den systemd-Service `rusttalk-server`. Konfiguration: [Server/server.toml](Server/server.toml) (Bind-Adresse, Passwort, `persist_state`, `db_path`, Start-Channels).

Port **7331** (TCP+UDP) in der Firewall freigeben, falls nftables/ufw aktiv ist.

### Client (Windows)

Voraussetzungen: Rust (MSVC-Toolchain inkl. C++-Buildtools).

> ⚠️ **WDAC-Hinweis (dieser Entwicklungsrechner):** WDAC blockiert cargo-Build-Skripte aus `Documents` (os error 4551). Deshalb **immer über den Wrapper** bauen/testen, der nach `%LOCALAPPDATA%\Programs\RustTalk-src` spiegelt:

```powershell
# bauen
powershell -ExecutionPolicy Bypass -File Client\Windows\Test_Scripts\cargo.ps1 -Retries 12 build --release -p rusttalk-client

# starten
& "$env:LOCALAPPDATA\Programs\RustTalk-src\target\release\rusttalk.exe"
```

Auf Rechnern ohne solche Policies reicht schlicht `cargo build --release -p rusttalk-client` (dann liegt die exe unter `target\release\`).

Beim Start: Server-Adresse eintragen (z. B. `10.69.40.101`), Benutzername, Verbinden. Server als Lesezeichen speichern = Klick auf „Als Lesezeichen speichern".

### Tests

```powershell
# Windows (alles)
powershell -ExecutionPolicy Bypass -File Client\Windows\Test_Scripts\cargo.ps1 -Retries 12 test --workspace
# Windows: E2E gegen Live-Server
powershell -ExecutionPolicy Bypass -File Client\Windows\Test_Scripts\e2e_server.ps1
```

```bash
# Linux: Server-Tests + lokaler E2E-Smoke
bash Server/Test_Scripts/run_tests.sh
bash Server/Test_Scripts/e2e_local.sh
```

## Projektstruktur

```
├── shared/            # Protokoll (v5), Framing, UDP-Paketformat
├── Server/            # Linux-Server (+ test_client CLI, Integrationstests, deploy/, Test_Scripts/)
│   └── src/           #   main, config, state (RAM), db (SQLite), tcp, udp
└── Client/Windows/    # egui-Client
    └── src/           #   app (UI), net, audio/ (capture, encoder+RNNoise, playback, jitter,
                       #   resample), media (Bilder/GIPHY), video (Kamera/Bildschirm/Fenster), config, logo
```

## Bekannte Einschränkungen

- Keine Verschlüsselung/Authentifizierung außer optionalem Server-Passwort (LAN-Einsatz)
- PTT wirkt nur bei fokussiertem Fenster (globaler Hotkey: geplant)
- Keine Echo-Kompensation → Kopfhörer empfohlen
- GIPHY: **kaputt, Fix in Arbeit**
- Video als MJPEG über TCP: bei „Hoch" ~5–6 MB/s pro Bildschirm-Stream und Zuschauer – fürs LAN gedacht (im WLAN „Ausgewogen" wählen). Bei sehr großen Monitoren (3440×1440) schafft „Hoch" ~19 fps
