# RustTalk – Testprotokoll

> Pflichtdokumentation aller Testläufe. Neueste oben.

## 2026-11-11 (22) – D1–D3: Echo-Unterdrückung, Bildschirmton, Verschlüsselung (Protokoll v12, v0.6.0)

**Client / Server:**
- **D1 Echo-Unterdrückung** (`audio/aec.rs`): WebRTC AEC3 als reiner Rust-Port (`aec3`, kein C++-Build). Die Wiedergabe gibt jeden gemischten 48-kHz-Sample per lock-freiem Ring an den Encoder; pro 10 ms werden Mikrofon und Referenz durch Hochpass + AEC3 geschickt (eigenes RNNoise/AGC bleiben). Schalter „Echo cancellation“ (Standard an). Rechenzeit ~1,5 % (8 s Audio in 0,12 s).
- **D2 Bildschirmton** (`audio/screen_audio.rs`): WASAPI-Loopback des Ausgabegeräts (cpal), eine zweite AEC3-Instanz entfernt die eigene RustTalk-Wiedergabe (sonst hörten alle die anderen Stimmen doppelt), Opus Musikmodus 96 kbit/s, Stille wird nicht gesendet. UDP-Flag `FLAG_SCREEN_AUDIO` (Server leitet unverändert weiter); Empfänger: eigener Decoder pro Person+Stream, Mute/Lautstärke pro Person gelten, kein Sprech-Indikator. Schalter „Share audio“ im Bildschirm-Dialog + Einstellungen (Standard an).
- **D3 Verschlüsselung** (`shared/secure.rs`, `Server/tls.rs`): TCP über TLS (`rustls`, ring-Backend → nur C-Compiler nötig). Server erzeugt beim ersten Start ein selbstsigniertes Zertifikat neben der DB, loggt den Fingerprint; Client pinnt ihn beim ersten Verbinden (`known_servers`), ein anderes Zertifikat wird verweigert („The identity of … changed“, Button „Trust the new identity“), kein Auto-Reconnect in diesem Fall. Audio: Header v2 (28 Byte, 64-Bit-Zähler), ChaCha20-Poly1305 mit Sitzungsschlüssel aus dem `Welcome`; Server öffnet mit dem Schlüssel des Senders und versiegelt pro Empfänger neu; die UDP-Adresse wird erst nach gültigem Tag gelernt (vorher konnte ein gefälschtes Token die Audio-Adresse umbiegen). Settings > Advanced: Verschlüsselungsstatus, Fingerprint, bekannte Server („Forget“).

**Tests:** Client 80 ✅ (neu: AEC entfernt ≥15 dB eines 40-ms-Echos und lässt Sprache ohne Wiedergabe durch, Mixer-Keys, verweigertes geändertes Zertifikat; manuell: Loopback öffnet ✅), Server 50 ✅ (neu: Zertifikat bleibt über Neustarts gleich), Integration 15 ✅ (alle jetzt über TLS + verschlüsseltes Audio-Relay; neu: Klartext-Client bekommt kein Welcome), Shared 16 ✅ (neu: Pakete verschlüsseln/manipulieren, Fingerprints).
**Live geprüft:** Testserver + `test_client` + Debug-Client über TLS: „Connected encrypted“, Fingerprint = Server-Log, Server in `known_servers` gespeichert (Screenshot).
**Nicht geprüft:** Echo-Unterdrückung und Bildschirmton mit echten Lautsprechern/Ton (hätte Testtöne auf deinem Rechner abgespielt), Zertifikatswarnung per Screenshot.

---

## 2026-11-11 (21) – Kategorie C: Identitäten, Rollen, Kick/Ban/Verschieben, Invites, Audit-Log (Protokoll v11, v0.5.0)

**Entscheidungen (Rückfrage):** Identitäten wie bei TeamSpeak statt nur Admin-Passwort; ohne Admin bleibt der Server offen wie bisher.

**Server** (`moderation.rs`, Persistenz als bincode-Blobs in der `settings`-Tabelle):
- **Identität:** Client sendet im `Hello` ein 256-Bit-Geheimnis (hex), der Server speichert nur SHA-256 davon („uid“). Rollen und Bans hängen an der uid, Bans zusätzlich an der IP.
- **Rollen:** Member / Moderator (verschieben, kicken, fremde Nachrichten löschen) / Admin (Channels, Einstellungen, GIF-Key, Rollen, Bans, Invites, Audit-Log). Prüfung zentral vor jeder Aktion (`required_role`), Ziel muss eine niedrigere Rolle haben, nicht man selbst; letzter Admin kann nicht entfernt werden.
- **Offener Modus:** solange niemand Admin ist, darf jeder alles (Verhalten wie v0.4). „Become admin“ (`ClaimAdmin`) macht den ersten Admin; danach nur noch mit `admin_password` aus `server.toml`.
- **Kick/Ban:** Ziel bekommt `Removed { reason, banned }`, Verbindung wird geschlossen; Ban mit Dauer (1 h … permanent) und Grund, Login meldet „You are banned … for N more minute(s): Grund“. Abgelaufene Bans fallen automatisch weg.
- **Invites:** 8-stelliger Code, max. Nutzungen + Ablauf, ersetzt das Server-Passwort.
- **Audit-Log:** Channels anlegen/löschen/umbenennen, Einstellungen, GIF-Key, Rollen, Kick, Ban, Unban, Verschieben, Invites (max. 500 Einträge).

**Client:** Identität wird beim ersten Start erzeugt (`client_config.toml`), Rollen-Schild neben Namen, Rechtsklick auf Nutzer: Move to / Kick… / Ban… (Grund + Dauer) / Role, Server-Settings-Tabs **Roles** (offener Modus + „Become admin“, Admin-Login, Rollenliste), **Bans** (Unban), **Invites** (erstellen, kopieren, löschen), **Audit Log**. Nicht-Admins: Channel-Anlegen und „Save Changes“ gesperrt. Nach Kick/Ban kein Auto-Reconnect, Grund wird angezeigt. Invite-Links ins Adressfeld einfügen oder per `rusttalk://` öffnen (Installer registriert das Protokoll).

**Tests:** Client 76 ✅ (neu: Identität, Invite-Links, Datumsformat), Server 49 ✅ (neu: uid-Hash, offener Modus/letzter Admin, Bans uid/IP/Ablauf, Invites, Audit-Cap, **Neustart behält Rollen/Bans/Invites/Audit**), Integration 14 ✅ (neu: `roles_kick_ban_and_audit`, `invites_replace_the_password_and_admin_password_works`), Shared 13 ✅.
**Live geprüft (Testserver 7444 + `test_client` „Bob“, Screenshots):** Roles-Tab offen, „Become admin“ + Invite (Link `rusttalk://127.0.0.1:7444/…`), Audit-Log, Admin-Rolle nach Client-Neustart erhalten (Identität).
**Nicht geprüft:** Rechtsklick-Menü/Kick-/Ban-Dialog per Maus, `rusttalk://`-Registrierung in einer echten Installation (Testlauf mit Deinstallation wurde vom Sicherheits-Check blockiert, da er Einstellungen löscht), Öffnen eines Links aus dem Browser.

---

## 2026-11-11 (20) – Windows-Toasts, Installation statt portabel, Updater (v0.4.0)

**Wünsche:** Desktop-Pop-ups (Windows-Toasts); keine portable Version mehr, sondern installieren + persistenter Client; Update-Funktion, die sich die aktuellste Version aus dem Repo holt.

**Client:**
- **Installation** (`install.rs`): Die Release-Exe ist ihr eigener Installer. Außerhalb des Installationsordners gestartet zeigt sie „Install RustTalk“ → `%LOCALAPPDATA%\Programs\RustTalk\rusttalk.exe`, Startmenü- (+ optional Desktop-)Verknüpfung (`mslnk`), Eintrag in „Apps & Features“ (HKCU, Deinstallation über `--uninstall`, fragt ob Einstellungen gelöscht werden sollen), App-ID für Toasts (HKCU `AppUserModelId`). Einstellungen der installierten App in `%APPDATA%\RustTalk` (alte `client_config.toml` wird übernommen). Kein Admin nötig. Dev-Builds (Debug oder aus `target\`, also auch `cargo.ps1`) bleiben portabel wie bisher.
- **Updater** (`update.rs`): prüft beim Start `releases/latest` von `JakronCode/RustTalk` (User-Agent „RustTalk-Updater“, keine Nutzerdaten), Settings > About > Updates mit „Install update & restart“ + Fortschritt; Download → SHA-256 gegen `.sha256` aus dem Release (ohne Prüfsumme kein Update) → laufende Exe umbenennen, neue an ihren Platz, Neustart. Hinweis-Banner auf dem Verbindungsbildschirm.
- **Toasts** (`notify.rs`, `tauri-winrt-notification`): neue Nachricht bei Fenster im Hintergrund → Pop-up „Name · Channel“ + Text (abschaltbar: „Show message text“); Klick holt das Fenster nach vorne und öffnet den Channel; max. 1 Toast / 2 s (Erwähnungen immer). Installiert als „RustTalk“ mit Icon, portabel unter „PowerShell“.
- Kommandozeile: `--install [--desktop-shortcut] [--no-start]`, `--uninstall [--quiet] [--remove-settings]`, `--update`. Release-Builds ohne Konsolenfenster.
- **Release-Workflow** `.github/workflows/release.yml`: bei Tag `v*` Versionsprüfung (Tag = Cargo-Versionen), Windows-Client als `rusttalk-windows.exe` + `.sha256`, Linux-Server-Paket. Versionen auf **0.4.0** angehoben.

**Tests:** Client 73 ✅ (neu: Modus-Erkennung, Icons, Versionsvergleich, Release-Parsing, Prüfsummen, Toast-Texte; Toast manuell per `--ignored` ✅), Server 43 ✅, Integration 12 ✅, Shared 13 ✅.
**Live geprüft (Windows, echtes Benutzerprofil, danach wieder entfernt):** Installationsbildschirm (Screenshot); `--install --desktop-shortcut` → Exe, Icons, beide Verknüpfungen (Ziel/Icon/Arbeitsordner), Apps-&-Features-Eintrag 0.4.0, App-ID; `--update` gegen Fake-GitHub: falsche Prüfsumme → abgelehnt, Exe unverändert; gültiges Release v0.4.1 → Exe getauscht, Hash stimmt; gleiche Version → nichts; `--uninstall --quiet --remove-settings` → alles entfernt (**Bugfix:** Ordner blieb zuerst stehen, weil `cmd.exe` Rusts `\"`-Quoting nicht versteht → `raw_arg`). Echter Toast über die Windows-API angezeigt.
**Nicht geprüft:** Toast-Klick, Toast mit „RustTalk“-App-ID im installierten Client, Install-Button per Maus, Release-Workflow auf GitHub (läuft erst beim ersten Tag).

---

## 2026-11-11 (19) – Klarere UI fürs Löschen, Antworten und Bearbeiten

**Wünsche:** Beim Löschen nachfragen, beim Antworten die Nachricht hervorheben, beim Bearbeiten Haken/X zum Bestätigen/Abbrechen.

**Client:**
- **Löschen:** Dialog „Delete Message“ mit Vorschau der Nachricht (Avatar, Name, Zeit, Text), rotes „Delete“ / „Cancel“, Enter/Esc. Shift+Klick auf den Papierkorb löscht ohne Nachfrage (wie Discord).
- **Antworten:** Ziel-Nachricht deutlich hervorgehoben (Akzentfarbe + Balken + Hinweis „Replying“ oben rechts), dazu die Leiste „Replying to …“ über dem Eingabefeld.
- **Bearbeiten:** jetzt direkt in der Nachricht statt im Eingabefeld: Textfeld mit grünem „✓ Save“ und „✗ Cancel“, Enter = speichern, Esc = abbrechen, Shift+Enter = neue Zeile; leer speichern fragt nach Löschen; unveränderter Text wird nicht gesendet. Pfeil-hoch im leeren Eingabefeld öffnet die letzte eigene Nachricht im Inline-Editor.
- **Bugfix:** Esc in Textfeldern wurde ignoriert (egui gibt den Fokus bei Esc ab, bevor die Prüfung lief) – betraf auch das Abbrechen einer Antwort. Gefunden durch den neuen Test.

**Tests:** Client 67 ✅ (neu: Inline-Editor Enter/Esc mit simulierten Tasten), Server 43 ✅, Integration 12 ✅, Shared 13 ✅.
**Live geprüft (Screenshots, Testserver 7444):** Inline-Editor, Lösch-Dialog, Antwort-Hervorhebung (neue Debug-Hooks `edit`, `delete`, `reply` in `RUSTTALK_DEV_OPEN`).
**Nicht geprüft:** mit echter Maus/Tastatur; Release-Client nicht neu gebaut (lief gerade).

---

## 2026-11-11 (18) – Chat-Bugfixes, GIF-Picker wie Discord, GIPHY-Key pro Server (Protokoll v10)

**Rückmeldung:** Hover-Leiste, Bearbeiten/Löschen und Antworten/Erwähnungen „buggen“; GIF-Fenster wie bei Discord; kein API-Key im Client, sondern pro Server (Server Settings).

**Ursachen (Chat):**
- Die Hover-Leiste (Antworten/Bearbeiten/Löschen) liegt in einer eigenen Vordergrund-Ebene über der Nachricht. Die Hover-Prüfung (`rect_contains_pointer`) verlangt, dass die *oberste* Ebene unter dem Mauszeiger die der Nachricht ist – sobald die Maus die Leiste erreichte, galt die Nachricht als nicht mehr gehovert → Leiste verschwand/flackerte, Klicks gingen ins Leere. Fix: Hover = Zeiger über Nachricht **oder** Leiste, oberste Ebene ist Chat **oder** Leiste.
- Nach „Antworten“/„Bearbeiten“ bekam das Eingabefeld keinen Fokus → Tippen/Enter wirkten wirkungslos. Fix: Fokus ins Eingabefeld, beim Bearbeiten Cursor ans Textende; ebenso nach Auswahl eines @-Vorschlags.

**GIFs (Protokoll v10):** Suche läuft über den Server (`GifSearch` → `GifResults`), Key einmal pro Server unter Server Settings > **Integrations** (wird beim Speichern mit einer Anfrage geprüft, in der Server-DB getrennt von den Einstellungen abgelegt, nie an Clients gesendet; Clients erfahren nur `gif_search: bool` / `GifSearchChanged`). Server cacht Seiten 10 min (Gratis-Key: ~100 Anfragen/h). Client-Key-Einstellung entfernt. **Picker:** Popup über dem GIF-Button, Trending beim Öffnen, Suche beim Tippen (350 ms Debounce), zwei Spalten (Masonry, Seitenverhältnis bleibt), Nachladen beim Scrollen, nur sichtbare Vorschauen werden geladen, Hover animiert, Klick sendet sofort (auch als Antwort), Esc/Klick daneben schließt; ohne Key Hinweis + Button zu den Server Settings.

**Tests:** Client 66 ✅ (neu: **Hover-Regressionstest** mit simulierten Mausereignissen – schlägt mit der alten Logik nachweislich fehl –, Masonry-Layout), Server 43 ✅ (neu: GIPHY-Parsing, Cache, Key-Speicherung), Integration 12 ✅ (neu: `gif_search_needs_a_server_key`, `gif_key_is_checked_and_search_is_cached` mit Fake-GIPHY im Test), Shared 13 ✅.
**Live geprüft:** Testserver (Port 7444) + lokaler Fake-GIPHY (`RUSTTALK_GIPHY_API`, nur für Tests) + Debug-Client, Fenster-Screenshots: Picker ohne Key (Hinweis), Picker mit Trending-Grid (nur sichtbare Vorschauen geladen), Integrations-Tab.
**Nicht geprüft:** echter GIPHY-Key, GIF per Klick senden und Hover-Leiste mit echter Maus (Klick-Logik per Regressionstest abgedeckt).

---

## 2026-11-11 (17) – Kategorie B: globales PTT, GIPHY, Benachrichtigungen, Chat-Funktionen (Protokoll v9)

**Wünsche:** Kategorie B aus [ROADMAP.md](ROADMAP.md) umsetzen, testen, pushen.

**Client:**
- **B1 Globales Push-to-Talk:** Neues Modul `hotkey.rs`. Ein Thread fragt die PTT-Taste alle 10 ms per `device_query` (`GetAsyncKeyState`) ab – kein Tastatur-Hook, kein `unsafe` im Projekt, die Taste kommt weiter bei anderen Apps an. Opt-in unter Keybinds („Push to talk in the background“). Neu: Maus-Seitentasten Mouse4/Mouse5 als PTT-Taste (auch im fokussierten Modus).
- **B2 GIPHY repariert:** Ursache – das Vorschau-Grid suchte Texturen per Hash der *URL*, der Download legte sie per Hash der *Bilddaten* ab → Vorschauen blieben ewig „...“. Außerdem waren Originale oft > 4 MiB (Server lehnt ab) und Fehler wurden verschluckt. Jetzt: Vorschauen nach URL gecacht, Anhang = `downsized`-Variante, Größenlimit mit Meldung, 401/429 verständlich, API-Key nie in Fehlertexten. GIF-Button im Composer aktiv. **Animierte GIFs** im Chat und im Picker (Einstellung „Animated GIFs“).
- **B3 Benachrichtigungen:** Töne (im Code erzeugt, keine Dateien) für Join/Leave im eigenen Voice-Channel, neue Nachricht (anderer Channel, Fenster im Hintergrund oder @Erwähnung) und Mute/Unmute/Deafen; Taskleiste blinkt bei neuen Nachrichten im Hintergrund. Einstellungen mit Vorhör-Button. Desktop-Pop-ups bleiben WIP.
- **B4 Chat:** Antworten (Zitat über der Nachricht), eigene Nachrichten bearbeiten („(edited)“) und löschen – per Hover-Leiste oder Rechtsklick, Esc bricht ab, Pfeil-hoch bearbeitet die letzte eigene Nachricht. @Erwähnungen: gelbe Hervorhebung, Ton, Autovervollständigung beim Tippen von „@“. Suche im Channel-Kopf (Lupe).

**Server / Protokoll v9:** Nachrichten-IDs (DB-Zeilen-ID bzw. Zähler ohne Persistenz), `reply_to`, `EditChat`/`DeleteChat` → `ChatEdited`/`ChatDeleted`. Nur eigene Nachrichten (per Name, wie der Chat-Verlauf), Antwort-ID wird verworfen, wenn sie nicht im selben Channel existiert. DB-Migration: Spalten `reply_to`, `edited`.

**Tests:** Client 64 ✅ (neu u. a.: Tastenbelegung/Maus, OS-Tastenabfrage, GIPHY-Parsing, GIF-Animation, Töne, Erwähnungen), Server 40 ✅ (neu: Bearbeiten/Löschen/Antworten, Nachrichten-IDs in der DB), Integration 10 ✅ (neu: `chat_reply_edit_delete`), Shared 13 ✅ – Windows, Release-Client via `cargo.ps1`.
**Live geprüft:** Isolierter Testserver (Port 7444, eigene DB mit eingespielten Nachrichten) + Debug-Client, Fenster-Screenshots (nur das RustTalk-Fenster): Erwähnung hervorgehoben, Antwort-Zitate, „(edited)“, „Original message was deleted“, GIF-Button, Lupe; Einstellungsseiten Keybinds und Notifications. Bestehende `rusttalk.db` (v8) beim Serverstart live auf v9 migriert. GIPHY: ungültiger Key → HTTP 401 (wird als „GIPHY rejected the API key“ angezeigt).
**Nicht geprüft:** Globales PTT mit echtem Tastendruck im Hintergrund, Töne mit echtem Hören, GIPHY-Suche mit gültigem Key, Hover-Leiste/Suche/Autovervollständigung per Maus und Tastatur.

---

## 2026-11-11 (16) – Kategorie A: Lautstärke pro Nutzer, Auto-Reconnect, Eingangslautstärke, Servernachrichten (Protokoll v8)

**Wünsche:** Checkliste ([ROADMAP.md](ROADMAP.md)), Kategorie A umsetzen, testen, pushen.

**Client:**
- **A1 Lautstärke pro Nutzer:** Rechtsklick auf einen Nutzer → Regler 0–200 % (+ „Reset to 100 %“). Wird im Mixer pro Sprecher angewendet, lokal per Nutzername in `client_config.toml` (`user_volumes`) gespeichert und beim Verbinden/Join wieder angewendet.
- **A2 Auto-Reconnect:** Bricht eine bestehende Verbindung unerwartet ab, verbindet der Client neu (Backoff 1, 2, 4, 8, 15, 30 s …, max. 25 Versuche), betritt den letzten Voice-Channel wieder (per ID, Fallback Name) und stellt Mute/Deafen wieder her. Verbindungsbildschirm zeigt Countdown, „Reconnect now“ und „Cancel“. Manuelles Trennen startet keinen Reconnect.
- **A3 Eingangslautstärke:** Settings > Voice & Audio, 0–200 %, wirkt im Capture-Callback vor Pegelanzeige, VAD, RNNoise und AGC (Anzeige und Schwelle passen zusammen).
- **A4 Servernachrichten:** Server Settings > Messages: Announcement-Text-Channel, Join-/Leave-Nachricht (`{user}`), Regeln (in der Lobby angezeigt), AFK-Hinweis (`{channel}`, `{minutes}`), Message of the Day (Toast beim Verbinden).

**Server / Protokoll v8:** `ServerSettings` um `join_message`, `leave_message`, `announce_channel`, `rules`, `afk_notice`, `motd` erweitert; Validierung (300 Zeichen, Regeln 1000, Announcement nur Text-Channel), gelöschter Announcement-Channel → aus. Ansagen kommen als Chat von „RustTalk“ (id 0) und werden persistiert. **Migration:** gespeicherte v7-Einstellungen (bincode) werden gelesen und um Defaults ergänzt – Name/Icon/AFK bleiben erhalten.

**Tests:** Client 55 ✅ (neu: Backoff, Eingangs-Gain, Config-Felder), Server 38 ✅ (neu: Servernachrichten-Validierung/Ansagen, AFK-Vorlage, v7-Migration), Integration 9 ✅ (neu: `join_leave_announcements_and_motd`), Shared 13 ✅ (neu: Platzhalter) – Windows (direkt + Release-Build via `cargo.ps1`).
**Live geprüft (Windows, lokaler Server):** Debug-Client mit `RUSTTALK_DEV_AUTOCONNECT` + `join:Lounge`, Server gekillt und nach 8 s neu gestartet → Client verbindet sich selbst neu und ist wieder in der Lounge (`UserMoved { channel_id: 6 }` beim Beobachter-`test_client`).
**Nicht geprüft:** Regler pro Nutzer / Eingangslautstärke mit echtem Ton und per Screenshot, Messages-Tab per Screenshot, Linux-Server-Build.

---

## 2026-11-11 (15) – Servereinstellungen, AFK-Mover, Disconnect, Bug-Report (Protokoll v7)

**Wünsche:** Servereinstellungen (Auto-AFK nach 10 min stumm, Logo + Name, Platzhalter für Nachrichten), sichtbarer Disconnect soll den Server trennen (nicht in die Lobby), Disconnect raus aus dem Server-Menü, dort „Server Settings“ rein, Menü wurde immer breiter, Lobby per Klick beitreten, „Report a Bug“-Button.

**Protokoll v7:** `ServerSettings { name, icon, welcome_message, afk_enabled, afk_timeout_mins, afk_channel }` im `Welcome`, `UpdateServerSettings` → `ServerSettingsChanged` (an alle), `Notice` (Hinweis an einen Client).

**Server:** Einstellungen validiert (Name, Icon ≤ 256 KiB, Willkommenstext ≤ 1000 Zeichen, Timeout 1–240 min, AFK-Ziel nur Voice/Lobby) und in SQLite gespeichert (Tabelle `settings`, bincode); Config-`server_name` nur als Startwert. **AFK-Mover:** Server merkt sich, seit wann ein Client stumm/taub ist; alle 15 s werden Voice-Nutzer über dem Limit ins AFK-Ziel (Standard: Lobby) verschoben, bekommen eine `Notice` und bleiben stumm. Gelöschter AFK-Channel → zurück auf Lobby. Kein Rollen-System: jeder darf ändern (LAN).

**Client:**
- Neuer Dialog „Server Settings“: Overview (Icon hochladen → quadratisch 128 px PNG, Name), AFK (an/aus, Timeout, Ziel-Channel), Messages (Willkommenstext funktioniert; Join/Leave/Regeln/AFK-Hinweis/MOTD als WIP), Roles/Bans/Invites/Audit Log als WIP.
- Server-Icon (oder Initialen) im Sidebar-Kopf und in der Lobby; Lobby zeigt den Willkommenstext.
- Server-Menü: Server Settings, Create Channel/Category, App Settings, Report a Bug; Disconnect entfernt. **Breiten-Bug** behoben (Einträge nahmen `available_width`, die Zeile mit WIP-Badge machte das Popup jedes Frame breiter → feste Breite).
- Roter Button im Voice-Panel = **vom Server trennen** (immer sichtbar, auch in der Lobby). Voice verlassen = Klick auf die Lobby, die jetzt per Klick beitretbar ist.
- „Report a Bug“ (Server-Menü + About) öffnet ein neues GitHub-Issue mit Version/Protokoll vorausgefüllt.

**Tests:** Client 53 ✅ (neu: Icon-PNG 128×128, URL-Kodierung, Initialen), Server 35 ✅ (neu: Validierung, AFK-Sweep inkl. Taub/Entstummen/Aus, Einstellungen überleben Neustart), Integration 8 ✅ (neu: `server_settings_werden_verteilt`), Shared 12 ✅ – Windows + Linux. Screenshots: Server Settings (Overview, AFK, Messages), Voice-Panel mit Disconnect.
**Nicht geprüft:** AFK-Verschiebung live (minimal 1 min, nur Unit-Test), Menü-Popup per Screenshot (nicht automatisiert öffenbar), Icon-Upload per Dateidialog.

---

## 2026-11-11 (14) – UI-Rework (Englisch), Channel Drag & Drop, Einstellungen (Protokoll v6)

**Wünsche:** Channels per Drag & Drop verschieben, Channel-Einstellungen (vorerst visuell, WIP), Haupt-UI polieren/Icons sinnvoll anordnen, im Spawn Mikro/Kamera/Bildschirm ausgrauen, anderes Text-Channel-Icon, neutrale Standard-Channelnamen, komplette UI auf Englisch, Einstellungsmenü mit Kategorien.

**Server / Protokoll v6:**
- `ChannelInfo.position`, `ClientMessage::MoveChannel { channel_id, parent, index }` → `ServerMessage::ChannelsReordered`; `Welcome.server_name`.
- Regeln: Lobby bleibt oben fest, Kategorien nur auf oberster Ebene, Text/Voice oben oder in Kategorien; Geschwister werden neu nummeriert und gespeichert (DB-Spalte `position`, Migration).
- **Persistenz-Fix:** Mit Persistenz ist die DB jetzt die Quelle der Wahrheit; die Config befüllt nur beim allerersten Start. Vorher wurde die Config bei jedem Start neu angelegt und per Name abgeglichen → umbenannte Config-Channels doppelt.
- Standard-Channels: Lobby, „Text Channels“ (general, announcements), „Voice Channels“ (Lounge, Gaming). Servertexte (Fehler, Join-Log) auf Englisch.

**Client:** `app.rs` (2900 Zeilen) aufgeteilt in `app/{mod, theme, connect, sidebar, views, settings, channel_settings, media_ui}.rs`.
- Sidebar: Server-Kopf mit Menü (Create Channel/Category, App Settings, Server Settings WIP, Disconnect), Channel-Baum mit Drag & Drop (Einfügelinie, Ghost-Label, Drop in/aus Kategorien), Hover-Zahnrad, „Voice Connected“-Panel (Leave, Camera, Screen), User-Panel unten (Mikro, Kopfhörer, Einstellungen).
- Lobby: Mikro, Kamera und Bildschirm ausgegraut und deaktiviert; laufende Kamera/Freigabe stoppt beim Wechsel in die Lobby.
- Text-Channel-Icon: Sprechblase statt `#`. Nachrichtenliste mit Gruppierung, Composer im Discord-Stil, Statusmeldungen als Toast.
- Channel-Einstellungen: Overview/Permissions/Invites mit WIP-Banner; Name, Kategorie und Löschen funktionieren. Bestätigungsdialog fürs Löschen.
- Einstellungsfenster: Kategorien My Account, Appearance, Voice & Audio, Video, Keybinds, Notifications, Advanced, About. Funktionsfähig u. a.: UI-Skalierung, Zeitstempel, Ausgabelautstärke (neu), Eingabemodus-Karten, PTT-Taste per Tastendruck aufnehmen, Standardkamera, Stream-Qualität. Platzhalter mit WIP-Badge.
- Debug-Builds: `RUSTTALK_DEV_AUTOCONNECT`, `RUSTTALK_DEV_OPEN`, `RUSTTALK_DEV_TITLE` für automatisierte Screenshots.

**Tests:** Client 50 ✅, Server 32 ✅ (neu: Reihenfolge, Verschieberegeln, Neustart, kein Duplikat nach Umbenennen, DB vor Config), Integration 7 ✅, Shared 12 ✅ – Windows und Linux-Server.
**Visuell geprüft** (lokaler Testserver + Bots, Screenshots): Voice-Bühne mit Sprechanzeige, Lobby, Text-Chat, alle Einstellungsseiten, Channel-Einstellungen, „Create Channel“, Verbindungsbildschirm.
**Nicht geprüft:** Drag & Drop mit echter Maus (Logik serverseitig getestet, UI-Interaktion nicht automatisiert).

---

## 2026-11-10 (13) – Voice-Qualität (kompatibel mit schlechten Mikros)

**Analyse der Kette** (Mikro → Mono → Resampler → RNNoise → Gate → Opus → UDP → Jitter → Opus → Mix → Ausgabe), Schwachstellen:
- Aufnahme-Resampler war **linear ohne Tiefpass** → Aliasing/Spiegelfrequenzen bei 16-kHz-Headsets (Bluetooth), 44,1 und 96 kHz (metallischer/kratziger Klang).
- Opus: `inband_fec` an, aber ohne `packet_loss_perc` erzeugt Opus **keine** FEC-Daten → verlorene Pakete wurden nur geschätzt (PLC).
- Kein Pegelausgleich: leise Mikros kommen leise an, laute verzerren; Mixer schnitt mehrere laute Sprecher hart ab.

**Änderungen:**
- Neuer Resampler: gefensterter Sinc (Blackman, Polyphasen-Tabelle), DC-normiert, Latenz < 1 ms.
- DC-Blocker vor RNNoise (Gleichanteil billiger Mikros).
- **Automatische Lautstärke** (AGC, Ziel −20 dBFS, −12…+16 dB, passt sich nur bei erkannter Sprache an, runter schnell/hoch langsam) + weicher Limiter; Einstellung „Automatische Lautstärke", Standard an.
- Opus: 64 kbit/s, Komplexität 10, Signal „Voice", erwarteter Paketverlust 10 % (aktiviert FEC). Bleibt im Hybrid-Modus (SILK+CELT).
- Empfang: letztes fehlendes Frame vor einem Paket wird aus dessen FEC rekonstruiert (davor PLC).
- Mixer: weicher Limiter statt Hard-Clip.
- RNNoise-Sprachwahrscheinlichkeit = Maximum beider 10-ms-Hälften (vorher nur die zweite).

**Tests:** Client 48 ✅ (neu u. a. `downsampling_suppresses_aliasing` > 40 dB, `upsampling_suppresses_images`, `passband_tone_keeps_amplitude`, AGC hebt leises Mikro/zähmt lautes/ignoriert Rauschen, `fec_recovers_lost_packets_better_than_plc`), Server 27 ✅, Integration 7 ✅, Shared 12 ✅.
**FEC-Messung** mit echter Sprache (Windows-Sprachsynthese, `Client/Windows/testdata/speech16k.wav`): Fehler eines verlorenen Pakets bei 64 kbit/s FEC 15,8 vs. PLC 114 (~7× besser). Mit synthetischen Tönen erzeugt SILK keine FEC (VAD erkennt keine Sprache) – deshalb echte Sprachprobe als Testdaten.

**Kompatibilität:** kein Protokollwechsel (Server unverändert, v5). Ältere Clients hören die neuen ganz normal.

**Manuell zu prüfen:** Klang mit verschiedenen Mikros (Headset, Bluetooth, Laptop), AGC-Verhalten bei leisen/lauten Sprechern.

---

## 2026-11-10 (12) – Verbindungsabbruch, hängende Server-Verbindung, Auflösung

**Feedback:** Gute Bildrate, aber Auflösung mau; ein Fehler mit Neuverbindung, alte Verbindung blieb auf dem Server offen.

**Ursachen:**
- **Fehler/Neuverbindung (Client):** `read_frame` lief als Zweig im `tokio::select!` und ist nicht abbruchsicher. Bei jedem ausgehenden Befehl (25 fps eigenes Video) wurde ein halb gelesener eingehender Frame verworfen → Stream verschoben → „Netzwerkfehler". Fix: Lesen in eigenem Task. Regressionstest `net::tests::large_frames_in_both_directions_stay_intact` (150 × 180 KB in beide Richtungen); **Gegenprobe mit altem Ablauf schlägt fehl** („Netzwerkfehler: unexpected end of file"), neuer Ablauf grün.
- **Hängende Verbindung (Server):** Server-Log zeigte die alte Verbindung von 10.69.43.53 weiter als ESTABLISHED, 245 KB unbestätigt, 9 Retransmits; der Server merkte das nicht (wartete nur aufs Lesen). Fix: TCP-Keepalive (15 s/5 s/3), `TCP_USER_TIMEOUT` 20 s (Linux), Schreib-Timeout 15 s, Sitzung endet auch, wenn der Writer scheitert. Verifiziert: `ss -o` zeigt `timer:(keepalive,…)`.
- **Speicher/Latenz:** Video-Frames landeten pro Zuschauer in einer unbegrenzten Queue (auch für tote Clients). Jetzt max. 3 MB Rückstand pro Zuschauer, darüber werden Frames verworfen; Client-Upload ebenso begrenzt.
- **Auflösung:** Bildschirm war auf 1280×720 / JPEG-Qualität 60 begrenzt (3440×1440 → 1280×536, im Vollbild 2,7× hochskaliert). Neu: Einstellung **Video-Qualität** – „Hoch" (Standard: Bildschirm 1920×1080 q75, Kamera 1280×720 q80) oder „Ausgewogen" (720p/540p q70). Kodierung in einem Encoder-Pool (bis 3 Threads, Reihenfolge bleibt).

**Messung (Release, Monitor 3440×1440):** 1080p-Kodierung 64 ms/Bild, 1280er 36 ms. „Hoch" → **18,7 fps**, ~290 KB/Frame (≈ 5,4 MB/s pro Zuschauer); Grenze ist jetzt die GDI-Aufnahme des 5-MP-Monitors. „Ausgewogen" → ~25 fps. Kleinere Monitore/Fenster sind schneller.

**Tests:** Client 39 ✅ (neu: Regressionstest Netz, `encoder_pool_keeps_order_and_shuts_down`, `quality_levels_scale_up`), Server 27 ✅, Integration 7 ✅, Shared 12 ✅ – unter Windows und auf dem Linux-Server.

**Server 10.69.40.101:** neu gebaut, installiert (vorheriges Binary als `rusttalk-server.v5a.bak`), Dienst `active`, `test_client` → `WELCOME`. Protokoll bleibt v5 (Clients aus Eintrag 11 verbinden weiter, haben aber den Lese-Bug).

---

## 2026-11-10 (11) – Kamera + Bildschirm gleichzeitig, 25 fps, Vollbild (Protokoll v5)

**Feedback:** Webcam läuft auf dem zweiten Testgerät. Wünsche: Vollbild mit User-Leiste, Kamera und Bildschirm gleichzeitig, weniger Lag (Ziel 25 fps).

**Änderungen:**
- **Protokoll v5:** `VideoKind { Camera, Screen }` an `VideoFrame`; neu `StopVideo` / `VideoStopped` zum Beenden eines einzelnen Streams. Server relayed die Art mit. Client und Server müssen beide v5 sein.
- **Parallel teilen:** Kamera und Bildschirm stoppen sich nicht mehr gegenseitig; pro User bis zu zwei Kacheln (Icon zeigt die Art).
- **Lag:** (1) UI zeichnete nur alle 100 ms neu → jetzt Repaint sofort bei jedem eigenen und empfangenen Frame. (2) 25 fps statt 5 fps (gleichmäßiger Takt, `Pacer`). (3) Kamera fordert 720p MJPEG an, sendet max. 960×540; Bildschirm max. 1280×720. (4) Bildschirm: Aufnahme und Kodierung in getrennten Threads (vorher 13 fps bei 3440×1440, jetzt 25,7 fps).
- **Vollbild:** Klick auf eine Kachel → Fenster im Vollbild, Stream füllt die Fläche, unten die Channel-Mitglieder (grüner Punkt + Ring beim Sprechen, Mute-Icons, Buttons zum Umschalten auf andere Streams). Beenden mit Esc, Doppelklick oder Button.
- Geteiltes Fenster geschlossen → Teilen endet sauber (Button nicht mehr grün).

**Tests (Windows):** Client 36 ✅ (neu: `pacer_yields_about_25_fps_from_30_fps_camera`, `screen_frames_use_larger_size`), Server 27 ✅, Integration 7 ✅ (Video-Test erweitert: Kamera + Bildschirm getrennt, `VideoStopped`), Shared 12 ✅. Hardware (Release): Monitor 3440×1440 → **25,7 fps**, ~113 KB/Frame (≈ 2,8 MB/s pro Zuschauer).

**Server 10.69.40.101:** Quellen nach `/opt/rusttalk/src` übertragen (vorher identisch mit Commit 8ef2f5c), Release-Build + Tests dort grün (27/7/12). Installiert und Dienst neu gestartet (altes Binary als `rusttalk-server.v4.bak`), Dienst `active`, 7 Channels / 12 Log-Einträge / 4 Chat-Nachrichten unverändert vorhanden. `test_client` (v5) bekommt `WELCOME`.

**Manuell zu prüfen:** Vollbild-Ansicht + User-Leiste in der GUI, Kamera + Bildschirm gleichzeitig mit zwei Geräten, gefühlte Flüssigkeit.

---

## 2026-11-10 (10) – Webcam-Fix + Vorschau, Bildschirm/Fenster-Auswahl

**Ursachen:**
- **Webcam:** `nokhwa` wurde ohne Feature `input-msmf` gebaut → auf Windows gab es gar kein Kamera-Backend, jedes Öffnen schlug fehl. Fix: `nokhwa = { features = ["input-msmf"] }`, Format-Fallbacks (640×480 MJPEG → YUYV → NV12 → Default).
- **Screen-Share:** Der alte `scrap`-Code ignorierte den Zeilen-Stride der DXGI-Frames (bei gepaddeten Zeilen ein verzerrtes Bild), nahm fest den Hauptmonitor und kann keine einzelnen Fenster. Ersetzt durch `xcap` (Monitore **und** Fenster).

**Neu:**
- Kamera-Button öffnet einen Dialog mit Geräteauswahl + Live-Vorschau; erst „Kamera teilen" sendet (`VideoCapture::set_sending`). Gewählte Kamera wird in `client_config.toml` (`camera`) gemerkt.
- Bildschirm-Button öffnet eine Auswahl im Discord-Stil: Reiter „Anwendungen" / „Gesamter Bildschirm", Kacheln mit Vorschaubild + Titel/App, Klick wählt, Doppelklick oder „Teilen" startet, „Aktualisieren" lädt neu.
- Buttons grün hinterlegt, solange live geteilt wird; erneuter Klick beendet das Teilen.

**Tests:** 34 Client-Tests grün (neu: `thumbnail_fits_picker_box`, `sending_flag_toggles`, `camera_errors_are_readable`). Hardware-Tests (`#[ignore]`, Aufruf: Test-Binary mit `hw_ --ignored --nocapture`):
- `hw_share_sources_and_window_capture` ✅ – 2 Monitore (3440×1440, 1920×1080) und 16 Fenster (u. a. World of Warcraft, Chrome, Discord) mit Vorschaubild gefunden; Monitor- und Fenster-Capture liefern JPEG-Frames.
- `hw_camera_delivers_frames` ❌ – „Lenovo Performance RGB Camera" wird gefunden und geöffnet, jeder Lesevorgang endet mit `0x800703E3` (ERROR_OPERATION_ABORTED), in allen Formaten. Gegenprobe mit reinem Media Foundation (ohne nokhwa, mit und ohne Video-Processing): identischer Fehler → Geräte-/Systemzustand, nicht RustTalk. Datenschutz-Einstellungen erlauben Kamerazugriff, keine andere App nutzt sie. Der Client zeigt dafür jetzt einen verständlichen Hinweis (Blende/Kamera-Schalter prüfen, neu einstecken).

**Manuell zu prüfen:** Dialoge in der laufenden GUI (Layout, Vorschau-Kacheln), Empfang beim Gegenüber, Webcam nach Neu-Einstecken bzw. mit anderer Kamera.

---

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
