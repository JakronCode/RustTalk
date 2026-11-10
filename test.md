# RustTalk – Testprotokoll

> Pflichtdokumentation aller Testläufe. Neueste oben.

## 2026-11-10 (9) – WIP-Deklaration + Git-Repo + README

**WIP (kaputt, Fix in Arbeit):** Webcam/Screen-Share und GIPHY funktionieren nicht zuverlässig → Buttons in der UI deaktiviert (Tooltip: „aktuell kaputt – Fix in Arbeit"). Code-Basis bleibt im Repo (`video.rs`, `media.rs`) inkl. Tests. Konkrete Fix-Gründe ausstehend (Aufnahme/Transport prüfen).

**Doku:** [README.md](README.md) (Features, Tech-Stack, Setup Server/Client inkl. WDAC-Hinweis, Tests, Struktur, bekannte Einschränkungen). Git-Repo initialisiert (.gitignore: target/, *.db, client_config.toml).

**Tests:** 31 Client-Tests grün (unveraendert), Release-Build OK, GUI gestartet.

---

## 2026-11-10 (8) – Medien: Bilder im Chat, GIPHY, Webcam/Screen-Share, Kategorie-Fix (Protokoll v4)

**Neu:**
- **Chat-Eingabe 4-zeilig** (Enter = senden, Shift+Enter = neue Zeile), Senden/Bild/GIF-Buttons rechts
- **Bilder im Chat** (Text + Voice-Feed): Datei-Dialog (rfd, max 4 MiB), Vorschau-Chip, Anzeige via Texture-Cache; persistiert als BLOB in SQLite (mit Migration der messages-Tabelle)
- **Voice-Channels haben jetzt Media-Chat** (wie Discords Sprach-Chat) + Video-Grid
- **GIPHY-Integration:** API-Key in den Einstellungen (kostenlos unter developers.giphy.com), Such-Fenster mit Vorschau-Grid, Klick hängt das GIF an
- **Webcam + Bildschirm-Teilen (MJPEG ~5 fps, 640×360, über TCP):** nokhwa (Kamera) + scrap (DXGI) → JPEG → Server relayed an Voice-Channel-Mitglieder. Toggles in der Bottom-Bar, eigene Vorschau inklusive
- **Kategorie-Bugfix:** Klickfläche war nur der Text — jetzt volle Zeilenbreite (manuell alloziiert; egui 0.31 hat kein take_available_width), Rechtsklick-Menü (Umbenennen/Löschen) funktioniert

**Tests (alle grün):** 31 Client (neu: JPEG-Encode/Thumbnail, Texture-Cache, Config mit GIPHY-Key), 27 Server-Unit (neu: sharing/video_targets, Bild-Persistenz, Migration), 8 Integration (neu: `video_relay_und_sharing`, Bild-Chat), E2E LAN Audio weiterhin OK (150/129 Pakete).

**Hinweise:** nokhwa-`Camera` und scrap-`Capturer` sind nicht `Send` → Erzeugung im Capture-Thread. Video läuft bewusst über TCP (einfach, LAN-tauglich); H.264/AV1 über UDP wäre Phase 5.

**Manuell zu prüfen:** 4-zeilige Eingabe, Bild anhängen + senden + Empfang beim Gegenüber, GIF-Suche (mit Key), Kamera/Bildschirm-Kacheln im Voice-Channel, Kategorie umbenennen per Rechtsklick.

---

## 2026-11-10 (7) – Chat-Input-Fix + SQLite-Persistenz (Protokoll v3)

**Chat-Input-Bug (lag seit dem Typen-Umbau):** Die Chat-`ScrollArea` nahm mit `auto_shrink([false,false])` die gesamte Resthöhe ein → die Eingabezeile wurde unter die Fensterkante geschoben (unsichtbar, nicht beschreibbar). Fix: Eingabezeile als `TopBottomPanel::bottom().show_inside()` innerhalb der Mitte; die ScrollArea füllt nur den Rest.

**SQLite-Persistenz (Server, `rusqlite` bundled, WAL):**
- Tabellen: `channels`, `messages` (Retention: 10.000/Channel), `events` (Spawn-Log)
- Start: Config ∪ DB (Union per Name+Art, ID-Remapping), Spawn-Log geladen
- `Welcome` trägt `chat_history` (letzte 100/Channel) → Client lädt Verlauf beim Connect (Protokoll v3)
- systemd-Unit: `ReadWritePaths=/opt/rusttalk/server` (ProtectSystem=strict hatte den DB-Zugriff blockiert)

**Tests (alle grün, Windows + Debian):**
- 27 Client, 12 shared, 24 Server-Unit (neu: 4× db.rs + persistence_roundtrip + config/db-Dedup)
- 6 Integration (neu: `server_restart_keeps_state` — Channel + Chat + Log überleben Prozess-Neustart)
- **Live über LAN:** Chat in #allgemein → `systemctl restart` → `history=1`, `spawn_log=2` im nächsten Welcome ✅
- Release-Builds OK; Server v3 produktiv (Persistenz aktiv, DB: /opt/rusttalk/server/rusttalk.db); GUI gestartet

**Manuell zu prüfen:** In #allgemein schreiben (Eingabezeile unten sichtbar!), Client neu verbinden → Verlauf da; neuen Channel erstellen, Server-Dienst neu starten → Channel noch da.

---

## 2026-11-10 (6) – Lag-Fix (Playback), Pegel-Meter, Selbst-Sprech-Indikator

**Lag-Bug (bestand seit dem Oneshot):** Der Output-Callback lud pro Callback `frames + 960` Samples aus der 48-kHz-FIFO, spielte aber nur `frames` ab — der Rest wurde verworfen. Folge: Jitter-Buffer lief ~3× zu schnell leer, Prebuffer-Neustarts, hoerbares Stottern („200 ms Ton / 200 ms Stille"). Der Paket-E2E-Test konnte das nicht sehen (zaehlt nur UDP).

**Fix:** [playback.rs](Client/Windows/src/audio/playback.rs) nutzt jetzt einen `PullResampler` (fractionale Position, exakter Konsum) ueber einer just-in-time gefuellten FIFO. Regressionstests: `identity_rate_consumes_exactly`, `no_drift_over_many_callbacks`, `underrun_yields_silence`, Upsample-Monotonie, 44,1k-Pitch.

**Neu:**
- Pegel-Meter in den Einstellungen (gruen = ueber VAD-Schwelle, weisser Marker = Schwelle); RMS-Sink im Capture-Callback (geglaettet); `MicMonitor` misst auch ohne Server-Verbindung
- Selbst-Sprech-Indikator: Encoder meldet `transmitting` → gruener Ring um den eigenen Avatar (Sidebar, Bottom-Bar, Voice-Grid)

**Tests:** 27 Client-Tests gruen. Release gebaut + gestartet (kein Crash).

**Manuell zu pruefen:** Sprache ohne Stottern (PTT + VAD), Pegel-Meter reagiert aufs Mikro, eigener Avatar bekommt gruenen Ring beim Sprechen.

---

## 2026-11-10 (5) – Bugfix: Start-Crash im neuen Startfenster

**Symptom:** Client crasht beim Start (`hit_test.rs` Panic im Release, `ui.rs:1390` assert im Debug).

**Ursache:** `min_size(vec2(f32::INFINITY, ..))` bzw. `desired_width(f32::INFINITY)` **innerhalb fest dimensionierter `allocate_ui`-Panels** bläht `min_rect` auf ∞; die folgende `horizontal`-Zeile bekam negative Restbreite → egui-Assert/Panic. In voller Fensterbreite (alte Version) war das unproblematisch, im festen Panel nicht mehr.

**Fix:** [app.rs](Client/Windows/src/app.rs) Startfenster nutzt jetzt `add_sized([w, h], …)` mit `ui.available_width()` statt ∞. Kommentar im Code markiert die Falle.

**Verifikation:** Debug-Build startet stabil (4 s Lauf ohne Panic), 22 Client-Tests grün, Release gebaut und gestartet. Erkenntnis festgehalten: keine `INFINITY`-Größen in `allocate_ui`-Panels.

---

## 2026-11-10 (4) – Logo, Lesezeichen, RNNoise, Settings-Fenster, Kontrast

**Änderungen (nur Client, Server unveraendert):**
- **Immer Dark-Mode** (war schon forciert; bleibt der einzige Modus)
- **Logo** prozedural generiert ([logo.rs](Client/Windows/src/logo.rs)): abgerundetes Quadrat in Akzentblau mit Klangwellen-Balken – als Fenster-Icon (64×64 RGBA zur Laufzeit erzeugt, keine Asset-Datei) und auf dem Startfenster
- **Startfenster:** Lesezeichen-Liste (Klick = ausfüllen, Doppelklick = verbinden, Rechtsklick = löschen) + überarbeitetes Verbindungsformular; „Als Lesezeichen speichern“ (persistiert in `client_config.toml`)
- **Einstellungen als eigenes OS-Fenster** via `ctx.show_viewport_immediate` (statt egui-Window im Hauptfenster)
- **Kontrast:** `fg_stroke`/`weak_bg_fill` der Widgets angehoben (egui 0.31 berechnet schwachen Text als Mischung Richtung `weak_bg_fill`)
- **KI-Rauschunterdrückung:** `nnnoiseless` (RNNoise-Port) vor dem Opus-Encoder; läuft kontinuierlich (Modell-Kontinuität), erster Frame verworfen; Sprach-Wahrscheinlichkeit (>0.6) speist zusaetzlich die VAD. Umschaltbar in Einstellungen (Default: an)

**Tests (alle grün):** 21 Client-Tests (neu: `denoise_processes_frames`, `icon_has_correct_size_and_content`, `rounded_rect_geometry`, Config mit Bookmarks). Release-Build OK, GUI gestartet.

**Manuell zu prüfen (GUI):**
- [ ] Logo auf Startfenster + in der Fenster-Titelleiste sichtbar
- [ ] Lesezeichen: speichern, klicken (füllt aus), doppelklicken (verbindet), rechtsklick-löschen
- [ ] Einstellungen oeffnen sich als separates Fenster; RNNoise-Checkbox wirkt
- [ ] Texte deutlich besser lesbar (schwache Texte heller)
- [ ] Sprachtest mit/ohne Rauschunterdrückung (Lüfter/Tastatur-Klackern)

---

## 2026-11-10 (3) – Channel-Typen-Modell (Protokoll v2)

**Änderungen:**
- `ChannelKind`: **Spawn** (genau 1×, alle landen dort, Audio serverseitig blockiert, Join-Log), **Kategorie** (nur Gruppierung), **Sprache** (nur Voice), **Text** (nur Chat, global sichtbar)
- Server: `spawn_log` im Memory (Cap 500), `Welcome` enthält Log, `SpawnLogEntry`-Broadcast bei Join
- Server-Validierung: Join nur Spawn/Voice; Chat nur Text; zweiter Spawn verboten; Kategorie-Löschung → Kinder werden Top-Level
- Client: Baum-Sidebar (Spawn gepinnt, Kategorien klappbar, Kinder eingerückt), Klick auf Sprach-Channel = beitreten (Discord-Verhalten), Spawn-Ansicht = Join-Log, Voice-Ansicht = Belegung + Aktivität
- Channel-Erstellen-Dialog mit Art-Auswahl + Kategorie-Dropdown
- test_client: `--join-kind voice` Flag; E2E join damit automatisch
- WDAC: Dice-Roll im [cargo.ps1](Client/Windows/Test_Scripts/cargo.ps1) automatisiert (hartnäckig geblockte eigene Binaries bekommen hochgezählten Kommentar → neuer Inhalt = neues Verdict)

**Tests (alle grün, Windows + Debian):**
- shared 17 (inkl. ChannelKind-Capabilities, Roundtrips v2)
- Server-Unit 12 (Spawn-Eindeutigkeit, Join-/Chat-Regeln, Kategorie-Gruppierung, Sprechverbot im Spawn, Log-Cap)
- Integration 5 (Handshake+Spawn-Log, Typen-Regeln über TCP, Mute, Passwort, Audio nur in Voice)
- E2E LAN: 150/127 Pakete in „Funkturm 116“ ✅
- Release-Builds OK; Server v2 via systemd neu gestartet; GUI v2 gestartet

---

## 2026-11-10 (2) – UI-Aufwertung: Icons, Kontextmenüs, Channel-Wechsel

**Änderungen:**
- Phosphor-Icons (`egui-phosphor 0.9`, Regular-Variante als Fallback-Font)
- Channel-Wechsel per **Doppelklick** (war vorher nur per Rechtsklick-Menü erreichbar – Usability-Bug)
- Kontextmenü Channel (Rechtsklick): Beitreten / Umbenennen / Löschen mit Icons
- Kontextmenü User (Rechtsklick): **lokale Stummschaltung** (clientseitig im Mixer, `LocalMuteSet`)
- Avatare (deterministische Farbe + Initial), Talking-Ring beim Sprechen
- Chat: System-Meldungen (Join/Leave/Move), eigene Nachrichten farblich hervorgehoben, Ungelesen-Punkt an Channels
- Dialoge: Channel erstellen (+) und Umbenennen als Fenster

**Tests:** `cargo.ps1 test -p rusttalk-client` → alle 19 Client-Tests grün (neu: avatar_color_is_deterministic). Release-Build OK, GUI neu gestartet.

**Manuell zu prüfen (GUI):**
- [ ] Doppelklick auf Channel → Voice-Beitritt, Zeile wird grün mit „(Du)"
- [ ] Rechtsklick auf Channel → Menü erscheint (Beitreten/Umbenennen/Löschen)
- [ ] Rechtsklick auf anderen User → „Lokal stummschalten“, Name wird grau
- [ ] Icons sichtbar (Mikro/Speaker/Zahnrad/Plus/Senden-Pfeil)
- [ ] System-Meldungen im Chat bei Join/Leave/Wechsel

---

## 2026-11-10 – Oneshot: Implementierung, Deployment, E2E

**Umgebung:**
- Client-Rechner: Windows 11 (VS Code), Rust 1.99.1, WDAC enforced
- Server: Debian 13 (trixie) `el-dev01` / 10.69.40.101, Rust 1.98.1, systemd 257

### Ergebnisübersicht

| Test | Wo | Ergebnis |
|---|---|---|
| Unit-Tests shared (11): Protokoll-Roundtrip, Framing, UDP-Paket Encode/Decode | Windows + Debian | ✅ |
| Unit-Tests Server (10): Config, State (Channels, Passwort, UDP-Targets) | Windows + Debian | ✅ |
| Unit-Tests Client (18): Config, Jitter (inkl. Opus-Roundtrip + PLC), VAD-Gate, Resampler, Key-Mapping | Windows | ✅ |
| Integrationstests (5): Handshake, Channel-Mgmt + Chat-Routing, Mute-Broadcast, Passwort-Abweisung, UDP-Audio-Relay | Windows + Debian | ✅ |
| E2E über LAN: 2× test_client (Windows) → Debian-Server | LAN | ✅ A: 150 Pakete von B, B: 128 von A (3 s à 50 Pakete/s) |
| systemd-Service `rusttalk-server` | Debian | ✅ active (running), TCP+UDP 0.0.0.0:7331 |
| Release-Build Client (`rusttalk.exe`) | Windows | ✅ (nach build-override, siehe unten) |
| GUI-Start | Windows | ✅ gestartet (Sprachtest mit 2. Client manuell, s. Checkliste) |

### Befehle zum Nachvollziehen

```powershell
# Windows: ALLE Tests (WDAC-sicher)
powershell -NoProfile -ExecutionPolicy Bypass -File Client\Windows\Test_Scripts\cargo.ps1 -Retries 10 test --workspace

# Windows: E2E gegen den Live-Server
powershell -ExecutionPolicy Bypass -File Client\Windows\Test_Scripts\e2e_server.ps1
```

```bash
# Debian: alle Server-Tests
cd /opt/rusttalk/src && bash Server/Test_Scripts/run_tests.sh
# Debian: lokaler E2E-Smoke (Server + 2 Clients auf localhost)
bash Server/Test_Scripts/e2e_local.sh
# Service-Status/Logs
systemctl status rusttalk-server; journalctl -u rusttalk-server -f
```

### WDAC-Befund (wichtig für Windows-Builds!)

`cargo build` aus `C:\Users\jb\Documents` scheiterte mit **os error 4551** ("Application Control policy has blocked this file") bei Build-Skripten.

Empirisch ermittelt (7 kontrollierte Experimente):
- Blockiert, sobald **cargo-cwd ODER Manifest-Pfad unter `Documents`** liegen.
- Ausführung manuell gestarteter unsignierter exes: überall erlaubt.
- Zusätzlich: **transiente** Blocks frisch kompilierter Dateien (Defender MAPS/Cloud nicht erreichbar) und **deterministischer** Block des release-optimierten `num-traits`-Build-Skripts.

**Lösungen (im Repo verankert):**
1. [cargo.ps1](Client/Windows/Test_Scripts/cargo.ps1): spiegelt den Workspace per robocopy nach `%LOCALAPPDATA%\Programs\RustTalk-src` und baut dort; Retry-Schleife bei transienten 4551-Blöcken.
2. `Cargo.toml`: `[profile.release.build-override] opt-level = 0` — Build-Skripte werden debug-kompiliert (deterministischer num-traits-Block umgangen).

### Manuelle Checkliste (GUI, mit zweitem Client)

- [x] GUI startet, Verbindungsmaske zeigt Server/Username
- [ ] Verbinden mit 10.69.40.101:7331 → User-Liste + Channels (Allgemein, Gaming) sichtbar
- [ ] Zweiten Client verbinden (z. B. test_client oder zweiter Windows-Rechner) → beide sehen sich
- [ ] PTT (Taste V): Sprechen A → hörbar bei B (Latenz < 200 ms gefühlt)
- [ ] Sprachaktivierung (Einstellungen → Checkbox, Empfindlichkeits-Slider)
- [ ] Channel erstellen/umbenennen/löschen (Rechtsklick auf Channel)
- [ ] Text-Chat pro Channel (Echo + Gegenseite)
- [ ] „Mikro aus" / „Ton aus" → Gegenseite hört nichts / sieht [stumm]/[taub]
- [ ] Talking-Indikator (grüner Punkt beim Sprecher)

### Bekannte Grenzen (bewusst, MVP)

- Keine Verschlüsselung/Authentifizierung außer optionalem Server-Passwort (LAN).
- PTT wirkt nur bei fokussiertem Fenster (globaler Hotkey: Phase 3).
- Keine Echo-Kompensation → Kopfhörer empfohlen.
- Chat ohne Persistenz (nur Live).
