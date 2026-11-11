//! Zentraler Server-State: Clients und Channels komplett in Memory.
//!
//! Alle Methoden sind synchron (kein `.await` im Lock), damit ein
//! einfacher `std::sync::Mutex` reicht.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusttalk_shared::protocol::{
    ChannelInfo, ChannelKind, ChannelPlacement, ChatRecord, LogEntry, ServerMessage, UserInfo,
    ServerSettings, MAX_ICON_BYTES, MAX_NAME_LEN, MAX_SERVER_MESSAGE_LEN, MAX_WELCOME_LEN,
    SPAWN_LOG_CAP, DEFAULT_AFK_NOTICE, fill_template,
};
use tokio::sync::mpsc;

use crate::config::{ChannelKindConfig, ServerConfig};
use crate::db::Db;

/// Max. wartende Video-Bytes pro Zuschauer (~1 s Bildschirm-Stream). Darueber
/// werden Frames fuer diesen Zuschauer verworfen: langsame oder tote
/// Verbindungen bremsen niemanden und fressen keinen Speicher.
pub const MAX_VIDEO_BACKLOG: usize = 3 * 1024 * 1024;

/// Sender name of server announcements in text channels (client id 0).
pub const ANNOUNCER_NAME: &str = "RustTalk";

/// Thread-sicher geteilter State.
pub type SharedState = std::sync::Arc<std::sync::Mutex<State>>;

pub struct Client {
    pub id: u64,
    pub name: String,
    pub channel_id: u64,
    pub muted: bool,
    pub deafened: bool,
    /// Teilt gerade Kamera/Bildschirm.
    pub sharing: bool,
    pub udp_token: u64,
    pub udp_addr: Option<SocketAddr>,
    /// Seit wann stumm (fuer den AFK-Mover). None = nicht stumm.
    pub muted_since: Option<Instant>,
    /// Video-Bytes, die fuer diesen Client in `tx` warten (Writer zieht ab).
    /// Ueber `MAX_VIDEO_BACKLOG` werden Frames verworfen statt gestaut.
    pub video_backlog: Arc<AtomicUsize>,
    /// Eingehende Nachrichten für diesen Client (vom Writer-Task gesendet).
    pub tx: mpsc::UnboundedSender<ServerMessage>,
}

impl Client {
    pub fn info(&self) -> UserInfo {
        UserInfo {
            id: self.id,
            name: self.name.clone(),
            channel_id: self.channel_id,
            muted: self.muted,
            deafened: self.deafened,
            sharing: self.sharing,
        }
    }
}

#[derive(Debug)]
pub struct Channel {
    pub id: u64,
    pub name: String,
    pub kind: ChannelKind,
    pub parent: Option<u64>,
    /// Sortierposition unter Geschwistern (gleicher `parent`).
    pub position: u32,
}

pub struct State {
    clients: HashMap<u64, Client>,
    channels: HashMap<u64, Channel>,
    token_index: HashMap<u64, u64>,
    next_client_id: u64,
    next_channel_id: u64,
    /// Der eine Spawn-Channel (immer vorhanden, nie loeschbar).
    spawn_channel_id: u64,
    spawn_log: VecDeque<LogEntry>,
    max_clients: usize,
    password: Option<String>,
    /// Servereinstellungen (Name, Icon, Willkommenstext, AFK).
    settings: ServerSettings,
    /// SQLite-Persistenz (None = alles nur in Memory).
    db: Option<Db>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl State {
    /// Startzustand.
    ///
    /// Channels: Mit Persistenz ist die DB die Quelle der Wahrheit (IDs,
    /// Namen, Kategorien, Reihenfolge bleiben exakt erhalten - Chat-Verlauf
    /// haengt an den IDs). Die Config-Channels legen nur beim allerersten
    /// Start (leere DB) die Grundstruktur an. Frueher wurde die Config bei
    /// jedem Start neu angelegt und per Name mit der DB abgeglichen -
    /// umbenannte Config-Channels tauchten dann doppelt auf.
    pub fn new(cfg: &ServerConfig) -> anyhow::Result<Self> {
        let mut state = State {
            clients: HashMap::new(),
            channels: HashMap::new(),
            token_index: HashMap::new(),
            next_client_id: 1,
            next_channel_id: 1,
            spawn_channel_id: 1,
            spawn_log: VecDeque::new(),
            max_clients: cfg.max_clients,
            password: cfg.password.clone(),
            settings: ServerSettings { name: cfg.server_name.clone(), ..ServerSettings::default() },
            db: None,
        };

        let spawn_entries: Vec<_> =
            cfg.channels.iter().filter(|c| c.kind == ChannelKindConfig::Spawn).collect();
        if spawn_entries.len() > 1 {
            anyhow::bail!("Config-Fehler: mehr als ein Spawn-Channel definiert");
        }

        let db = if cfg.persist_state {
            Some(
                Db::open(&cfg.db_path)
                    .map_err(|e| anyhow::anyhow!("Persistenz ({}): {e:#}", cfg.db_path))?,
            )
        } else {
            None
        };
        let stored = match &db {
            Some(db) => db.load_channels()?,
            None => Vec::new(),
        };

        if stored.is_empty() {
            state.seed_from_config(cfg)?;
            // Erstbefuellung sofort speichern -> ab jetzt ist die DB massgeblich.
            if let Some(db) = &db {
                for ch in state.channels.values() {
                    db.upsert_channel(ch.id, &ch.name, ch.kind, ch.parent, ch.position)?;
                }
            }
        } else {
            for s in &stored {
                state.channels.insert(
                    s.id,
                    Channel {
                        id: s.id,
                        name: s.name.clone(),
                        kind: s.kind,
                        parent: s.parent,
                        position: s.position,
                    },
                );
            }
            state.next_channel_id = stored.iter().map(|s| s.id).max().unwrap_or(0) + 1;
            // Verwaiste Parents (Kategorie fehlt) -> oberste Ebene.
            let ids: std::collections::HashSet<u64> = state.channels.keys().copied().collect();
            for ch in state.channels.values_mut() {
                if ch.parent.is_some_and(|p| !ids.contains(&p)) {
                    ch.parent = None;
                }
            }
            // Genau ein Spawn: fehlt er (alte DB), aus der Config anlegen.
            let mut spawns: Vec<u64> = state
                .channels
                .values()
                .filter(|c| c.kind == ChannelKind::Spawn)
                .map(|c| c.id)
                .collect();
            spawns.sort_unstable();
            match spawns.first() {
                Some(&id) => state.spawn_channel_id = id,
                None => {
                    let name = spawn_entries
                        .first()
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| "Lobby".to_string());
                    let id = state.insert_channel(name, ChannelKind::Spawn, None);
                    state.spawn_channel_id = id;
                    if let (Some(db), Some(ch)) = (&db, state.channels.get(&id)) {
                        db.upsert_channel(ch.id, &ch.name, ch.kind, ch.parent, ch.position)?;
                    }
                }
            }
        }

        if let Some(db) = db {
            // Gespeicherte Servereinstellungen haben Vorrang vor der Config.
            if let Some(saved) = db.load_settings()? {
                state.settings = saved;
            }
            if state.settings.afk_channel.is_some_and(|id| !state.is_afk_target(id)) {
                state.settings.afk_channel = None;
            }
            if state.settings.announce_channel.is_some_and(|id| !state.is_text_channel(id)) {
                state.settings.announce_channel = None;
            }
            for entry in db.load_events()? {
                if state.spawn_log.len() >= SPAWN_LOG_CAP {
                    state.spawn_log.pop_front();
                }
                state.spawn_log.push_back(entry);
            }
            tracing::info!(
                "Persistenz aktiv: {} ({} Channels, {} Log-Eintraege geladen)",
                cfg.db_path,
                state.channels.len(),
                state.spawn_log.len()
            );
            state.db = Some(db);
        }
        Ok(state)
    }

    /// Grundstruktur aus der Config (Spawn zuerst, dann Kategorien, dann
    /// Channels - IDs und Positionen in Config-Reihenfolge).
    fn seed_from_config(&mut self, cfg: &ServerConfig) -> anyhow::Result<()> {
        let spawn_name = cfg
            .channels
            .iter()
            .find(|c| c.kind == ChannelKindConfig::Spawn)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "Lobby".to_string());
        self.spawn_channel_id = self.insert_channel(spawn_name, ChannelKind::Spawn, None);

        let mut category_ids: HashMap<String, u64> = HashMap::new();
        for ch in cfg.channels.iter().filter(|c| c.kind == ChannelKindConfig::Category) {
            let id = self.insert_channel(ch.name.clone(), ChannelKind::Category, None);
            category_ids.insert(ch.name.clone(), id);
        }
        for ch in cfg
            .channels
            .iter()
            .filter(|c| !matches!(c.kind, ChannelKindConfig::Spawn | ChannelKindConfig::Category))
        {
            let parent = match ch.category.as_deref() {
                Some(name) => Some(*category_ids.get(name).ok_or_else(|| {
                    anyhow::anyhow!("Config-Fehler: unbekannte Kategorie '{name}'")
                })?),
                None => None,
            };
            self.insert_channel(ch.name.clone(), ch.kind.into(), parent);
        }
        Ok(())
    }

    /// Neuer Channel ans Ende seiner Geschwister (Spawn: Position 0).
    fn insert_channel(&mut self, name: String, kind: ChannelKind, parent: Option<u64>) -> u64 {
        let id = self.next_channel_id;
        self.next_channel_id += 1;
        let position = if kind == ChannelKind::Spawn {
            0
        } else {
            self.channels
                .values()
                .filter(|c| c.parent == parent && c.kind != ChannelKind::Spawn)
                .map(|c| c.position + 1)
                .max()
                .unwrap_or(1)
                .max(1)
        };
        self.channels.insert(id, Channel { id, name, kind, parent, position });
        id
    }

    pub fn settings(&self) -> &ServerSettings {
        &self.settings
    }

    /// Kann ein Channel AFK-Ziel sein? (Voice oder Lobby)
    fn is_afk_target(&self, id: u64) -> bool {
        self.channels
            .get(&id)
            .is_some_and(|c| matches!(c.kind, ChannelKind::Voice | ChannelKind::Spawn))
    }

    fn is_text_channel(&self, id: u64) -> bool {
        self.channels.get(&id).is_some_and(|c| c.kind == ChannelKind::Text)
    }

    /// Neue Servereinstellungen pruefen, uebernehmen und speichern.
    pub fn update_settings(&mut self, new: ServerSettings) -> Result<ServerSettings, String> {
        let name = validate_name(&new.name)?;
        if new.icon.as_ref().is_some_and(|i| i.len() > MAX_ICON_BYTES) {
            return Err(format!("Server icon too large (max. {} KiB)", MAX_ICON_BYTES / 1024));
        }
        let welcome = new.welcome_message.trim().to_string();
        if welcome.chars().count() > MAX_WELCOME_LEN {
            return Err(format!("Welcome message too long (max. {MAX_WELCOME_LEN} characters)"));
        }
        if !(1..=240).contains(&new.afk_timeout_mins) {
            return Err("AFK timeout must be between 1 and 240 minutes".to_string());
        }
        if let Some(id) = new.afk_channel {
            if !self.is_afk_target(id) {
                return Err("AFK channel must be a voice channel or the lobby".to_string());
            }
        }
        if let Some(id) = new.announce_channel {
            if !self.is_text_channel(id) {
                return Err("Announcement channel must be a text channel".to_string());
            }
        }
        let short = |label: &str, text: &str| -> Result<String, String> {
            let t = text.trim().to_string();
            if t.chars().count() > MAX_SERVER_MESSAGE_LEN {
                return Err(format!("{label} too long (max. {MAX_SERVER_MESSAGE_LEN} characters)"));
            }
            Ok(t)
        };
        let join_message = short("Join message", &new.join_message)?;
        let leave_message = short("Leave message", &new.leave_message)?;
        let afk_notice = short("AFK notice", &new.afk_notice)?;
        let motd = short("Message of the day", &new.motd)?;
        let rules = new.rules.trim().to_string();
        if rules.chars().count() > MAX_WELCOME_LEN {
            return Err(format!("Rules too long (max. {MAX_WELCOME_LEN} characters)"));
        }
        self.settings = ServerSettings {
            name,
            welcome_message: welcome,
            join_message,
            leave_message,
            afk_notice,
            motd,
            rules,
            ..new
        };
        if let Some(db) = &self.db {
            if let Err(e) = db.save_settings(&self.settings) {
                tracing::warn!("Einstellungen speichern fehlgeschlagen: {e}");
            }
        }
        Ok(self.settings.clone())
    }

    /// Join/leave announcement: builds the chat message for the announcement
    /// channel (and persists it). None if no channel is set or the template is empty.
    /// `template` is `settings.join_message` or `settings.leave_message`.
    pub fn announcement(&self, template: &str, username: &str) -> Option<ServerMessage> {
        let channel_id = self.settings.announce_channel.filter(|id| self.is_text_channel(*id))?;
        if template.trim().is_empty() {
            return None;
        }
        let text = fill_template(template, &[("user", username)]);
        let timestamp = now_secs();
        self.persist_message(channel_id, ANNOUNCER_NAME, &text, timestamp, None);
        Some(ServerMessage::Chat {
            channel_id,
            from_id: 0,
            from_name: ANNOUNCER_NAME.to_string(),
            text,
            timestamp,
            image: None,
        })
    }

    /// Text for a user moved by the AFK mover (template or default).
    pub fn afk_notice(&self, channel_name: &str) -> String {
        let template = if self.settings.afk_notice.trim().is_empty() {
            DEFAULT_AFK_NOTICE
        } else {
            &self.settings.afk_notice
        };
        let mins = self.settings.afk_timeout_mins.to_string();
        fill_template(template, &[("channel", channel_name), ("minutes", &mins)])
    }

    /// Auto-AFK: Clients in Voice-Channels, die laenger als das Limit stumm
    /// sind, ins AFK-Ziel verschieben. Rueckgabe: (Client, Ziel-Channel).
    pub fn afk_sweep(&mut self, now: Instant) -> Vec<(u64, u64)> {
        if !self.settings.afk_enabled {
            return Vec::new();
        }
        let target = self
            .settings
            .afk_channel
            .filter(|id| self.is_afk_target(*id))
            .unwrap_or(self.spawn_channel_id);
        let limit = Duration::from_secs(u64::from(self.settings.afk_timeout_mins) * 60);
        let voice: std::collections::HashSet<u64> = self
            .channels
            .values()
            .filter(|c| c.kind == ChannelKind::Voice)
            .map(|c| c.id)
            .collect();
        let mut moved = Vec::new();
        for c in self.clients.values_mut() {
            let idle = c.muted_since.is_some_and(|t| now.saturating_duration_since(t) >= limit);
            if idle && voice.contains(&c.channel_id) && c.channel_id != target {
                c.channel_id = target;
                moved.push((c.id, target));
            }
        }
        moved
    }

    /// Geschwister (gleicher Parent, ohne Spawn) in Anzeige-Reihenfolge.
    fn sorted_siblings(&self, parent: Option<u64>) -> Vec<u64> {
        let mut sib: Vec<&Channel> = self
            .channels
            .values()
            .filter(|c| c.parent == parent && c.kind != ChannelKind::Spawn)
            .collect();
        sib.sort_by_key(|c| (c.position, c.id));
        sib.into_iter().map(|c| c.id).collect()
    }

    /// Verschiebt einen Channel (Drag & Drop) an `index` unter `parent`.
    /// Regeln: Spawn bleibt oben fest; Kategorien nur auf oberster Ebene;
    /// Text/Voice oben oder in eine Kategorie. Nummeriert die betroffenen
    /// Geschwister neu und speichert sie.
    pub fn move_channel(
        &mut self,
        channel_id: u64,
        parent: Option<u64>,
        index: u32,
    ) -> Result<Vec<ChannelPlacement>, String> {
        let ch = self.channels.get(&channel_id).ok_or("Channel does not exist")?;
        if ch.kind == ChannelKind::Spawn {
            return Err("The lobby always stays at the top".to_string());
        }
        if ch.kind == ChannelKind::Category && parent.is_some() {
            return Err("Categories cannot be nested".to_string());
        }
        if let Some(p) = parent {
            match self.channels.get(&p) {
                Some(pc) if pc.kind == ChannelKind::Category => {}
                Some(_) => return Err("Target is not a category".to_string()),
                None => return Err("Target category does not exist".to_string()),
            }
        }
        let old_parent = ch.parent;

        let mut siblings: Vec<u64> =
            self.sorted_siblings(parent).into_iter().filter(|&id| id != channel_id).collect();
        let at = (index as usize).min(siblings.len());
        siblings.insert(at, channel_id);

        if let Some(c) = self.channels.get_mut(&channel_id) {
            c.parent = parent;
        }
        let mut touched: Vec<u64> = Vec::new();
        for (i, id) in siblings.iter().enumerate() {
            if let Some(c) = self.channels.get_mut(id) {
                c.position = i as u32 + 1;
                touched.push(*id);
            }
        }
        if old_parent != parent {
            for (i, id) in self.sorted_siblings(old_parent).iter().enumerate() {
                if let Some(c) = self.channels.get_mut(id) {
                    c.position = i as u32 + 1;
                    touched.push(*id);
                }
            }
        }
        if let Some(db) = &self.db {
            for id in &touched {
                if let Some(c) = self.channels.get(id) {
                    if let Err(e) = db.upsert_channel(c.id, &c.name, c.kind, c.parent, c.position) {
                        tracing::warn!("Channel-Reihenfolge speichern fehlgeschlagen: {e}");
                    }
                }
            }
        }
        Ok(self.all_placements())
    }

    /// Ort aller Channels (fuer `ChannelsReordered`).
    pub fn all_placements(&self) -> Vec<ChannelPlacement> {
        let mut p: Vec<ChannelPlacement> = self
            .channels
            .values()
            .map(|c| ChannelPlacement { id: c.id, parent: c.parent, position: c.position })
            .collect();
        p.sort_by_key(|x| x.id);
        p
    }

    pub fn spawn_channel_id(&self) -> u64 {
        self.spawn_channel_id
    }

    /// Vorschau auf die nächste Client-ID (für Fallback-Usernamen).
    pub fn next_client_id_hint(&self) -> u64 {
        self.next_client_id
    }

    pub fn spawn_log(&self) -> Vec<LogEntry> {
        self.spawn_log.iter().cloned().collect()
    }

    /// Neuer Eintrag im Join-Log (Cap: SPAWN_LOG_CAP) + Persistenz.
    pub fn push_spawn_log(&mut self, text: String) -> LogEntry {
        let entry = LogEntry { timestamp: now_secs(), text };
        if self.spawn_log.len() >= SPAWN_LOG_CAP {
            self.spawn_log.pop_front();
        }
        if let Some(db) = &self.db {
            if let Err(e) = db.log_event(&entry) {
                tracing::warn!("Event-Persistenz fehlgeschlagen: {e}");
            }
        }
        self.spawn_log.push_back(entry.clone());
        entry
    }

    /// Speichert eine Chat-Nachricht (no-op ohne Persistenz).
    pub fn persist_message(
        &self,
        channel_id: u64,
        from_name: &str,
        text: &str,
        timestamp: u64,
        image: Option<&[u8]>,
    ) {
        if let Some(db) = &self.db {
            if let Err(e) = db.add_message(channel_id, from_name, text, timestamp, image) {
                tracing::warn!("Chat-Persistenz fehlgeschlagen: {e}");
            }
        }
    }

    /// Letzte Nachrichten aller Text-Channels (fuer Welcome). Leer ohne Persistenz.
    pub fn chat_history(&self) -> Vec<ChatRecord> {
        match &self.db {
            Some(db) => db.recent_messages().unwrap_or_default(),
            None => Vec::new(),
        }
    }

    pub fn check_password(&self, provided: &Option<String>) -> Result<(), &'static str> {
        match &self.password {
            Some(expected) if provided.as_deref() != Some(expected.as_str()) => {
                Err("Wrong or missing server password")
            }
            _ => Ok(()),
        }
    }

    pub fn is_full(&self) -> bool {
        self.clients.len() >= self.max_clients
    }

    /// Registriert einen neuen Client und bringt ihn in den Spawn-Channel.
    /// Gibt (client_id, udp_token) zurück.
    pub fn add_client(&mut self, name: String, tx: mpsc::UnboundedSender<ServerMessage>) -> (u64, u64) {
        let id = self.next_client_id;
        self.next_client_id += 1;
        let mut token = rand::random::<u64>();
        while self.token_index.contains_key(&token) {
            token = rand::random::<u64>();
        }
        self.token_index.insert(token, id);
        self.clients.insert(
            id,
            Client {
                id,
                name,
                channel_id: self.spawn_channel_id,
                muted: false,
                deafened: false,
                sharing: false,
                udp_token: token,
                udp_addr: None,
                muted_since: None,
                video_backlog: Arc::new(AtomicUsize::new(0)),
                tx,
            },
        );
        (id, token)
    }

    pub fn remove_client(&mut self, id: u64) -> Option<Client> {
        let client = self.clients.remove(&id)?;
        self.token_index.remove(&client.udp_token);
        Some(client)
    }

    pub fn client(&self, id: u64) -> Option<&Client> {
        self.clients.get(&id)
    }

    pub fn channel(&self, id: u64) -> Option<&Channel> {
        self.channels.get(&id)
    }

    pub fn channel_exists(&self, id: u64) -> bool {
        self.channels.contains_key(&id)
    }

    pub fn channel_info(&self, id: u64) -> Option<ChannelInfo> {
        let ch = self.channels.get(&id)?;
        let mut user_ids: Vec<u64> = self
            .clients
            .values()
            .filter(|c| c.channel_id == id)
            .map(|c| c.id)
            .collect();
        user_ids.sort_unstable();
        Some(ChannelInfo {
            id: ch.id,
            name: ch.name.clone(),
            kind: ch.kind,
            parent: ch.parent,
            position: ch.position,
            user_ids,
        })
    }

    pub fn all_channels(&self) -> Vec<ChannelInfo> {
        let mut infos: Vec<ChannelInfo> =
            self.channels.keys().filter_map(|id| self.channel_info(*id)).collect();
        infos.sort_by_key(|c| c.id);
        infos
    }

    pub fn all_users(&self) -> Vec<UserInfo> {
        let mut users: Vec<UserInfo> = self.clients.values().map(Client::info).collect();
        users.sort_by_key(|u| u.id);
        users
    }

    /// Erstellt einen Channel. Validierung: kein zweiter Spawn,
    /// Parent muss eine Kategorie sein.
    pub fn create_channel(
        &mut self,
        name: &str,
        kind: ChannelKind,
        parent: Option<u64>,
    ) -> Result<ChannelInfo, String> {
        let name = validate_name(name)?;
        if kind == ChannelKind::Spawn {
            return Err("There is already a lobby channel".to_string());
        }
        if let Some(parent_id) = parent {
            match self.channels.get(&parent_id) {
                Some(p) if p.kind == ChannelKind::Category => {}
                Some(_) => return Err("Parent is not a category".to_string()),
                None => return Err("Parent category does not exist".to_string()),
            }
        }
        let id = self.insert_channel(name.clone(), kind, parent);
        let info = self.channel_info(id).expect("frisch erstellter Channel");
        if let Some(db) = &self.db {
            if let Err(e) = db.upsert_channel(info.id, &info.name, info.kind, info.parent, info.position) {
                tracing::warn!("Channel-Persistenz fehlgeschlagen: {e}");
            }
        }
        Ok(info)
    }

    /// Löscht einen Channel. Mitglieder (Voice) wandern in den Spawn-Channel,
    /// Kinder einer Kategorie werden Top-Level. Gibt verschobene Client-IDs zurück.
    pub fn delete_channel(&mut self, id: u64) -> Result<Vec<u64>, String> {
        let channel = self.channels.get(&id).ok_or("Channel does not exist")?;
        if channel.kind == ChannelKind::Spawn {
            return Err("The lobby cannot be deleted".to_string());
        }
        let was_category = channel.kind == ChannelKind::Category;
        self.channels.remove(&id);
        let afk_gone = self.settings.afk_channel == Some(id);
        let announce_gone = self.settings.announce_channel == Some(id);
        if afk_gone {
            self.settings.afk_channel = None;
        }
        if announce_gone {
            self.settings.announce_channel = None;
        }
        if afk_gone || announce_gone {
            if let Some(db) = &self.db {
                let _ = db.save_settings(&self.settings);
            }
        }
        if was_category {
            for ch in self.channels.values_mut() {
                if ch.parent == Some(id) {
                    ch.parent = None;
                }
            }
        }
        let spawn_id = self.spawn_channel_id;
        let mut moved = Vec::new();
        for client in self.clients.values_mut() {
            if client.channel_id == id {
                client.channel_id = spawn_id;
                moved.push(client.id);
            }
        }
        if let Some(db) = &self.db {
            if let Err(e) = db.delete_channel(id) {
                tracing::warn!("Channel-Loesch-Persistenz fehlgeschlagen: {e}");
            }
        }
        Ok(moved)
    }

    pub fn rename_channel(&mut self, id: u64, name: &str) -> Result<(), String> {
        let name = validate_name(name)?;
        let channel = self.channels.get_mut(&id).ok_or("Channel does not exist")?;
        channel.name = name;
        if let Some(db) = &self.db {
            if let Err(e) = db.upsert_channel(
                channel.id,
                &channel.name,
                channel.kind,
                channel.parent,
                channel.position,
            ) {
                tracing::warn!("Channel-Rename-Persistenz fehlgeschlagen: {e}");
            }
        }
        Ok(())
    }

    /// Verschiebt einen Client. Ziel muss join-bar sein (Spawn oder Voice).
    pub fn move_client(&mut self, client_id: u64, channel_id: u64) -> Result<(), String> {
        let channel = self.channels.get(&channel_id).ok_or("Channel does not exist")?;
        if !channel.kind.can_join() {
            return Err(match channel.kind {
                ChannelKind::Category => "You cannot join a category".to_string(),
                ChannelKind::Text => "Text channels are for chatting only".to_string(),
                _ => "You cannot join this channel".to_string(),
            });
        }
        let client = self.clients.get_mut(&client_id).ok_or("Client does not exist")?;
        client.channel_id = channel_id;
        Ok(())
    }

    pub fn set_mute_state(&mut self, client_id: u64, muted: bool, deafened: bool) {
        if let Some(client) = self.clients.get_mut(&client_id) {
            let now_muted = muted || deafened;
            if now_muted && client.muted_since.is_none() {
                client.muted_since = Some(Instant::now());
            } else if !now_muted {
                client.muted_since = None;
            }
            client.muted = muted;
            client.deafened = deafened;
        }
    }

    /// Kamera/Screen-Sharing-Status setzen. Gibt false zurück, wenn der
    /// Client nicht in einem Voice-Channel ist (Sharing nur dort).
    pub fn set_sharing(&mut self, client_id: u64, sharing: bool) -> bool {
        let Some(client) = self.clients.get_mut(&client_id) else { return false };
        let in_voice = self
            .channels
            .get(&client.channel_id)
            .is_some_and(|ch| ch.kind == ChannelKind::Voice);
        if sharing && !in_voice {
            return false;
        }
        client.sharing = sharing;
        true
    }

    /// Relay-Ziele fuer Video-Frames: Mitglieder desselben Voice-Channels
    /// (ausser dem Sender selbst). Leer, wenn der Sender nicht teilt.
    pub fn video_targets(
        &self,
        client_id: u64,
    ) -> Vec<(mpsc::UnboundedSender<ServerMessage>, Arc<AtomicUsize>)> {
        let Some(sender) = self.clients.get(&client_id) else { return Vec::new() };
        if !sender.sharing {
            return Vec::new();
        }
        let in_voice = self
            .channels
            .get(&sender.channel_id)
            .is_some_and(|ch| ch.kind == ChannelKind::Voice);
        if !in_voice {
            return Vec::new();
        }
        self.clients
            .values()
            .filter(|c| c.id != client_id && c.channel_id == sender.channel_id)
            .map(|c| (c.tx.clone(), c.video_backlog.clone()))
            .collect()
    }

    /// Darf in diesem Channel gechattet werden? (Text + Voice-Media-Feed)
    pub fn chat_allowed(&self, channel_id: u64) -> Result<(), String> {
        match self.channels.get(&channel_id) {
            None => Err("Channel does not exist".to_string()),
            Some(ch) if !ch.kind.has_chat() => Err(match ch.kind {
                ChannelKind::Spawn => "The lobby is a join log only".to_string(),
                ChannelKind::Category => "Categories have no chat".to_string(),
                _ => "You cannot write here".to_string(),
            }),
            Some(_) => Ok(()),
        }
    }

    /// Sendet eine Nachricht an alle verbundenen Clients.
    pub fn broadcast(&self, msg: &ServerMessage) {
        for client in self.clients.values() {
            let _ = client.tx.send(msg.clone());
        }
    }

    pub fn send_to(&self, client_id: u64, msg: &ServerMessage) {
        if let Some(client) = self.clients.get(&client_id) {
            let _ = client.tx.send(msg.clone());
        }
    }

    /// Verarbeitet ein eingehendes UDP-Audiopaket:
    /// - lernt/aktualisiert die UDP-Adresse des Sprechers (Registrierung)
    /// - liefert (speaker_id, Relay-Zieladressen)
    ///
    /// Audio wird NUR gerelayed, wenn der Sprecher in einem Voice-Channel ist
    /// (Spawn = Sprechverbot). Ziele: alle anderen Clients im selben Channel,
    /// die nicht „taub" sind und bereits eine UDP-Adresse haben.
    pub fn udp_targets(&mut self, token: u64, from: SocketAddr) -> Option<(u64, Vec<SocketAddr>)> {
        let speaker_id = *self.token_index.get(&token)?;
        let channel_id = {
            let speaker = self.clients.get_mut(&speaker_id)?;
            if speaker.udp_addr != Some(from) {
                speaker.udp_addr = Some(from);
            }
            speaker.channel_id
        };
        if !self.channels.get(&channel_id).is_some_and(|ch| ch.kind.has_voice()) {
            return Some((speaker_id, Vec::new())); // Sprechverbot (z. B. Spawn)
        }
        let targets = self
            .clients
            .values()
            .filter(|c| c.id != speaker_id && c.channel_id == channel_id && !c.deafened)
            .filter_map(|c| c.udp_addr)
            .collect();
        Some((speaker_id, targets))
    }
}

fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Name must not be empty".to_string());
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(format!("Name too long (max. {MAX_NAME_LEN} characters)"));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> State {
        let cfg: ServerConfig = toml::from_str(
            r#"
server_name = "Test"

[[channels]]
name = "Spawn"
kind = "spawn"

[[channels]]
name = "Talk"
kind = "category"

[[channels]]
name = "Funkturm"
kind = "voice"
category = "Talk"

[[channels]]
name = "quasselbude"
kind = "text"
category = "Talk"
"#,
        )
        .unwrap();
        State::new(&cfg).unwrap()
    }

    fn add_test_client(state: &mut State, name: &str) -> (u64, u64) {
        let (tx, _rx) = mpsc::unbounded_channel();
        state.add_client(name.to_string(), tx)
    }

    #[test]
    fn spawn_channel_is_id_1_and_clients_land_there() {
        let mut state = test_state();
        assert_eq!(state.spawn_channel_id(), 1);
        let (id, _) = add_test_client(&mut state, "alice");
        let info = state.channel_info(1).unwrap();
        assert_eq!(info.kind, ChannelKind::Spawn);
        assert_eq!(info.user_ids, vec![id]);
    }

    #[test]
    fn config_without_spawn_gets_one_automatically() {
        let cfg: ServerConfig = toml::from_str("").unwrap();
        let state = State::new(&cfg).unwrap();
        let spawn = state.channel_info(state.spawn_channel_id()).unwrap();
        assert_eq!(spawn.kind, ChannelKind::Spawn);
        assert_eq!(spawn.name, "Lobby");
    }

    #[test]
    fn two_spawn_channels_are_rejected() {
        let cfg: ServerConfig = toml::from_str(
            r#"
[[channels]]
name = "A"
kind = "spawn"
[[channels]]
name = "B"
kind = "spawn"
"#,
        )
        .unwrap();
        assert!(State::new(&cfg).is_err());
    }

    #[test]
    fn categories_group_channels() {
        let state = test_state();
        let funkturm = state.all_channels().into_iter().find(|c| c.name == "Funkturm").unwrap();
        let talk = state.all_channels().into_iter().find(|c| c.name == "Talk").unwrap();
        assert_eq!(funkturm.parent, Some(talk.id));
        assert_eq!(funkturm.kind, ChannelKind::Voice);
    }

    #[test]
    fn join_rules_per_kind() {
        let mut state = test_state();
        let (id, _) = add_test_client(&mut state, "bob");
        let chans = state.all_channels();
        let voice = chans.iter().find(|c| c.kind == ChannelKind::Voice).unwrap().id;
        let text = chans.iter().find(|c| c.kind == ChannelKind::Text).unwrap().id;
        let cat = chans.iter().find(|c| c.kind == ChannelKind::Category).unwrap().id;

        assert!(state.move_client(id, voice).is_ok());
        assert_eq!(state.client(id).unwrap().channel_id, voice);
        assert!(state.move_client(id, text).is_err());
        assert!(state.move_client(id, cat).is_err());
        assert!(state.move_client(id, state.spawn_channel_id()).is_ok());
    }

    #[test]
    fn cannot_create_second_spawn() {
        let mut state = test_state();
        assert!(state.create_channel("Zweiter Spawn", ChannelKind::Spawn, None).is_err());
        assert!(state.create_channel("Ok", ChannelKind::Voice, None).is_ok());
    }

    #[test]
    fn create_with_parent_validation() {
        let mut state = test_state();
        let chans = state.all_channels();
        let cat = chans.iter().find(|c| c.kind == ChannelKind::Category).unwrap().id;
        let voice = chans.iter().find(|c| c.kind == ChannelKind::Voice).unwrap().id;

        // Parent muss Kategorie sein:
        assert!(state.create_channel("X", ChannelKind::Voice, Some(voice)).is_err());
        assert!(state.create_channel("X", ChannelKind::Voice, Some(999)).is_err());
        assert!(state.create_channel("X", ChannelKind::Voice, Some(cat)).is_ok());
    }

    #[test]
    fn delete_category_orphans_children() {
        let mut state = test_state();
        let chans = state.all_channels();
        let cat = chans.iter().find(|c| c.kind == ChannelKind::Category).unwrap().id;
        state.delete_channel(cat).unwrap();
        let funkturm = state.all_channels().into_iter().find(|c| c.name == "Funkturm").unwrap();
        assert_eq!(funkturm.parent, None);
    }

    #[test]
    fn delete_voice_moves_members_to_spawn() {
        let mut state = test_state();
        let (id, _) = add_test_client(&mut state, "carol");
        let voice = state.all_channels().iter().find(|c| c.kind == ChannelKind::Voice).unwrap().id;
        state.move_client(id, voice).unwrap();
        let moved = state.delete_channel(voice).unwrap();
        assert_eq!(moved, vec![id]);
        assert_eq!(state.client(id).unwrap().channel_id, state.spawn_channel_id());
    }

    #[test]
    fn spawn_channel_cannot_be_deleted() {
        let mut state = test_state();
        assert!(state.delete_channel(state.spawn_channel_id()).is_err());
    }

    #[test]
    fn chat_in_text_und_voice_erlaubt() {
        let state = test_state();
        let chans = state.all_channels();
        let text = chans.iter().find(|c| c.kind == ChannelKind::Text).unwrap().id;
        let voice = chans.iter().find(|c| c.kind == ChannelKind::Voice).unwrap().id;
        assert!(state.chat_allowed(text).is_ok());
        assert!(state.chat_allowed(voice).is_ok(), "Voice hat Media-Chat (v4)");
        assert!(state.chat_allowed(state.spawn_channel_id()).is_err());
        assert!(state.chat_allowed(999).is_err());
    }

    #[test]
    fn spawn_log_records_and_caps() {
        let mut state = test_state();
        let e = state.push_spawn_log("alice ist erschienen".into());
        assert_eq!(e.text, "alice ist erschienen");
        assert!(e.timestamp > 0);
        assert_eq!(state.spawn_log().len(), 1);
        for i in 0..(SPAWN_LOG_CAP + 10) {
            state.push_spawn_log(format!("eintrag {i}"));
        }
        assert_eq!(state.spawn_log().len(), SPAWN_LOG_CAP);
    }

    #[test]
    fn no_audio_relay_in_spawn_channel() {
        let mut state = test_state();
        let (id_a, token_a) = add_test_client(&mut state, "a");
        let (_id_b, token_b) = add_test_client(&mut state, "b");
        let addr_a: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        let addr_b: SocketAddr = "10.0.0.2:5000".parse().unwrap();

        // Beide im Spawn: kein Relay (Sprechverbot).
        state.udp_targets(token_b, addr_b);
        let (_, targets) = state.udp_targets(token_a, addr_a).unwrap();
        assert!(targets.is_empty(), "Spawn darf kein Audio relayen");

        // a in Voice-Channel: b (Spawn) hoert nichts, a sendet an niemanden im Voice.
        let voice = state.all_channels().iter().find(|c| c.kind == ChannelKind::Voice).unwrap().id;
        state.move_client(id_a, voice).unwrap();
        let (_, targets) = state.udp_targets(token_a, addr_a).unwrap();
        assert!(targets.is_empty());

        // b auch in Voice: jetzt fliessend Audio.
        state.move_client(_id_b, voice).unwrap();
        let (_, targets) = state.udp_targets(token_a, addr_a).unwrap();
        assert_eq!(targets, vec![addr_b]);
    }

    #[test]
    fn udp_targets_excludes_deafened() {
        let mut state = test_state();
        let (id_a, token_a) = add_test_client(&mut state, "a");
        let (id_b, token_b) = add_test_client(&mut state, "b");
        let voice = state.all_channels().iter().find(|c| c.kind == ChannelKind::Voice).unwrap().id;
        state.move_client(id_a, voice).unwrap();
        state.move_client(id_b, voice).unwrap();
        let addr_a: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        let addr_b: SocketAddr = "10.0.0.2:5000".parse().unwrap();
        state.udp_targets(token_a, addr_a);
        state.udp_targets(token_b, addr_b);
        state.set_mute_state(id_b, false, true);
        let (_, targets) = state.udp_targets(token_a, addr_a).unwrap();
        assert!(targets.is_empty(), "tauber Client bekommt kein Audio");
    }

    #[test]
    fn password_check() {
        let cfg: ServerConfig = toml::from_str(r#"password = "geheim""#).unwrap();
        let state = State::new(&cfg).unwrap();
        assert!(state.check_password(&Some("geheim".to_string())).is_ok());
        assert!(state.check_password(&Some("falsch".to_string())).is_err());
        assert!(state.check_password(&None).is_err());
    }

    #[test]
    fn sharing_nur_in_voice_und_video_targets() {
        let mut state = test_state();
        let (tx1, _r1) = mpsc::unbounded_channel();
        let (tx2, _r2) = mpsc::unbounded_channel();
        let (tx3, _r3) = mpsc::unbounded_channel();
        let (id_a, _) = state.add_client("a".into(), tx1);
        let (id_b, _) = state.add_client("b".into(), tx2);
        let (id_c, _) = state.add_client("c".into(), tx3);
        let voice = state.all_channels().iter().find(|c| c.kind == ChannelKind::Voice).unwrap().id;

        // Im Spawn: Sharing verboten.
        assert!(!state.set_sharing(id_a, true));
        assert!(state.video_targets(id_a).is_empty());

        // In Voice: Sharing ok; Ziele = andere Mitglieder.
        state.move_client(id_a, voice).unwrap();
        state.move_client(id_b, voice).unwrap();
        assert!(state.set_sharing(id_a, true));
        let targets = state.video_targets(id_a);
        assert_eq!(targets.len(), 1, "nur b ist im Voice-Channel");

        // b teilt nicht -> keine Ziele; c im Spawn bekommt nie was.
        assert!(state.video_targets(id_b).is_empty());
        state.move_client(id_c, voice).unwrap();
        assert_eq!(state.video_targets(id_a).len(), 2);

        // Sharing aus -> keine Ziele.
        state.set_sharing(id_a, false);
        assert!(state.video_targets(id_a).is_empty());
    }

    #[test]
    fn persistence_roundtrip() {
        let path = std::env::temp_dir()
            .join(format!("rusttalk-state-test-{}.db", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);
        let cfg: ServerConfig = toml::from_str(&format!(
            "persist_state = true\ndb_path = '{path}'"
        ))
        .unwrap();

        let ch_id;
        {
            let mut st = State::new(&cfg).unwrap();
            ch_id = st.create_channel("Memo", ChannelKind::Text, None).unwrap().id;
            st.persist_message(ch_id, "alice", "bleibt erhalten", 1000, None);
            st.push_spawn_log("alice ist erschienen".into());
        } // Drop -> Neustart simulieren

        {
            let mut st2 = State::new(&cfg).unwrap();
            let memo = st2
                .all_channels()
                .into_iter()
                .find(|c| c.name == "Memo")
                .expect("Channel ueberlebt Neustart");
            assert_eq!(memo.id, ch_id, "ID bleibt stabil");
            assert_eq!(memo.kind, ChannelKind::Text);

            let history = st2.chat_history();
            assert!(history.iter().any(|m| m.text == "bleibt erhalten"), "Chat-Verlauf geladen");
            assert!(
                st2.spawn_log().iter().any(|e| e.text.contains("alice")),
                "Spawn-Log geladen"
            );

            // Loeschung persistiert auch:
            st2.delete_channel(ch_id).unwrap();
            drop(st2);
            let st3 = State::new(&cfg).unwrap();
            assert!(!st3.all_channels().iter().any(|c| c.name == "Memo"));
            assert!(!st3.chat_history().iter().any(|m| m.text == "bleibt erhalten"));
        }
        std::fs::remove_file(&path).ok();
    }

    fn temp_db_cfg(tag: &str, extra: &str) -> (String, ServerConfig) {
        let path = std::env::temp_dir()
            .join(format!("rusttalk-state-{tag}-{}.db", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);
        let cfg: ServerConfig =
            toml::from_str(&format!("persist_state = true\ndb_path = '{path}'\n{extra}")).unwrap();
        (path, cfg)
    }

    #[test]
    fn renamed_config_channel_does_not_duplicate_after_restart() {
        // Frueherer Bug: Config-Channel umbenannt -> nach Neustart doppelt.
        let (path, cfg) =
            temp_db_cfg("rename", "[[channels]]\nname = \"General\"\nkind = \"voice\"\n");
        let mut st = State::new(&cfg).unwrap();
        let id = st.all_channels().iter().find(|c| c.name == "General").unwrap().id;
        st.rename_channel(id, "Lounge").unwrap();
        drop(st);
        let st2 = State::new(&cfg).unwrap();
        let names: Vec<String> = st2.all_channels().into_iter().map(|c| c.name).collect();
        assert!(names.contains(&"Lounge".to_string()));
        assert!(!names.contains(&"General".to_string()), "kein Duplikat: {names:?}");
        assert_eq!(st2.all_channels().iter().find(|c| c.name == "Lounge").unwrap().id, id);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn db_wins_over_changed_config_after_first_start() {
        let (path, cfg) = temp_db_cfg("seed", "[[channels]]\nname = \"Alt\"\nkind = \"voice\"\n");
        drop(State::new(&cfg).unwrap()); // Erststart: Config -> DB
        let cfg2: ServerConfig = toml::from_str(&format!(
            "persist_state = true\ndb_path = '{path}'\n[[channels]]\nname = \"Neu\"\nkind = \"voice\"\n"
        ))
        .unwrap();
        let st = State::new(&cfg2).unwrap();
        let names: Vec<String> = st.all_channels().into_iter().map(|c| c.name).collect();
        assert!(names.contains(&"Alt".to_string()) && !names.contains(&"Neu".to_string()));
        std::fs::remove_file(&path).ok();
    }

    /// Reihenfolge einer Geschwistergruppe als Namen.
    fn order(st: &State, parent: Option<u64>) -> Vec<String> {
        let mut v: Vec<ChannelInfo> = st
            .all_channels()
            .into_iter()
            .filter(|c| c.parent == parent && c.kind != ChannelKind::Spawn)
            .collect();
        v.sort_by_key(|c| (c.position, c.id));
        v.into_iter().map(|c| c.name).collect()
    }

    #[test]
    fn new_channels_are_appended_in_order() {
        let mut st = test_state();
        let talk = st.all_channels().iter().find(|c| c.name == "Talk").unwrap().id;
        st.create_channel("neu", ChannelKind::Text, Some(talk)).unwrap();
        assert_eq!(order(&st, Some(talk)), vec!["Funkturm", "quasselbude", "neu"]);
    }

    #[test]
    fn move_channel_reorders_and_changes_category() {
        let mut st = test_state();
        let talk = st.all_channels().iter().find(|c| c.name == "Talk").unwrap().id;
        let text = st.all_channels().iter().find(|c| c.name == "quasselbude").unwrap().id;
        // Innerhalb der Kategorie nach oben.
        st.move_channel(text, Some(talk), 0).unwrap();
        assert_eq!(order(&st, Some(talk)), vec!["quasselbude", "Funkturm"]);
        // Raus auf die oberste Ebene, vor die Kategorie.
        let placements = st.move_channel(text, None, 0).unwrap();
        assert_eq!(order(&st, None), vec!["quasselbude", "Talk"]);
        assert_eq!(order(&st, Some(talk)), vec!["Funkturm"]);
        assert!(placements.iter().any(|p| p.id == text && p.parent.is_none()));
    }

    #[test]
    fn move_channel_rules() {
        let mut st = test_state();
        let talk = st.all_channels().iter().find(|c| c.name == "Talk").unwrap().id;
        let voice = st.all_channels().iter().find(|c| c.name == "Funkturm").unwrap().id;
        let cat2 = st.create_channel("Mehr", ChannelKind::Category, None).unwrap().id;
        assert!(st.move_channel(st.spawn_channel_id(), None, 3).is_err(), "Spawn fest");
        assert!(st.move_channel(cat2, Some(talk), 0).is_err(), "keine verschachtelten Kategorien");
        assert!(st.move_channel(voice, Some(voice), 0).is_err(), "Ziel muss Kategorie sein");
        assert!(st.move_channel(voice, Some(cat2), 99).is_ok(), "Index wird begrenzt");
        assert_eq!(order(&st, Some(cat2)), vec!["Funkturm"]);
    }

    #[test]
    fn channel_order_survives_restart() {
        let (path, cfg) = temp_db_cfg(
            "order",
            "[[channels]]\nname = \"A\"\nkind = \"voice\"\n[[channels]]\nname = \"B\"\nkind = \"voice\"\n",
        );
        let mut st = State::new(&cfg).unwrap();
        let b = st.all_channels().iter().find(|c| c.name == "B").unwrap().id;
        st.move_channel(b, None, 0).unwrap();
        drop(st);
        let st2 = State::new(&cfg).unwrap();
        assert_eq!(order(&st2, None), vec!["B", "A"]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn server_messages_validation_and_announcements() {
        let mut st = test_state();
        let voice = st.all_channels().iter().find(|c| c.name == "Funkturm").unwrap().id;
        let text = st.all_channels().iter().find(|c| c.name == "quasselbude").unwrap().id;
        // No announcement channel -> nothing is posted.
        assert!(st.announcement(&st.settings().join_message.clone(), "anna").is_none());
        // Only text channels are allowed as announcement target.
        assert!(st
            .update_settings(ServerSettings { announce_channel: Some(voice), ..ServerSettings::default() })
            .is_err());
        for bad in [
            ServerSettings { join_message: "x".repeat(MAX_SERVER_MESSAGE_LEN + 1), ..ServerSettings::default() },
            ServerSettings { leave_message: "x".repeat(MAX_SERVER_MESSAGE_LEN + 1), ..ServerSettings::default() },
            ServerSettings { afk_notice: "x".repeat(MAX_SERVER_MESSAGE_LEN + 1), ..ServerSettings::default() },
            ServerSettings { motd: "x".repeat(MAX_SERVER_MESSAGE_LEN + 1), ..ServerSettings::default() },
            ServerSettings { rules: "x".repeat(MAX_WELCOME_LEN + 1), ..ServerSettings::default() },
        ] {
            assert!(st.update_settings(bad).is_err());
        }
        let saved = st
            .update_settings(ServerSettings {
                announce_channel: Some(text),
                join_message: "  hi {user}!  ".into(),
                motd: " Game night ".into(),
                ..ServerSettings::default()
            })
            .unwrap();
        assert_eq!(saved.join_message, "hi {user}!", "trimmed");
        assert_eq!(saved.motd, "Game night");
        match st.announcement(&saved.join_message, "anna") {
            Some(ServerMessage::Chat { channel_id, from_id, from_name, text: t, .. }) => {
                assert_eq!(channel_id, text);
                assert_eq!(from_id, 0);
                assert_eq!(from_name, ANNOUNCER_NAME);
                assert_eq!(t, "hi anna!");
            }
            other => panic!("expected chat announcement, got {other:?}"),
        }
        // Empty template = off.
        assert!(st.announcement("  ", "anna").is_none());
        // Deleting the announcement channel switches announcements off.
        st.delete_channel(text).unwrap();
        assert_eq!(st.settings().announce_channel, None);
        assert!(st.announcement("hi {user}", "anna").is_none());
    }

    #[test]
    fn afk_notice_uses_template_or_default() {
        let mut st = test_state();
        assert_eq!(st.afk_notice("Lobby"), "You were moved to Lobby after 10 min muted (AFK)");
        st.update_settings(ServerSettings {
            afk_timeout_mins: 5,
            afk_notice: "Zzz - {channel} ({minutes})".into(),
            ..ServerSettings::default()
        })
        .unwrap();
        assert_eq!(st.afk_notice("AFK"), "Zzz - AFK (5)");
        st.update_settings(ServerSettings { afk_notice: String::new(), ..ServerSettings::default() }).unwrap();
        assert!(st.afk_notice("Lobby").starts_with("You were moved to Lobby"), "empty = default");
    }

    #[test]
    fn settings_validation() {
        let mut st = test_state();
        assert_eq!(st.settings().name, "Test", "Name aus der Config");
        let voice = st.all_channels().iter().find(|c| c.name == "Funkturm").unwrap().id;
        let text = st.all_channels().iter().find(|c| c.name == "quasselbude").unwrap().id;
        let ok = ServerSettings {
            name: "  Neuer Name  ".into(),
            welcome_message: "Willkommen!".into(),
            afk_channel: Some(voice),
            ..ServerSettings::default()
        };
        let saved = st.update_settings(ok).unwrap();
        assert_eq!(saved.name, "Neuer Name", "getrimmt");
        for bad in [
            ServerSettings { name: " ".into(), ..ServerSettings::default() },
            ServerSettings { icon: Some(vec![0; MAX_ICON_BYTES + 1]), ..ServerSettings::default() },
            ServerSettings { welcome_message: "x".repeat(MAX_WELCOME_LEN + 1), ..ServerSettings::default() },
            ServerSettings { afk_timeout_mins: 0, ..ServerSettings::default() },
            ServerSettings { afk_channel: Some(text), ..ServerSettings::default() },
        ] {
            assert!(st.update_settings(bad).is_err());
        }
        assert_eq!(st.settings().name, "Neuer Name", "Fehler aendern nichts");
        // AFK-Ziel geloescht -> zurueck auf Lobby.
        st.delete_channel(voice).unwrap();
        assert_eq!(st.settings().afk_channel, None);
    }

    #[test]
    fn afk_sweep_moves_long_muted_voice_users() {
        let mut st = test_state();
        let voice = st.all_channels().iter().find(|c| c.name == "Funkturm").unwrap().id;
        let (a, _) = add_test_client(&mut st, "a");
        let (b, _) = add_test_client(&mut st, "b");
        let (c, _) = add_test_client(&mut st, "c");
        for id in [a, b, c] {
            st.move_client(id, voice).unwrap();
        }
        st.set_mute_state(a, true, false); // stumm
        st.set_mute_state(b, false, true); // taub zaehlt auch
        let now = Instant::now();
        assert!(st.afk_sweep(now + Duration::from_secs(9 * 60)).is_empty(), "noch keine 10 min");
        let mut moved = st.afk_sweep(now + Duration::from_secs(11 * 60));
        moved.sort_unstable();
        let lobby = st.spawn_channel_id();
        assert_eq!(moved, vec![(a, lobby), (b, lobby)], "c redet, bleibt");
        assert!(st.afk_sweep(now + Duration::from_secs(20 * 60)).is_empty(), "nur einmal");

        // Entstummen setzt den Timer zurueck; abgeschaltet -> nichts.
        st.move_client(a, voice).unwrap();
        st.set_mute_state(a, false, false);
        assert!(st.afk_sweep(now + Duration::from_secs(60 * 60)).is_empty());
        st.set_mute_state(a, true, false);
        let off = ServerSettings { afk_enabled: false, ..st.settings().clone() };
        st.update_settings(off).unwrap();
        assert!(st.afk_sweep(Instant::now() + Duration::from_secs(60 * 60)).is_empty());
    }

    #[test]
    fn settings_survive_restart() {
        let (path, cfg) = temp_db_cfg("settings", "");
        let mut st = State::new(&cfg).unwrap();
        let s = ServerSettings {
            name: "Gespeichert".into(),
            icon: Some(vec![1, 2, 3]),
            welcome_message: "Hallo".into(),
            afk_timeout_mins: 30,
            ..ServerSettings::default()
        };
        st.update_settings(s.clone()).unwrap();
        drop(st);
        let st2 = State::new(&cfg).unwrap();
        assert_eq!(st2.settings(), &s);
        std::fs::remove_file(&path).ok();
    }
}
