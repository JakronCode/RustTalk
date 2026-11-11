//! egui UI: Discord-style dark layout with Phosphor icons.
//!
//! Module layout:
//! - `mod.rs`        state, network events, frame loop
//! - `theme`         colors, style, shared widgets
//! - `connect`       connect screen
//! - `sidebar`       server header, channel tree (drag & drop), voice + user panel
//! - `views`         channel header, text chat, voice stage, lobby
//! - `settings`      app settings window (categories)
//! - `channel_settings` channel settings (WIP), create/delete dialogs
//! - `media_ui`      fullscreen, camera preview, screen picker, GIF search
//!
//! Channel kinds: Lobby (house, pinned, join log, no talking), Category
//! (collapsible group), Voice (speaker, click = join), Text (speech bubble).

mod channel_settings;
mod connect;
mod lifecycle;
mod media_ui;
mod server_settings;
mod settings;
mod sidebar;
mod theme;
mod views;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use rusttalk_shared::protocol::{
    GifInfo, ChannelInfo, ChannelKind, ServerMessage, ServerSettings, UserInfo, VideoKind, MAX_MEDIA_BYTES,
};

use crate::audio::sfx::{self, Sound};
use crate::audio::{capture, playback, AudioControl, AudioEngine, LocalMuteSet, TalkingMap, UserVolumeMap};
use crate::config::{ClientConfig, ServerBookmark};
use crate::hotkey;
use crate::logo;
use crate::media::{self, TextureCache};
use crate::net::{self, Command, NetEvent};
use crate::video::{self, CameraDevice, ShareEntry, ShareSource, VideoCapture};

use channel_settings::ChannelSettings;
use settings::SettingsPage;

// Phosphor icons (regular variant) as fallback font.
mod icons {
    pub use egui_phosphor::regular;
}

/// Max. video bytes in our own send queue (~1 s of screen stream).
const MAX_UPLOAD_BACKLOG: usize = 3 * 1024 * 1024;

enum ChatKind {
    Message { from: String, own: bool },
    System,
}

struct ChatLine {
    /// Server message id (0 = local/system line).
    id: u64,
    kind: ChatKind,
    text: String,
    timestamp: u64,
    /// Optionales Bild (PNG/JPEG/GIF-Bytes).
    image: Option<Vec<u8>>,
    /// Answer to this message id.
    reply_to: Option<u64>,
    edited: bool,
}

impl ChatLine {
    fn system(text: String, timestamp: u64) -> Self {
        ChatLine { id: 0, kind: ChatKind::System, text, timestamp, image: None, reply_to: None, edited: false }
    }
}

/// What the user picked on a chat message (hover buttons / right click).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatAction {
    Reply(u64),
    /// Start editing inline.
    Edit(u64),
    /// Ask "really delete?".
    Delete(u64),
    /// Delete without asking (Shift + click).
    DeleteNow(u64),
    /// Inline edit: save (check) / cancel (x).
    SaveEdit,
    CancelEdit,
}

/// Does `text` mention `name` ("@name", case-insensitive, not inside a longer name)?
fn mentions(text: &str, name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    let hay = text.to_lowercase();
    let needle = format!("@{}", name.to_lowercase());
    let mut start = 0;
    while let Some(pos) = hay[start..].find(&needle) {
        let end = start + pos + needle.len();
        let next = hay[end..].chars().next();
        if !next.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            return true;
        }
        start = end;
    }
    false
}

/// The "@prefix" being typed at the end of the input, if any.
fn mention_prefix(input: &str) -> Option<&str> {
    let last = input.rsplit(char::is_whitespace).next()?;
    last.strip_prefix('@')
}

/// GIF picker popup (Discord style): trending on open, search while typing,
/// two-column grid, click = send. Searches go through the server.
#[derive(Default)]
struct GifPicker {
    open: bool,
    /// Text in the search field.
    query: String,
    /// When the search field last changed (debounce).
    typed_at: Option<Instant>,
    /// Query of the shown results (`Some("")` = trending).
    shown_query: Option<String>,
    results: Vec<GifInfo>,
    loading: bool,
    /// No more pages.
    end: bool,
    error: Option<String>,
    /// Downloaded previews by URL.
    previews: HashMap<String, Vec<u8>>,
    /// Preview URLs already being downloaded.
    requested: HashSet<String>,
    /// The chosen GIF is being downloaded.
    sending: bool,
    /// Focus the search field in the next frame.
    focus: bool,
}

/// Kamera-Dialog: Geraeteauswahl + Live-Vorschau vor dem Teilen.
struct CamDialog {
    devices: Vec<CameraDevice>,
    selected: Option<u32>,
    error: Option<String>,
}

/// Reiter im Bildschirm-Auswahldialog.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShareTab {
    Apps,
    Screens,
}

/// Bildschirm/Fenster-Auswahl (aehnlich Discord): Vorschaubilder werden
/// im Hintergrund erzeugt und per Channel geliefert.
struct ScreenPicker {
    tab: ShareTab,
    entries: Vec<(ShareEntry, Option<egui::TextureHandle>)>,
    selected: Option<ShareSource>,
    loading: bool,
    rx: std::sync::mpsc::Receiver<Vec<ShareEntry>>,
}

impl ScreenPicker {
    fn open() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("share-sources".into())
            .spawn(move || {
                let _ = tx.send(video::list_share_sources());
            });
        Self { tab: ShareTab::Apps, entries: Vec::new(), selected: None, loading: true, rx }
    }
}

/// Automatic reconnect after an unexpected connection loss.
struct Reconnect {
    /// Number of the next attempt (1-based).
    attempt: u32,
    /// When the next attempt starts (`None` = attempt is running).
    next_at: Option<Instant>,
    /// Voice channel we were in (id, name as fallback), rejoined afterwards.
    channel: Option<(u64, String)>,
    muted: bool,
    deafened: bool,
}

/// Give up after this many failed attempts (~10 minutes with backoff).
const MAX_RECONNECT_ATTEMPTS: u32 = 25;

/// Wait time before reconnect attempt `attempt` (1-based): 1, 2, 4, 8, 15, 30, 30, ... s.
fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_secs(match attempt {
        0 | 1 => 1,
        2 => 2,
        3 => 4,
        4 => 8,
        5 => 15,
        _ => 30,
    })
}

/// Nachrichten aus Hintergrund-Threads (GIPHY-Suche/Downloads).
enum GiphyMsg {
    Preview(String, Vec<u8>),
    Full(Vec<u8>),
    /// Download of the chosen GIF failed.
    FullFailed(String),
}

pub struct RustTalkApp {
    config: ClientConfig,
    event_tx: std::sync::mpsc::Sender<NetEvent>,
    event_rx: std::sync::mpsc::Receiver<NetEvent>,
    cmd_tx: Option<tokio::sync::mpsc::UnboundedSender<Command>>,

    // Gespiegelter Server-State
    my_id: u64,
    users: HashMap<u64, UserInfo>,
    channels: HashMap<u64, ChannelInfo>,
    chat: HashMap<u64, Vec<ChatLine>>,
    unread: HashSet<u64>,
    selected_channel: Option<u64>,
    collapsed: HashMap<u64, bool>,

    // UI-State
    chat_input: String,
    /// Composer answers this message.
    reply_to: Option<u64>,
    /// Own message being edited inline (text in `edit_text`).
    editing: Option<u64>,
    edit_text: String,
    /// Focus the inline edit field in the next frame.
    focus_edit: bool,
    /// Message waiting for "really delete?".
    confirm_delete_msg: Option<u64>,
    /// Search in the current channel (`Some` = search bar open).
    chat_search: Option<String>,
    /// Picked on a message; executed in the next frame (needs the command sender).
    chat_action: Option<ChatAction>,
    /// Focus the message input in the next frame (after Reply / Edit).
    focus_composer: bool,
    new_channel_name: String,
    new_channel_kind: ChannelKind,
    new_channel_parent: Option<u64>,
    show_new_channel: bool,
    show_settings: bool,
    settings_page: SettingsPage,
    /// Settings: waiting for a key press to bind push-to-talk.
    recording_key: bool,
    /// Camera list for the settings (lazy, `None` = reload).
    cameras: Option<Vec<CameraDevice>>,
    channel_settings: Option<ChannelSettings>,
    /// Channel waiting for "really delete?" confirmation.
    confirm_delete: Option<u64>,
    /// Set from a menu; executed at the start of the next frame.
    pending_disconnect: bool,
    /// Pending automatic reconnect (connection was lost unexpectedly).
    reconnect: Option<Reconnect>,
    /// Login finished (Welcome received). `cmd_tx` alone = connecting.
    connected: bool,
    server_settings: ServerSettings,
    /// Open "Server Settings" dialog (edit copy).
    server_settings_dialog: Option<server_settings::ServerSettingsDialog>,
    status: String,
    status_is_error: bool,
    /// When `status` last changed (toast fade-out).
    status_at: Instant,
    status_seen: String,
    input_devices: Vec<String>,
    output_devices: Vec<String>,
    mute_before_deafen: bool,

    // Audio
    audio: Option<AudioEngine>,
    control: Arc<AudioControl>,
    talking: TalkingMap,
    local_muted: LocalMuteSet,
    /// Notification sounds waiting for the output stream.
    sfx: sfx::SfxQueue,
    /// Binding for the global PTT poller (`None` = off / focused window only).
    global_ptt: hotkey::GlobalBinding,
    /// Per-user playback volume (client id -> gain), filled from `config.user_volumes`.
    user_volumes: UserVolumeMap,
    /// Pegel-Messung ohne Verbindung (Einstellungs-Fenster).
    mic_monitor: Option<capture::MicMonitor>,

    // Medien & Video
    textures: TextureCache,
    /// Angehaengtes, noch nicht gesendetes Bild (Name, Bytes).
    pending_image: Option<(String, Vec<u8>)>,
    /// Letzte Video-Frames pro (User, Art) als JPEG – Kamera und Bildschirm
    /// eines Users sind getrennte Kacheln.
    video_frames: HashMap<(u64, VideoKind), Vec<u8>>,
    /// Video-Bytes in der Sende-Queue (siehe `net::connect`).
    upload_backlog: Arc<std::sync::atomic::AtomicUsize>,
    /// Vom Bildschirm-Thread gesetzt, wenn er wegen eines Fehlers endet.
    screen_failed: Arc<std::sync::atomic::AtomicBool>,
    /// Vollbild-Ansicht eines Streams (None = normale Ansicht).
    fullscreen: Option<(u64, VideoKind)>,
    /// egui-Kontext fuer Repaints aus Capture-/Netzwerk-Threads.
    ctx: egui::Context,
    /// Kamera (Vorschau oder live, siehe `VideoCapture::is_sending`).
    cam: Option<VideoCapture>,
    screen: Option<VideoCapture>,
    cam_dialog: Option<CamDialog>,
    screen_picker: Option<ScreenPicker>,
    /// Letzter eigener Frame (auch im reinen Vorschau-Modus).
    local_preview: Arc<Mutex<Option<Vec<u8>>>>,
    /// Textur der eigenen Vorschau (wird pro Frame ueberschrieben).
    local_preview_tex: Option<(u64, egui::TextureHandle)>,
    /// Fehler aus dem Kamera-Thread fuer die Anzeige im Kamera-Dialog.
    cam_error: Arc<Mutex<Option<String>>>,
    giphy: GifPicker,
    /// Not installed yet: the whole window is the install screen.
    install_screen: Option<lifecycle::InstallScreen>,
    update: lifecycle::UpdateState,
    /// Clicked toasts report their channel here.
    toast_tx: std::sync::mpsc::Sender<u64>,
    toast_rx: std::sync::mpsc::Receiver<u64>,
    last_toast: Option<Instant>,
    /// The server offers GIF search (it has a GIPHY key).
    gif_available: bool,
    /// Where the GIF that is downloading goes: (channel, reply_to).
    gif_target: Option<(u64, Option<u64>)>,
    /// Screen rect of the GIF button (the picker opens above it).
    gif_button_rect: Option<egui::Rect>,
    giphy_tx: std::sync::mpsc::Sender<GiphyMsg>,
    giphy_rx: std::sync::mpsc::Receiver<GiphyMsg>,
}

impl RustTalkApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        cc.egui_ctx.set_fonts(fonts);
        theme::apply(&cc.egui_ctx);

        let config = ClientConfig::load();
        cc.egui_ctx.set_zoom_factor(config.ui_scale.clamp(0.75, 1.5));
        let control = Arc::new(AudioControl::new(config.voice_activation, config.vad_threshold));
        control.set_output_volume(config.output_volume);
        control.set_input_volume(config.input_volume);
        let global_ptt = hotkey::spawn_global_ptt(control.clone());
        let (toast_tx, toast_rx) = std::sync::mpsc::channel::<u64>();
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let (giphy_tx, giphy_rx) = std::sync::mpsc::channel::<GiphyMsg>();
        Self {
            control,
            config,
            event_tx,
            event_rx,
            cmd_tx: None,
            my_id: 0,
            users: HashMap::new(),
            channels: HashMap::new(),
            chat: HashMap::new(),
            unread: HashSet::new(),
            selected_channel: None,
            collapsed: HashMap::new(),
            chat_input: String::new(),
            reply_to: None,
            editing: None,
            edit_text: String::new(),
            focus_edit: false,
            confirm_delete_msg: None,
            chat_search: None,
            chat_action: None,
            focus_composer: false,
            new_channel_name: String::new(),
            new_channel_kind: ChannelKind::Text,
            new_channel_parent: None,
            show_new_channel: false,
            show_settings: false,
            settings_page: SettingsPage::Voice,
            recording_key: false,
            cameras: None,
            channel_settings: None,
            confirm_delete: None,
            pending_disconnect: false,
            reconnect: None,
            connected: false,
            server_settings: ServerSettings::default(),
            server_settings_dialog: None,
            status: String::new(),
            status_is_error: false,
            status_at: Instant::now(),
            status_seen: String::new(),
            input_devices: Vec::new(),
            output_devices: Vec::new(),
            mute_before_deafen: false,
            audio: None,
            talking: Arc::new(Mutex::new(HashMap::new())),
            local_muted: Arc::new(Mutex::new(HashSet::new())),
            global_ptt,
            sfx: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            user_volumes: Arc::new(Mutex::new(HashMap::new())),
            mic_monitor: None,
            textures: TextureCache::default(),
            pending_image: None,
            video_frames: HashMap::new(),
            screen_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            upload_backlog: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            fullscreen: None,
            ctx: cc.egui_ctx.clone(),
            cam: None,
            screen: None,
            cam_dialog: None,
            screen_picker: None,
            local_preview: Arc::new(Mutex::new(None)),
            local_preview_tex: None,
            cam_error: Arc::new(Mutex::new(None)),
            giphy: GifPicker::default(),
            install_screen: (crate::install::mode() == crate::install::Mode::NotInstalled).then(lifecycle::InstallScreen::new),
            update: lifecycle::UpdateState::new(),
            toast_tx,
            toast_rx,
            last_toast: None,
            gif_available: false,
            gif_target: None,
            gif_button_rect: None,
            giphy_tx: giphy_tx.clone(),
            giphy_rx,
        }
    }

    /// Logged in (Welcome received).
    fn is_connected(&self) -> bool {
        self.cmd_tx.is_some() && self.connected
    }

    fn spawn_channel_id(&self) -> Option<u64> {
        self.channels.values().find(|c| c.kind == ChannelKind::Spawn).map(|c| c.id)
    }

    /// Info message (toast in the main view, line on the connect screen).
    fn set_status(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_is_error = false;
        self.status_at = Instant::now();
        self.status_seen = self.status.clone();
    }

    /// Error message (red toast).
    fn set_error(&mut self, msg: impl Into<String>) {
        self.set_status(msg);
        self.status_is_error = true;
    }

    fn start_connect(&mut self) {
        self.set_status(format!("Connecting to {}...", self.config.server_address));
        self.control.set_vad_threshold(self.config.vad_threshold);
        self.control
            .voice_activation
            .store(self.config.voice_activation, Ordering::Relaxed);
        self.control.self_muted.store(false, Ordering::Relaxed);
        self.control.self_deafened.store(false, Ordering::Relaxed);
        self.control.noise_suppression.store(self.config.noise_suppression, Ordering::Relaxed);
        self.control.auto_gain.store(self.config.auto_gain, Ordering::Relaxed);
        // Neuer Zaehler pro Verbindung (alte Queue ist mit der Verbindung weg).
        self.upload_backlog = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        self.cmd_tx = Some(net::connect(
            self.config.clone(),
            self.event_tx.clone(),
            self.ctx.clone(),
            self.upload_backlog.clone(),
        ));
        self.config.save();
    }

    fn disconnect(&mut self) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(Command::Disconnect);
        }
        self.connected = false;
        self.channel_settings = None;
        self.server_settings_dialog = None;
        self.confirm_delete = None;
        self.show_new_channel = false;
        self.audio = None; // stops the audio threads
        self.users.clear();
        self.channels.clear();
        self.chat.clear();
        self.unread.clear();
        self.selected_channel = None;
        self.reply_to = None;
        self.editing = None;
        self.edit_text.clear();
        self.confirm_delete_msg = None;
        self.chat_search = None;
        self.chat_action = None;
        self.close_gif_picker();
        self.gif_available = false;
        self.gif_target = None;
        self.talking.lock().unwrap().clear();
        self.local_muted.lock().unwrap().clear();
        self.user_volumes.lock().unwrap().clear();
        self.sfx.lock().unwrap().clear();
        self.control.transmitting.store(false, Ordering::Relaxed);
        // Video stoppen + Frames rauswerfen.
        self.cam = None;
        self.screen = None;
        self.cam_dialog = None;
        self.screen_picker = None;
        *self.local_preview.lock().unwrap() = None;
        self.local_preview_tex = None;
        self.video_frames.clear();
        self.leave_fullscreen();
        self.pending_image = None;
    }

    /// System-Meldung in die Aktivitaetsliste eines Channels schreiben.
    fn system_line(&mut self, channel_id: u64, text: String, timestamp: u64) {
        self.chat
            .entry(channel_id)
            .or_default()
            .push(ChatLine::system(text, timestamp));
        if self.selected_channel != Some(channel_id) {
            self.unread.insert(channel_id);
        }
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn channel_name(&self, id: u64) -> String {
        self.channels.get(&id).map(|c| c.name.clone()).unwrap_or_else(|| "?".into())
    }

    fn channel_kind(&self, id: u64) -> Option<ChannelKind> {
        self.channels.get(&id).map(|c| c.kind)
    }

    /// Loads the saved volume for `name` (by username, local) into the mixer.
    fn apply_user_volume(&self, id: u64, name: &str) {
        let mut map = self.user_volumes.lock().unwrap();
        match self.config.user_volumes.get(name) {
            Some(&v) => {
                map.insert(id, v.clamp(0.0, crate::audio::MAX_VOLUME));
            }
            None => {
                map.remove(&id);
            }
        }
    }

    /// Network thread ended. After a successful login this starts the
    /// automatic reconnect; a failed reconnect attempt schedules the next one.
    fn on_connection_lost(&mut self, reason: String) {
        let was_connected = self.connected;
        if was_connected && self.reconnect.is_none() {
            let me = self.users.get(&self.my_id);
            let channel = me
                .and_then(|u| self.channels.get(&u.channel_id))
                .filter(|c| c.kind == ChannelKind::Voice)
                .map(|c| (c.id, c.name.clone()));
            self.reconnect = Some(Reconnect {
                attempt: 1,
                next_at: None,
                channel,
                muted: self.control.self_muted.load(Ordering::Relaxed),
                deafened: self.control.self_deafened.load(Ordering::Relaxed),
            });
        }
        self.disconnect();
        let Some(rc) = self.reconnect.as_mut() else {
            self.set_error(format!("Disconnected: {reason}"));
            return;
        };
        if !was_connected {
            rc.attempt += 1; // the running attempt failed
        }
        if rc.attempt > MAX_RECONNECT_ATTEMPTS {
            self.reconnect = None;
            self.set_error(format!("Disconnected: {reason} (gave up reconnecting)"));
            return;
        }
        let delay = reconnect_delay(rc.attempt);
        rc.next_at = Some(Instant::now() + delay);
        let attempt = rc.attempt;
        self.set_error(format!(
            "Connection lost ({reason}) - reconnecting in {} s (attempt {attempt}/{MAX_RECONNECT_ATTEMPTS})",
            delay.as_secs()
        ));
    }

    /// Starts the next reconnect attempt when its time has come.
    fn poll_reconnect(&mut self) {
        let due = self
            .reconnect
            .as_ref()
            .and_then(|rc| rc.next_at)
            .is_some_and(|t| Instant::now() >= t);
        if due && self.cmd_tx.is_none() {
            if let Some(rc) = self.reconnect.as_mut() {
                rc.next_at = None;
            }
            self.start_connect();
        }
    }

    /// Cancels a pending automatic reconnect (button on the connect screen).
    fn cancel_reconnect(&mut self) {
        if self.reconnect.take().is_some() {
            if self.cmd_tx.is_some() && !self.connected {
                self.disconnect(); // stop the running attempt
            }
            self.set_status("Reconnect cancelled");
        }
    }

    /// After a successful reconnect: back into the old voice channel, same mute state.
    fn restore_after_reconnect(&mut self, rc: Reconnect) {
        let Some(tx) = self.cmd_tx.clone() else { return };
        if rc.muted || rc.deafened {
            self.control.self_muted.store(rc.muted, Ordering::Relaxed);
            self.control.self_deafened.store(rc.deafened, Ordering::Relaxed);
            let _ = tx.send(Command::SetMute { muted: rc.muted, deafened: rc.deafened });
        }
        if let Some((id, name)) = rc.channel {
            let target = self
                .channels
                .get(&id)
                .filter(|c| c.kind == ChannelKind::Voice && c.name == name)
                .or_else(|| self.channels.values().find(|c| c.kind == ChannelKind::Voice && c.name == name))
                .map(|c| c.id);
            if let Some(id) = target {
                let _ = tx.send(Command::JoinChannel(id));
                self.select_channel(id);
            }
        }
        self.set_status(format!("Reconnected to {}", self.server_settings.name));
    }

    /// Our voice channel (None in the lobby or when not in a voice channel).
    fn my_voice_channel(&self) -> Option<u64> {
        let ch = self.users.get(&self.my_id)?.channel_id;
        (self.channel_kind(ch) == Some(ChannelKind::Voice)).then_some(ch)
    }

    fn window_focused(&self) -> bool {
        self.ctx.input(|i| i.viewport().focused).unwrap_or(true)
    }

    /// Plays a notification sound if it is enabled in the settings.
    pub(super) fn notify(&self, sound: Sound) {
        let on = match sound {
            Sound::Join => self.config.sound_join,
            Sound::Leave => self.config.sound_leave,
            Sound::Message => self.config.sound_message,
            Sound::Mute | Sound::Unmute => self.config.sound_mute,
        };
        if on && self.audio.is_some() {
            sfx::play(&self.sfx, sound);
        }
    }

    /// Current playback volume of one user (1.0 = 100 %).
    fn user_volume(&self, id: u64) -> f32 {
        self.user_volumes.lock().unwrap().get(&id).copied().unwrap_or(1.0)
    }

    /// Sets the playback volume of one user and remembers it by username.
    fn set_user_volume(&mut self, id: u64, name: &str, volume: f32) {
        let v = volume.clamp(0.0, crate::audio::MAX_VOLUME);
        if (v - 1.0).abs() < 0.005 {
            self.config.user_volumes.remove(name);
            self.user_volumes.lock().unwrap().remove(&id);
        } else {
            self.config.user_volumes.insert(name.to_string(), v);
            self.user_volumes.lock().unwrap().insert(id, v);
        }
    }

    /// Applies a picked message action (reply / edit / delete).
    fn apply_chat_action(&mut self, cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>) {
        let Some(action) = self.chat_action.take() else { return };
        match action {
            ChatAction::Reply(id) => {
                self.editing = None;
                self.reply_to = Some(id);
                self.focus_composer = true;
            }
            ChatAction::Edit(id) => {
                let text = self.chat.values().flatten().find(|l| l.id == id).map(|l| l.text.clone());
                if let Some(text) = text {
                    self.editing = Some(id);
                    self.edit_text = text;
                    self.focus_edit = true;
                }
            }
            ChatAction::Delete(id) => self.confirm_delete_msg = Some(id),
            ChatAction::DeleteNow(id) => {
                let _ = cmd_tx.send(Command::DeleteChat(id));
            }
            ChatAction::SaveEdit => {
                let Some(id) = self.editing.take() else { return };
                let text = std::mem::take(&mut self.edit_text).trim().to_string();
                let original = self.chat.values().flatten().find(|l| l.id == id).map(|l| l.text.clone());
                if text.is_empty() {
                    // Like Discord: saving an empty message asks to delete it.
                    self.confirm_delete_msg = Some(id);
                } else if original.as_deref() != Some(text.as_str()) {
                    let _ = cmd_tx.send(Command::EditChat { message_id: id, text });
                }
            }
            ChatAction::CancelEdit => {
                self.editing = None;
                self.edit_text.clear();
            }
        }
    }

    fn select_channel(&mut self, id: u64) {
        if self.selected_channel != Some(id) {
            self.reply_to = None;
            self.editing = None;
            self.chat_search = None;
        }
        self.selected_channel = Some(id);
        self.unread.remove(&id);
    }

    fn handle_event(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Connected {
                my_id,
                settings,
                users,
                channels,
                udp_socket,
                udp_token,
                spawn_log,
                chat_history,
                gif_search,
            } => {
                if self.cmd_tx.is_none() {
                    return; // attempt was cancelled meanwhile; the net thread ends on its own
                }
                self.connected = true;
                self.gif_available = gif_search;
                self.server_settings = settings;
                self.my_id = my_id;
                self.channels = channels.into_iter().map(|c| (c.id, c)).collect();
                self.users = users.into_iter().map(|u| (u.id, u)).collect();
                let known: Vec<(u64, String)> = self.users.values().map(|u| (u.id, u.name.clone())).collect();
                for (id, name) in known {
                    self.apply_user_volume(id, &name);
                }
                // Spawn-Log in die Spawn-Ansicht laden.
                if let Some(spawn_id) = self.spawn_channel_id() {
                    let lines = spawn_log
                        .into_iter()
                        .map(|e| ChatLine::system(e.text, e.timestamp))
                        .collect();
                    self.chat.insert(spawn_id, lines);
                    self.selected_channel = Some(spawn_id);
                }
                // Persistierten Chat-Verlauf in die Channels laden
                // ("eigene" Nachrichten per Name erkannt – kein Account-System).
                for rec in chat_history {
                    let own = rec.from_name == self.config.username;
                    self.chat.entry(rec.channel_id).or_default().push(ChatLine {
                        id: rec.id,
                        kind: ChatKind::Message { from: rec.from_name, own },
                        text: rec.text,
                        timestamp: rec.timestamp,
                        image: rec.image,
                        reply_to: rec.reply_to,
                        edited: rec.edited,
                    });
                }
                self.set_status(format!("Connected to {}", self.server_settings.name));
                if let Some(rc) = self.reconnect.take() {
                    self.restore_after_reconnect(rc);
                }
                if let Some(sock) = udp_socket {
                    match AudioEngine::start(
                        sock,
                        udp_token,
                        self.control.clone(),
                        self.talking.clone(),
                        self.local_muted.clone(),
                        self.user_volumes.clone(),
                        self.sfx.clone(),
                        self.config.input_device.as_deref(),
                        self.config.output_device.as_deref(),
                    ) {
                        Ok(engine) => self.audio = Some(engine),
                        Err(e) => self.set_error(format!("Audio error: {e:#} (chat still works)")),
                    }
                }
            }
            NetEvent::Message(msg) => self.handle_server_message(msg),
            NetEvent::Status(text) => self.set_error(text),
            NetEvent::Disconnected { reason } => self.on_connection_lost(reason),
        }
    }

    fn handle_server_message(&mut self, msg: ServerMessage) {
        match msg {
            ServerMessage::Welcome { .. } => {} // kommt nur via NetEvent::Connected
            ServerMessage::UserJoined { user } => {
                if let Some(ch) = self.channels.get_mut(&user.channel_id) {
                    if !ch.user_ids.contains(&user.id) {
                        ch.user_ids.push(user.id);
                    }
                }
                self.apply_user_volume(user.id, &user.name);
                self.users.insert(user.id, user);
                // Join-Zeile kommt serverseitig via SpawnLogEntry.
            }
            ServerMessage::UserLeft { client_id } => {
                if self.users.get(&client_id).is_some_and(|u| Some(u.channel_id) == self.my_voice_channel()) {
                    self.notify(Sound::Leave);
                }
                self.users.remove(&client_id);
                self.user_volumes.lock().unwrap().remove(&client_id);
                for ch in self.channels.values_mut() {
                    ch.user_ids.retain(|&id| id != client_id);
                }
                self.clear_video_of(client_id, None);
            }
            ServerMessage::UserMoved { client_id, channel_id } => {
                // Wer den Channel wechselt, schickt uns keine Frames mehr (bzw.
                // wir bekommen keine mehr, wenn wir selbst wechseln).
                if client_id == self.my_id {
                    let others: Vec<u64> = self
                        .video_frames
                        .keys()
                        .map(|(id, _)| *id)
                        .filter(|id| *id != self.my_id)
                        .collect();
                    for id in others {
                        self.clear_video_of(id, None);
                    }
                } else {
                    self.clear_video_of(client_id, None);
                }
                let name = self.users.get(&client_id).map(|u| u.name.clone());
                if client_id != self.my_id {
                    let mine = self.my_voice_channel();
                    let old = self.users.get(&client_id).map(|u| u.channel_id);
                    if mine.is_some() && Some(channel_id) == mine {
                        self.notify(Sound::Join);
                    } else if mine.is_some() && old == mine {
                        self.notify(Sound::Leave);
                    }
                }
                if let Some(u) = self.users.get_mut(&client_id) {
                    u.channel_id = channel_id;
                }
                for ch in self.channels.values_mut() {
                    if ch.id == channel_id {
                        if !ch.user_ids.contains(&client_id) {
                            ch.user_ids.push(client_id);
                        }
                    } else {
                        ch.user_ids.retain(|&id| id != client_id);
                    }
                }
                if let Some(name) = name {
                    let line = if client_id == self.my_id {
                        format!("You joined {}", self.channel_name(channel_id))
                    } else {
                        format!("{name} joined {}", self.channel_name(channel_id))
                    };
                    self.system_line(channel_id, line, Self::now());
                }
            }
            ServerMessage::ChannelCreated { channel } => {
                self.channels.insert(channel.id, channel);
            }
            ServerMessage::ChannelDeleted { channel_id } => {
                let was_category =
                    self.channels.get(&channel_id).is_some_and(|c| c.kind.is_group());
                self.channels.remove(&channel_id);
                if was_category {
                    // Kinder der geloeschten Kategorie werden Top-Level.
                    for ch in self.channels.values_mut() {
                        if ch.parent == Some(channel_id) {
                            ch.parent = None;
                        }
                    }
                }
                self.chat.remove(&channel_id);
                self.unread.remove(&channel_id);
                self.collapsed.remove(&channel_id);
                if self.selected_channel == Some(channel_id) {
                    self.selected_channel = self.spawn_channel_id();
                }
            }
            ServerMessage::ChannelRenamed { channel_id, name } => {
                if let Some(ch) = self.channels.get_mut(&channel_id) {
                    ch.name = name;
                }
            }
            ServerMessage::ChannelsReordered { placements } => {
                for p in placements {
                    if let Some(ch) = self.channels.get_mut(&p.id) {
                        ch.parent = p.parent;
                        ch.position = p.position;
                    }
                }
            }
            ServerMessage::SpawnLogEntry { entry } => {
                if let Some(spawn_id) = self.spawn_channel_id() {
                    self.system_line(spawn_id, entry.text, entry.timestamp);
                }
            }
            ServerMessage::Chat { id, channel_id, from_id, from_name, text, timestamp, image, reply_to } => {
                let own = from_id == self.my_id;
                // from_id 0 = server announcement (no sound).
                if !own && from_id != 0 {
                    let focused = self.window_focused();
                    let mentioned = mentions(&text, &self.config.username);
                    if mentioned || !focused || self.selected_channel != Some(channel_id) {
                        self.notify(Sound::Message);
                    }
                    if !focused {
                        self.message_toast(channel_id, &from_name, &text, image.is_some(), mentioned);
                    }
                    if !focused && self.config.flash_taskbar {
                        self.ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                            egui::UserAttentionType::Informational,
                        ));
                    }
                }
                self.chat
                    .entry(channel_id)
                    .or_default()
                    .push(ChatLine {
                        id,
                        kind: ChatKind::Message { from: from_name, own },
                        text,
                        timestamp,
                        image,
                        reply_to,
                        edited: false,
                    });
                if self.selected_channel != Some(channel_id) {
                    self.unread.insert(channel_id);
                }
            }
            ServerMessage::GifResults { query, offset, results, error } => self.on_gif_results(query, offset, results, error),
            ServerMessage::GifSearchChanged { available } => {
                self.gif_available = available;
                self.giphy.shown_query = None; // reload the picker
                self.giphy.results.clear();
            }
            ServerMessage::ChatEdited { channel_id, message_id, text } => {
                if let Some(line) = self.chat.get_mut(&channel_id).and_then(|v| v.iter_mut().find(|l| l.id == message_id)) {
                    line.text = text;
                    line.edited = true;
                }
            }
            ServerMessage::ChatDeleted { channel_id, message_id } => {
                if let Some(lines) = self.chat.get_mut(&channel_id) {
                    lines.retain(|l| l.id != message_id);
                }
                if self.editing == Some(message_id) {
                    self.editing = None;
                }
                if self.confirm_delete_msg == Some(message_id) {
                    self.confirm_delete_msg = None;
                }
                if self.reply_to == Some(message_id) {
                    self.reply_to = None;
                }
            }
            ServerMessage::MuteStateChanged { client_id, muted, deafened } => {
                if let Some(u) = self.users.get_mut(&client_id) {
                    u.muted = muted;
                    u.deafened = deafened;
                }
            }
            ServerMessage::SharingChanged { client_id, sharing } => {
                if let Some(u) = self.users.get_mut(&client_id) {
                    u.sharing = sharing;
                }
                if !sharing {
                    self.clear_video_of(client_id, None);
                }
            }
            ServerMessage::VideoFrame { from_id, kind, data } => {
                // Alten Frame ersetzen (Texture-Cache schont die Dekodierung).
                let key = (from_id, kind);
                if let Some(old) = self.video_frames.get(&key) {
                    if old.len() == data.len() && old == &data {
                        return; // identischer Frame, nichts tun
                    }
                    let old = old.clone();
                    self.textures.remove_bytes(&old);
                }
                self.video_frames.insert(key, data);
            }
            ServerMessage::VideoStopped { from_id, kind } => {
                self.clear_video_of(from_id, Some(kind));
            }
            ServerMessage::ServerSettingsChanged { settings } => {
                self.server_settings = settings;
            }
            ServerMessage::Notice { message } => {
                self.set_status(message);
            }
            ServerMessage::Error { message } => {
                self.set_error(message);
            }
        }
    }
}

impl eframe::App for RustTalkApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.install_screen.is_some() {
            self.ui_install(ctx);
            return;
        }
        self.poll_background(ctx);
        // Installed app: look for an update once per start.
        static CHECKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if self.config.check_updates
            && crate::install::mode() == crate::install::Mode::Installed
            && !CHECKED.swap(true, Ordering::Relaxed)
        {
            self.check_for_updates();
        }
        while let Ok(ev) = self.event_rx.try_recv() {
            self.handle_event(ev);
        }
        #[cfg(debug_assertions)]
        self.dev_hooks(ctx);
        if std::mem::take(&mut self.pending_disconnect) {
            self.reconnect = None;
            self.disconnect();
            self.set_status("Disconnected");
        }
        self.poll_reconnect();
        // Status changed somewhere without set_status -> restart the toast.
        if self.status != self.status_seen {
            self.status_seen = self.status.clone();
            self.status_at = Instant::now();
        }
        // Ctrl +/- zoom: remember for the next start.
        let zoom = ctx.zoom_factor();
        if (zoom - self.config.ui_scale).abs() > 0.001 {
            self.config.ui_scale = zoom;
        }

        if self.is_connected() {
            // Push-to-talk: either the global poller (also in the background)
            // or the focused window. Never while a new key is being recorded.
            let binding = if self.recording_key { None } else { hotkey::parse_binding(&self.config.ptt_key) };
            let global = self.config.global_ptt && binding.is_some();
            *self.global_ptt.lock().unwrap() = if global { binding } else { None };
            if !global {
                let pressed = binding.is_some_and(|b| ctx.input(|i| b.is_down_in(i)));
                self.control.ptt_pressed.store(pressed, Ordering::Relaxed);
            }
            self.ui_connected(ctx);
        } else {
            *self.global_ptt.lock().unwrap() = None;
            self.ui_connect(ctx);
        }
        if self.show_settings {
            self.ui_settings(ctx);
        }
        self.ui_new_channel(ctx);
        self.ui_channel_settings(ctx);
        self.ui_server_settings(ctx);
        self.ui_confirm_delete(ctx);
        self.ui_confirm_delete_message(ctx);
        self.ui_gif_picker(ctx);
        self.ui_camera_dialog(ctx);
        self.ui_screen_picker(ctx);

        // Mic monitor for the level meter in the settings (without a connection).
        if self.show_settings && self.audio.is_none() && self.mic_monitor.is_none() {
            self.mic_monitor =
                capture::MicMonitor::start(self.config.input_device.as_deref(), Some(self.control.input_volume_handle())).ok();
        }
        if !self.show_settings && self.mic_monitor.is_some() {
            self.mic_monitor = None; // Drop stoppt Stream + Thread
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.config.save();
    }
}

impl RustTalkApp {
    /// Main view while connected: sidebar + content (or fullscreen stream).
    fn ui_connected(&mut self, ctx: &egui::Context) {
        let cmd_tx = self.cmd_tx.clone().expect("connected");
        self.apply_chat_action(&cmd_tx);

        // Screen thread ended with an error (e.g. window closed).
        if self.screen_failed.swap(false, Ordering::Relaxed) && self.screen.is_some() {
            self.screen = None;
            self.after_source_stopped(VideoKind::Screen, &cmd_tx);
        }
        // No talking/sharing in the lobby: stop running sources when we land there.
        if self.in_lobby() {
            if self.cam.as_ref().is_some_and(|c| c.is_sending()) {
                self.cam = None;
                self.after_source_stopped(VideoKind::Camera, &cmd_tx);
            }
            if self.screen.is_some() {
                self.screen = None;
                self.after_source_stopped(VideoKind::Screen, &cmd_tx);
            }
        }

        // Fullscreen: only the stream + member bar. Ends when the stream is gone.
        if let Some(key) = self.fullscreen {
            if self.video_frames.contains_key(&key) {
                self.ui_fullscreen(ctx, key);
                return;
            }
            self.leave_fullscreen();
        }

        self.ui_sidebar(ctx, &cmd_tx);
        self.ui_main(ctx, &cmd_tx);
        // Toast only without an open dialog (it would cover dialog buttons).
        let dialog_open = self.channel_settings.is_some()
            || self.server_settings_dialog.is_some()
            || self.show_new_channel
            || self.confirm_delete.is_some()
            || self.confirm_delete_msg.is_some()
            || self.cam_dialog.is_some()
            || self.screen_picker.is_some();
        if !dialog_open {
            theme::toast(ctx, &self.status, self.status_at.elapsed().as_secs_f32(), self.status_is_error);
        }
    }

    /// Debug builds only: env-controlled shortcuts for UI screenshots/tests.
    /// `RUSTTALK_DEV_AUTOCONNECT=1` connects at start,
    /// `RUSTTALK_DEV_OPEN=settings:<page>|channel:<name>|new_channel|select:<name>`.
    #[cfg(debug_assertions)]
    fn dev_hooks(&mut self, _ctx: &egui::Context) {
        use std::sync::atomic::AtomicU8;
        static STAGE: AtomicU8 = AtomicU8::new(0);
        match STAGE.load(Ordering::Relaxed) {
            0 => {
                STAGE.store(1, Ordering::Relaxed);
                if std::env::var("RUSTTALK_DEV_AUTOCONNECT").is_ok() {
                    self.start_connect();
                }
            }
            1 if self.connected || std::env::var("RUSTTALK_DEV_AUTOCONNECT").is_err() => {
                STAGE.store(2, Ordering::Relaxed);
                let Ok(open) = std::env::var("RUSTTALK_DEV_OPEN") else { return };
                for part in open.split(',') {
                    let (what, arg) = part.split_once(':').unwrap_or((part, ""));
                    let by_name = |s: &Self| s.channels.values().find(|c| c.name == arg).map(|c| c.id);
                    match what {
                        "settings" => {
                            let page = match arg {
                                "account" => SettingsPage::Account,
                                "appearance" => SettingsPage::Appearance,
                                "video" => SettingsPage::Video,
                                "keybinds" => SettingsPage::Keybinds,
                                "notifications" => SettingsPage::Notifications,
                                "advanced" => SettingsPage::Advanced,
                                "about" => SettingsPage::About,
                                _ => SettingsPage::Voice,
                            };
                            self.open_settings(page);
                        }
                        "channel" => {
                            if let Some(id) = by_name(self) {
                                self.open_channel_settings(id);
                            }
                        }
                        "select" => {
                            if let Some(id) = by_name(self) {
                                self.select_channel(id);
                            }
                        }
                        "join" => {
                            if let (Some(id), Some(tx)) = (by_name(self), self.cmd_tx.clone()) {
                                let _ = tx.send(Command::JoinChannel(id));
                                self.select_channel(id);
                            }
                        }
                        "new_channel" => self.open_new_channel(None, ChannelKind::Text),
                        "gif" => self.open_gif_picker(),
                        // Message actions on the last own / last other message of the selected channel.
                        "edit" | "delete" | "reply" => {
                            let want_own = what != "reply";
                            let id = self.selected_channel.and_then(|ch| self.chat.get(&ch)).and_then(|v| {
                                v.iter()
                                    .rev()
                                    .find(|l| l.id != 0 && matches!(l.kind, ChatKind::Message { own, .. } if own == want_own))
                                    .map(|l| l.id)
                            });
                            self.chat_action = id.map(|id| match what {
                                "edit" => ChatAction::Edit(id),
                                "delete" => ChatAction::Delete(id),
                                _ => ChatAction::Reply(id),
                            });
                        }
                        "server" => self.open_server_settings(match arg {
                            "afk" => server_settings::ServerTab::Afk,
                            "messages" => server_settings::ServerTab::Messages,
                            "integrations" => server_settings::ServerTab::Integrations,
                            _ => server_settings::ServerTab::Overview,
                        }),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

/// Unix timestamp -> "HH:MM" (UTC; good enough for now).
fn format_time(ts: u64) -> String {
    let secs = ts % 86_400;
    format!("{:02}:{:02}", secs / 3600, (secs / 60) % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_detection() {
        assert!(mentions("hey @anna!", "Anna"));
        assert!(mentions("@ANNA", "anna"));
        assert!(mentions("@Max Power look", "Max Power"), "names with spaces");
        assert!(!mentions("@annabelle hi", "anna"), "longer name");
        assert!(mentions("@annabelle and @anna", "anna"), "second occurrence");
        assert!(!mentions("anna without at", "anna"));
        assert!(!mentions("@", ""));
        assert_eq!(mention_prefix("hi @an"), Some("an"));
        assert_eq!(mention_prefix("@"), Some(""));
        assert_eq!(mention_prefix("hi @anna "), None, "finished with a space");
        assert_eq!(mention_prefix("mail@x"), None);
    }

    #[test]
    fn reconnect_backoff() {
        let secs: Vec<u64> = (1..=8).map(|a| reconnect_delay(a).as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 4, 8, 15, 30, 30, 30]);
        assert_eq!(reconnect_delay(0).as_secs(), 1);
        assert_eq!(reconnect_delay(1000).as_secs(), 30);
    }

    #[test]
    fn time_formatting() {
        assert_eq!(format_time(0), "00:00");
        assert_eq!(format_time(3_661), "01:01");
        assert_eq!(format_time(86_399), "23:59");
        assert_eq!(format_time(90_061), "01:01"); // next day (UTC)
    }

}
