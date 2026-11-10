//! egui-Oberflaeche: Discord-inspiriertes Layout (dunkel) mit Phosphor-Icons.
//!
//! Channel-Arten (siehe Protokoll v2):
//! - **Spawn** (Haus-Icon, oben gepinnt): Join-Log, kein Sprechen.
//! - **Kategorie** (Ordner/Caret): aufklappbare Gruppe, kein Join/Chat.
//! - **Sprache** (Speaker-Icon): Klick = beitreten; Mitte = Belegung + Aktivitaet.
//! - **Text** (Hash-Icon): Klick = Chat oeffnen.
//!
//! Kontextmenues per Rechtsklick auf Channels und User.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use rusttalk_shared::protocol::{
    ChannelInfo, ChannelKind, ServerMessage, UserInfo, VideoKind, MAX_MEDIA_BYTES,
};

use crate::audio::{capture, playback, AudioControl, AudioEngine, LocalMuteSet, TalkingMap};
use crate::config::{ClientConfig, ServerBookmark};
use crate::logo;
use crate::media::{self, TextureCache};
use crate::net::{self, Command, NetEvent};
use crate::video::{self, CameraDevice, ShareEntry, ShareSource, VideoCapture};

// Phosphor-Icons (Regular-Variante) als Fallback-Font.
mod icons {
    pub use egui_phosphor::regular;
}

// Discord-inspirierte Farben (immer Dark-Mode; erhoehter Kontrast).
const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(0x31, 0x33, 0x38);
const COLOR_SIDEBAR: egui::Color32 = egui::Color32::from_rgb(0x2B, 0x2D, 0x31);
const COLOR_DARK: egui::Color32 = egui::Color32::from_rgb(0x1E, 0x1F, 0x22);
const COLOR_ACCENT: egui::Color32 = egui::Color32::from_rgb(0x58, 0x65, 0xF2);
const COLOR_TALKING: egui::Color32 = egui::Color32::from_rgb(0x23, 0xA5, 0x5A);
const COLOR_IDLE: egui::Color32 = egui::Color32::from_rgb(0xA3, 0xA8, 0xB2);
const COLOR_TEXT: egui::Color32 = egui::Color32::from_rgb(0xE3, 0xE5, 0xE8);
const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(0xF2, 0x3F, 0x43);
const COLOR_HOVER: egui::Color32 = egui::Color32::from_rgb(0x3A, 0x3C, 0x41);

/// Max. Video-Bytes in der eigenen Sende-Queue (~1 s Bildschirm-Stream).
const MAX_UPLOAD_BACKLOG: usize = 3 * 1024 * 1024;

/// Avatar-Farbpalette (Name -> deterministische Farbe).
const AVATAR_PALETTE: [egui::Color32; 8] = [
    egui::Color32::from_rgb(0x58, 0x65, 0xF2),
    egui::Color32::from_rgb(0x57, 0xF2, 0x87),
    egui::Color32::from_rgb(0xFE, 0xE7, 0x5C),
    egui::Color32::from_rgb(0xEB, 0x45, 0x95),
    egui::Color32::from_rgb(0xED, 0x42, 0x45),
    egui::Color32::from_rgb(0xF5, 0x77, 0x18),
    egui::Color32::from_rgb(0x9B, 0x84, 0xEE),
    egui::Color32::from_rgb(0x24, 0xB5, 0xA5),
];

enum ChatKind {
    Message { from: String, own: bool },
    System,
}

struct ChatLine {
    kind: ChatKind,
    text: String,
    timestamp: u64,
    /// Optionales Bild (PNG/JPEG/GIF-Bytes).
    image: Option<Vec<u8>>,
}

/// Zustand des GIPHY-Fensters.
#[derive(Default)]
struct GiphyState {
    open: bool,
    query: String,
    results: Vec<media::GifResult>,
    loading: bool,
    error: Option<String>,
    /// Bereits angefragte Vorschau-URLs (gegen Doppel-Downloads).
    requested: HashSet<String>,
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

/// Nachrichten aus Hintergrund-Threads (GIPHY-Suche/Downloads).
enum GiphyMsg {
    Results(Result<Vec<media::GifResult>, String>),
    Preview(String, Vec<u8>),
    Full(Vec<u8>),
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
    new_channel_name: String,
    new_channel_kind: ChannelKind,
    new_channel_parent: Option<u64>,
    show_new_channel: bool,
    show_settings: bool,
    renaming_channel: Option<(u64, String)>,
    status: String,
    input_devices: Vec<String>,
    output_devices: Vec<String>,
    mute_before_deafen: bool,

    // Audio
    audio: Option<AudioEngine>,
    control: Arc<AudioControl>,
    talking: TalkingMap,
    local_muted: LocalMuteSet,
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
    giphy: GiphyState,
    giphy_tx: std::sync::mpsc::Sender<GiphyMsg>,
    giphy_rx: std::sync::mpsc::Receiver<GiphyMsg>,
}

impl RustTalkApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Immer Dark-Mode (nur ein Theme, bewusst).
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = COLOR_BG;
        visuals.window_fill = COLOR_BG;
        visuals.extreme_bg_color = COLOR_DARK;
        visuals.faint_bg_color = COLOR_SIDEBAR;
        visuals.hyperlink_color = COLOR_ACCENT;
        // Kontrast erhoehen (Kritik: schwache Texte zu dunkel):
        // egui 0.31 berechnet weak_text_color = gray_out(text_color), wobei
        // gray_out Richtung weak_bg_fill mischt -> beide Stellschrauben heller.
        visuals.widgets.noninteractive.fg_stroke.color = COLOR_TEXT;
        visuals.widgets.noninteractive.weak_bg_fill =
            egui::Color32::from_rgb(0x6E, 0x73, 0x7E);
        cc.egui_ctx.set_visuals(visuals);

        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        cc.egui_ctx.set_fonts(fonts);

        let config = ClientConfig::load();
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let (giphy_tx, giphy_rx) = std::sync::mpsc::channel::<GiphyMsg>();
        Self {
            control: Arc::new(AudioControl::new(config.voice_activation, config.vad_threshold)),
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
            new_channel_name: String::new(),
            new_channel_kind: ChannelKind::Text,
            new_channel_parent: None,
            show_new_channel: false,
            show_settings: false,
            renaming_channel: None,
            status: String::new(),
            input_devices: Vec::new(),
            output_devices: Vec::new(),
            mute_before_deafen: false,
            audio: None,
            talking: Arc::new(Mutex::new(HashMap::new())),
            local_muted: Arc::new(Mutex::new(HashSet::new())),
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
            giphy: GiphyState::default(),
            giphy_tx: giphy_tx.clone(),
            giphy_rx,
        }
    }

    fn is_connected(&self) -> bool {
        self.cmd_tx.is_some()
    }

    fn spawn_channel_id(&self) -> Option<u64> {
        self.channels.values().find(|c| c.kind == ChannelKind::Spawn).map(|c| c.id)
    }

    fn start_connect(&mut self) {
        self.status = "Verbinde...".to_string();
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
        self.audio = None; // stoppt die Audio-Threads
        self.users.clear();
        self.channels.clear();
        self.chat.clear();
        self.unread.clear();
        self.selected_channel = None;
        self.talking.lock().unwrap().clear();
        self.local_muted.lock().unwrap().clear();
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
            .push(ChatLine { kind: ChatKind::System, text, timestamp, image: None });
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

    fn select_channel(&mut self, id: u64) {
        self.selected_channel = Some(id);
        self.unread.remove(&id);
    }

    fn handle_event(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Connected { my_id, users, channels, udp_socket, udp_token, spawn_log, chat_history } => {
                self.my_id = my_id;
                self.channels = channels.into_iter().map(|c| (c.id, c)).collect();
                self.users = users.into_iter().map(|u| (u.id, u)).collect();
                // Spawn-Log in die Spawn-Ansicht laden.
                if let Some(spawn_id) = self.spawn_channel_id() {
                    let lines = spawn_log
                        .into_iter()
                        .map(|e| ChatLine {
                            kind: ChatKind::System,
                            text: e.text,
                            timestamp: e.timestamp,
                            image: None,
                        })
                        .collect();
                    self.chat.insert(spawn_id, lines);
                    self.selected_channel = Some(spawn_id);
                }
                // Persistierten Chat-Verlauf in die Channels laden
                // ("eigene" Nachrichten per Name erkannt – kein Account-System).
                for rec in chat_history {
                    let own = rec.from_name == self.config.username;
                    self.chat.entry(rec.channel_id).or_default().push(ChatLine {
                        kind: ChatKind::Message { from: rec.from_name, own },
                        text: rec.text,
                        timestamp: rec.timestamp,
                        image: rec.image,
                    });
                }
                self.status = format!("Verbunden mit {}", self.config.server_address);
                if let Some(sock) = udp_socket {
                    match AudioEngine::start(
                        sock,
                        udp_token,
                        self.control.clone(),
                        self.talking.clone(),
                        self.local_muted.clone(),
                        self.config.input_device.as_deref(),
                        self.config.output_device.as_deref(),
                    ) {
                        Ok(engine) => self.audio = Some(engine),
                        Err(e) => {
                            self.status = format!("Audio-Fehler: {e:#} (Chat funktioniert trotzdem)")
                        }
                    }
                }
            }
            NetEvent::Message(msg) => self.handle_server_message(msg),
            NetEvent::Status(text) => self.status = text,
            NetEvent::Disconnected { reason } => {
                self.disconnect();
                self.status = format!("Getrennt: {reason}");
            }
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
                self.users.insert(user.id, user);
                // Join-Zeile kommt serverseitig via SpawnLogEntry.
            }
            ServerMessage::UserLeft { client_id } => {
                self.users.remove(&client_id);
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
                        format!("Du bist jetzt in #{}", self.channel_name(channel_id))
                    } else {
                        format!("{} -> #{}", name, self.channel_name(channel_id))
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
            ServerMessage::SpawnLogEntry { entry } => {
                if let Some(spawn_id) = self.spawn_channel_id() {
                    self.system_line(spawn_id, entry.text, entry.timestamp);
                }
            }
            ServerMessage::Chat { channel_id, from_id, from_name, text, timestamp, image } => {
                let own = from_id == self.my_id;
                self.chat
                    .entry(channel_id)
                    .or_default()
                    .push(ChatLine {
                        kind: ChatKind::Message { from: from_name, own },
                        text,
                        timestamp,
                        image,
                    });
                if self.selected_channel != Some(channel_id) {
                    self.unread.insert(channel_id);
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
            ServerMessage::Error { message } => {
                self.status = format!("Server: {message}");
            }
        }
    }
}

impl eframe::App for RustTalkApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(ev) = self.event_rx.try_recv() {
            self.handle_event(ev);
        }

        // PTT-Taste pollen (solange das Fenster Fokus hat).
        if self.is_connected() {
            let pressed =
                key_from_name(&self.config.ptt_key).is_some_and(|k| ctx.input(|i| i.key_down(k)));
            self.control.ptt_pressed.store(pressed, Ordering::Relaxed);
            self.ui_connected(ctx);
        } else {
            self.ui_connect(ctx);
        }
        if self.show_settings {
            self.ui_settings(ctx);
        }
        self.ui_rename(ctx);
        self.ui_new_channel(ctx);
        self.ui_giphy(ctx);
        self.ui_camera_dialog(ctx);
        self.ui_screen_picker(ctx);

        // Mikro-Monitor fuer den Pegel-Test in den Einstellungen (ohne Verbindung).
        if self.show_settings && self.audio.is_none() && self.mic_monitor.is_none() {
            self.mic_monitor =
                capture::MicMonitor::start(self.config.input_device.as_deref()).ok();
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

// ---------------------------------------------------------------------------
// UI-Teile
// ---------------------------------------------------------------------------

impl RustTalkApp {
    /// Verbindungsbildschirm (nicht verbunden): Logo, Lesezeichen, Formular.
    fn ui_connect(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(46.0);
                logo::paint_logo(ui, 74.0);
                ui.add_space(8.0);
                ui.heading(egui::RichText::new("RustTalk").size(32.0).color(COLOR_TEXT));
                ui.weak("Schlanker Voice-Chat. Lokal. Ohne Accounts.");
            });
            ui.add_space(22.0);

            ui.horizontal(|ui| {
                let avail = ui.available_width();
                let content_w = 640.0f32.min(avail);
                ui.add_space(((avail - content_w) / 2.0).max(0.0));

                // --- Links: Lesezeichen ---
                // WICHTIG: allocate_ui erbt das Layout des Eltern-Ui (hier:
                // horizontal) -> explizit top_down setzen, sonst fliessen die
                // Inhalte nebeneinander statt untereinander.
                ui.allocate_ui_with_layout(
                    egui::vec2(220.0, 260.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                    egui::Frame::default()
                        .fill(COLOR_SIDEBAR)
                        .inner_margin(egui::Margin::same(12))
                        .rounding(8.0)
                        .show(ui, |ui| {
                            ui.set_height(236.0);
                            ui.label(
                                egui::RichText::new(format!("{}  LESEZEICHEN", icons::regular::BOOKMARK))
                                    .strong()
                                    .size(12.0)
                                    .color(COLOR_IDLE),
                            );
                            ui.separator();
                            let bookmarks = self.config.bookmarks.clone();
                            let mut remove_idx: Option<usize> = None;
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                if bookmarks.is_empty() {
                                    ui.weak("Noch keine Lesezeichen.\nKlick = ausfuellen\nDoppelklick = verbinden");
                                }
                                for (i, bm) in bookmarks.iter().enumerate() {
                                    let resp = ui
                                        .selectable_label(
                                            false,
                                            egui::RichText::new(&bm.name).color(COLOR_TEXT),
                                        )
                                        .on_hover_text(&bm.address);
                                    if resp.clicked() {
                                        self.config.server_address = bm.address.clone();
                                        self.config.server_password = bm.password.clone();
                                    }
                                    if resp.double_clicked() {
                                        self.config.server_address = bm.address.clone();
                                        self.config.server_password = bm.password.clone();
                                        self.start_connect();
                                    }
                                    resp.context_menu(|ui| {
                                        if ui
                                            .button(egui::RichText::new(format!(
                                                "{}  Loeschen",
                                                icons::regular::TRASH
                                            )).color(COLOR_MUTED))
                                            .clicked()
                                        {
                                            remove_idx = Some(i);
                                            ui.close_menu();
                                        }
                                    });
                                }
                            });
                            if let Some(i) = remove_idx {
                                self.config.bookmarks.remove(i);
                                self.config.save();
                            }
                        });
                    },
                );

                ui.add_space(12.0);

                // --- Rechts: Verbindungs-Formular ---
                ui.allocate_ui_with_layout(
                    egui::vec2(408.0, 260.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                    egui::Frame::default()
                        .fill(COLOR_SIDEBAR)
                        .inner_margin(egui::Margin::same(14))
                        .rounding(8.0)
                        .show(ui, |ui| {
                            // WICHTIG: keine unendlichen Wunschbreiten in
                            // fest dimensionierten Sub-UIs (egui-Panic).
                            let w = ui.available_width();
                            let input_h = ui.spacing().interact_size.y;

                            ui.label(egui::RichText::new("Server-Adresse").strong().color(COLOR_TEXT));
                            ui.add_sized(
                                [w, input_h],
                                egui::TextEdit::singleline(&mut self.config.server_address)
                                    .hint_text("host oder host:port"),
                            );
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new("Benutzername").strong().color(COLOR_TEXT));
                            ui.add_sized(
                                [w, input_h],
                                egui::TextEdit::singleline(&mut self.config.username),
                            );
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new("Passwort (optional)").strong().color(COLOR_TEXT));
                            let mut pw = self.config.server_password.clone().unwrap_or_default();
                            if ui
                                .add_sized(
                                    [w, input_h],
                                    egui::TextEdit::singleline(&mut pw).password(true),
                                )
                                .changed()
                            {
                                self.config.server_password =
                                    if pw.is_empty() { None } else { Some(pw) };
                            }
                            ui.add_space(12.0);
                            let connect = ui.add_sized(
                                [w, 32.0],
                                egui::Button::new(
                                    egui::RichText::new(format!("{}  Verbinden", icons::regular::SIGN_IN))
                                        .size(15.0)
                                        .color(egui::Color32::WHITE),
                                )
                                .fill(COLOR_ACCENT),
                            );
                            if connect.clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                self.start_connect();
                            }
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                let save = ui.add(
                                    egui::Button::new(
                                        egui::RichText::new(format!(
                                            "{}  Als Lesezeichen speichern",
                                            icons::regular::BOOKMARK
                                        )).size(13.0),
                                    )
                                    .frame(false),
                                );
                                if save.clicked() {
                                    let addr = self.config.server_address.trim().to_string();
                                    if !addr.is_empty()
                                        && !self.config.bookmarks.iter().any(|b| b.address == addr)
                                    {
                                        self.config.bookmarks.push(ServerBookmark {
                                            name: addr.clone(),
                                            address: addr,
                                            password: self.config.server_password.clone(),
                                        });
                                        self.config.save();
                                    }
                                }
                                if ui
                                    .button(
                                        egui::RichText::new(format!("{}  Einstellungen", icons::regular::GEAR_SIX))
                                            .size(13.0),
                                    )
                                    .clicked()
                                {
                                    self.input_devices = capture::input_device_names();
                                    self.output_devices = playback::output_device_names();
                                    self.show_settings = true;
                                }
                            });
                        });
                    },
                );
            });

            if !self.status.is_empty() {
                ui.add_space(14.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(&self.status).color(COLOR_MUTED));
                });
            }
        });
    }

    /// Hauptansicht (verbunden).
    fn ui_connected(&mut self, ctx: &egui::Context) {
        let cmd_tx = self.cmd_tx.clone().expect("connected");

        // Bildschirm-Thread ist mit Fehler beendet (z. B. Fenster geschlossen).
        if self.screen_failed.swap(false, Ordering::Relaxed) && self.screen.is_some() {
            self.screen = None;
            self.after_source_stopped(VideoKind::Screen, &cmd_tx);
        }

        // Vollbild: nur Stream + User-Leiste. Endet, wenn der Stream weg ist.
        if let Some(key) = self.fullscreen {
            if self.video_frames.contains_key(&key) {
                self.ui_fullscreen(ctx, key);
                return;
            }
            self.leave_fullscreen();
        }

        // --- Bottom-Bar: eigener User + Controls ---
        egui::TopBottomPanel::bottom("bottom_bar")
            .frame(egui::Frame::default().fill(COLOR_DARK).inner_margin(egui::Margin::same(6)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let my_channel = self.users.get(&self.my_id).map(|u| u.channel_id);
                    let i_transmit = self.control.transmitting.load(Ordering::Relaxed);
                    draw_avatar(ui, &self.config.username, 26.0, i_transmit);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&self.config.username).strong().color(COLOR_TEXT));
                        ui.weak(match my_channel {
                            Some(id) => format!("#{}", self.channel_name(id)),
                            None => "nicht verbunden".to_string(),
                        });
                    });
                    ui.separator();

                    let muted = self.control.self_muted.load(Ordering::Relaxed);
                    let deafened = self.control.self_deafened.load(Ordering::Relaxed);

                    let mic_icon =
                        if muted { icons::regular::MICROPHONE_SLASH } else { icons::regular::MICROPHONE };
                    if icon_button(ui, mic_icon, muted, "Mikro an/aus").clicked() {
                        let new = !muted;
                        self.control.self_muted.store(new, Ordering::Relaxed);
                        let _ = cmd_tx.send(Command::SetMute { muted: new, deafened });
                    }
                    let deaf_icon =
                        if deafened { icons::regular::SPEAKER_SLASH } else { icons::regular::SPEAKER_HIGH };
                    if icon_button(ui, deaf_icon, deafened, "Ton an/aus (taub)").clicked() {
                        let new = !deafened;
                        if new {
                            self.mute_before_deafen =
                                self.control.self_muted.load(Ordering::Relaxed);
                            self.control.self_muted.store(true, Ordering::Relaxed);
                        } else {
                            self.control
                                .self_muted
                                .store(self.mute_before_deafen, Ordering::Relaxed);
                        }
                        self.control.self_deafened.store(new, Ordering::Relaxed);
                        let muted = self.control.self_muted.load(Ordering::Relaxed);
                        let _ = cmd_tx.send(Command::SetMute { muted, deafened: new });
                    }
                    ui.separator();
                    // Kamera / Bildschirm: Klick oeffnet Vorschau bzw. Auswahl,
                    // waehrend des Teilens beendet ein Klick das Teilen.
                    let cam_live = self.cam.as_ref().is_some_and(|c| c.is_sending());
                    let cam_tip = if cam_live { "Kamera ausschalten" } else { "Kamera (mit Vorschau)" };
                    if share_button(ui, icons::regular::VIDEO_CAMERA, cam_live, cam_tip).clicked() {
                        self.toggle_cam(&cmd_tx);
                    }
                    let screen_live = self.screen.is_some();
                    let screen_tip =
                        if screen_live { "Teilen beenden" } else { "Bildschirm / Fenster teilen" };
                    if share_button(ui, icons::regular::MONITOR, screen_live, screen_tip).clicked() {
                        self.toggle_screen(&cmd_tx);
                    }
                    ui.separator();
                    if icon_button(ui, icons::regular::GEAR_SIX, false, "Einstellungen").clicked() {
                        self.input_devices = capture::input_device_names();
                        self.output_devices = playback::output_device_names();
                        self.show_settings = true;
                    }
                    if icon_button(ui, icons::regular::PHONE_DISCONNECT, true, "Trennen").clicked() {
                        self.disconnect();
                        return;
                    }
                    ui.separator();
                    let mode = if self.config.voice_activation {
                        format!("{} Sprachaktivierung", icons::regular::MICROPHONE)
                    } else {
                        format!("PTT: [{}] halten", self.config.ptt_key)
                    };
                    ui.weak(mode);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.weak(&self.status);
                    });
                });
            });

        // --- Sidebar: Spawn, Channels-Baum, User ---
        egui::SidePanel::left("sidebar")
            .default_width(240.0)
            .frame(egui::Frame::default().fill(COLOR_SIDEBAR).inner_margin(egui::Margin::same(8)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(icons::regular::HEADSET).size(20.0).color(COLOR_ACCENT));
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("RustTalk").strong().size(15.0).color(COLOR_TEXT));
                        ui.weak(&self.config.server_address);
                    });
                });
                ui.separator();

                // Snapshots gegen Borrow-Konflikte in Closures.
                let mut channels: Vec<ChannelInfo> = self.channels.values().cloned().collect();
                channels.sort_by_key(|c| c.id);
                let users = self.users.clone();
                let talking: HashMap<u64, Instant> = self.talking.lock().unwrap().clone();
                let local_muted: HashSet<u64> = self.local_muted.lock().unwrap().clone();
                let unread = self.unread.clone();
                let collapsed = self.collapsed.clone();
                let my_channel = self.users.get(&self.my_id).map(|u| u.channel_id);

                egui::ScrollArea::vertical().show(ui, |ui| {
                    // Spawn oben gepinnt.
                    for ch in channels.iter().filter(|c| c.kind == ChannelKind::Spawn) {
                        self.channel_row(ui, ch, &users, &talking, &local_muted, &unread, my_channel, &cmd_tx, 0);
                    }

                    // Top-Level Text/Voice (ohne Kategorie).
                    let top_level: Vec<&ChannelInfo> = channels
                        .iter()
                        .filter(|c| !matches!(c.kind, ChannelKind::Spawn | ChannelKind::Category) && c.parent.is_none())
                        .collect();
                    if !top_level.is_empty() {
                        self.section_header(ui, "CHANNELS");
                        for ch in top_level {
                            self.channel_row(ui, ch, &users, &talking, &local_muted, &unread, my_channel, &cmd_tx, 0);
                        }
                    }

                    // Kategorien mit Kindern.
                    for cat in channels.iter().filter(|c| c.kind == ChannelKind::Category) {
                        let is_collapsed = collapsed.get(&cat.id).copied().unwrap_or(false);
                        let caret = if is_collapsed {
                            icons::regular::CARET_RIGHT
                        } else {
                            icons::regular::CARET_DOWN
                        };
                        let has_unread_child = channels
                            .iter()
                            .any(|c| c.parent == Some(cat.id) && unread.contains(&c.id));
                        // Kategorie-Header: manuell allozierte Zeile in voller
                        // Breite (egui 0.31 hat kein take_available_width).
                        let (rect, row) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 20.0),
                            egui::Sense::click(),
                        );
                        if ui.is_rect_visible(rect) {
                            let p = ui.painter();
                            let left = rect.left_center() + egui::vec2(2.0, 0.0);
                            p.text(
                                left,
                                egui::Align2::LEFT_CENTER,
                                caret,
                                egui::FontId::proportional(12.0),
                                COLOR_IDLE,
                            );
                            let text_col = if has_unread_child { COLOR_TEXT } else { COLOR_IDLE };
                            p.text(
                                left + egui::vec2(18.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                cat.name.to_uppercase(),
                                egui::FontId::proportional(12.0),
                                text_col,
                            );
                        }
                        if row.clicked() {
                            self.collapsed.insert(cat.id, !is_collapsed);
                        }
                        row.context_menu(|ui| {
                            self.channel_context_menu(ui, cat, &cmd_tx);
                        });
                        row.on_hover_text("Klick: ein-/ausklappen  |  Rechtsklick: Menue");

                        if !is_collapsed {
                            for ch in channels.iter().filter(|c| c.parent == Some(cat.id)) {
                                self.channel_row(ui, ch, &users, &talking, &local_muted, &unread, my_channel, &cmd_tx, 1);
                            }
                        }
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new(format!("{}  Channel", icons::regular::PLUS)).size(13.0),
                        ).frame(false))
                        .clicked()
                    {
                        self.show_new_channel = true;
                        self.new_channel_parent = None;
                    }
                    ui.weak("Rechtsklick = Menue");
                });
            });

        // --- Mitte: je nach Channel-Art ---
        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(ch_id) = self.selected_channel else {
                ui.centered_and_justified(|ui| {
                    ui.weak("Kein Channel ausgewaehlt");
                });
                return;
            };
            match self.channel_kind(ch_id) {
                Some(ChannelKind::Text) => self.ui_text_channel(ui, ch_id, &cmd_tx),
                Some(ChannelKind::Spawn) => self.ui_spawn_channel(ui, ch_id),
                Some(ChannelKind::Voice) => self.ui_voice_channel(ui, ch_id, &cmd_tx),
                Some(ChannelKind::Category) => {
                    ui.centered_and_justified(|ui| {
                        ui.weak("Kategorien haben keinen Inhalt.");
                    });
                }
                None => {
                    ui.centered_and_justified(|ui| {
                        ui.weak("Channel unbekannt.");
                    });
                }
            }
        });
    }

    /// Sektions-Ueberschrift mit +-Button.
    fn section_header(&mut self, ui: &mut egui::Ui, label: &str) {
        ui.horizontal(|ui| {
            ui.weak(label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(egui::RichText::new(icons::regular::PLUS)).frame(false))
                    .on_hover_text("Channel erstellen")
                    .clicked()
                {
                    self.show_new_channel = true;
                }
            });
        });
        ui.add_space(2.0);
    }

    /// Eine Channel-Zeile im Baum.
    #[allow(clippy::too_many_arguments)]
    fn channel_row(
        &mut self,
        ui: &mut egui::Ui,
        ch: &ChannelInfo,
        users: &HashMap<u64, UserInfo>,
        talking: &HashMap<u64, Instant>,
        local_muted: &HashSet<u64>,
        unread: &HashSet<u64>,
        my_channel: Option<u64>,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
        indent: usize,
    ) {
        let selected = self.selected_channel == Some(ch.id);
        let i_am_here = my_channel == Some(ch.id);
        let has_unread = unread.contains(&ch.id);

        let icon = match ch.kind {
            ChannelKind::Spawn => icons::regular::HOUSE,
            ChannelKind::Text => icons::regular::HASH,
            ChannelKind::Voice => icons::regular::SPEAKER_HIGH,
            ChannelKind::Category => icons::regular::FOLDER,
        };

        let frame = egui::Frame::default()
            .fill(if selected { COLOR_HOVER } else { egui::Color32::TRANSPARENT })
            .rounding(4.0)
            .inner_margin(egui::Margin::symmetric(6, 3));
        let row = frame
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if indent > 0 {
                        ui.add_space(14.0 * indent as f32);
                    }
                    ui.label(egui::RichText::new(icon).color(COLOR_IDLE).size(13.0));
                    let mut text = egui::RichText::new(&ch.name);
                    if i_am_here {
                        text = text.strong().color(COLOR_TALKING);
                    } else if has_unread {
                        text = text.strong().color(COLOR_TEXT);
                    } else {
                        text = text.color(COLOR_TEXT);
                    }
                    ui.label(text);
                    if i_am_here {
                        ui.weak("(Du)");
                    }
                    if has_unread && !selected {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 3.0, COLOR_ACCENT);
                    }
                });
            })
            .response
            .interact(egui::Sense::click());

        // Klick-Verhalten je nach Art:
        match ch.kind {
            ChannelKind::Voice => {
                // Discord-Verhalten: Klick auf Sprach-Channel = beitreten + anzeigen.
                if row.clicked() {
                    self.select_channel(ch.id);
                    let _ = cmd_tx.send(Command::JoinChannel(ch.id));
                }
            }
            ChannelKind::Text | ChannelKind::Spawn => {
                if row.clicked() {
                    self.select_channel(ch.id);
                }
            }
            ChannelKind::Category => {}
        }

        row.context_menu(|ui| {
            self.channel_context_menu(ui, ch, cmd_tx);
        });

        // User (nur Voice/Spawn)
        if ch.kind.can_join() {
            for uid in &ch.user_ids {
                let Some(user) = users.get(uid) else { continue };
                self.user_row(ui, user, talking, local_muted, indent);
            }
        }
        ui.add_space(2.0);
    }

    /// Kontextmenue fuer einen Channel (art-abhaengig).
    fn channel_context_menu(
        &mut self,
        ui: &mut egui::Ui,
        ch: &ChannelInfo,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        if ch.kind == ChannelKind::Voice
            && ui.button(format!("{}  Beitreten", icons::regular::SIGN_IN)).clicked()
        {
            let _ = cmd_tx.send(Command::JoinChannel(ch.id));
            ui.close_menu();
        }
        if ch.kind.has_chat()
            && ui.button(format!("{}  Chat oeffnen", icons::regular::CHAT_TEXT)).clicked()
        {
            self.select_channel(ch.id);
            ui.close_menu();
        }
        if ui
            .button(format!("{}  Umbenennen", icons::regular::PENCIL_SIMPLE))
            .clicked()
        {
            self.renaming_channel = Some((ch.id, ch.name.clone()));
            ui.close_menu();
        }
        // Spawn ist unloeschbar.
        if ch.kind != ChannelKind::Spawn
            && ui
                .button(
                    egui::RichText::new(format!("{}  Loeschen", icons::regular::TRASH))
                        .color(COLOR_MUTED),
                )
                .clicked()
        {
            let _ = cmd_tx.send(Command::DeleteChannel(ch.id));
            ui.close_menu();
        }
    }

    /// Eine User-Zeile: Avatar (mit Talking-Ring), Name, Status-Icons.
    fn user_row(
        &mut self,
        ui: &mut egui::Ui,
        user: &UserInfo,
        talking: &HashMap<u64, Instant>,
        local_muted: &HashSet<u64>,
        indent: usize,
    ) {
        let locally_muted = local_muted.contains(&user.id);
        let self_talking =
            user.id == self.my_id && self.control.transmitting.load(Ordering::Relaxed);
        let talking_now = talking
            .get(&user.id)
            .is_some_and(|t| t.elapsed() < Duration::from_millis(250))
            || self_talking;

        ui.horizontal(|ui| {
            ui.add_space(26.0 + 14.0 * indent as f32);
            draw_avatar(ui, &user.name, 20.0, talking_now);
            let mut label = egui::RichText::new(&user.name).color(COLOR_TEXT);
            if user.id == self.my_id {
                label = label.strong();
            }
            if locally_muted {
                label = label.color(COLOR_IDLE);
            }
            let resp = ui.add(egui::Label::new(label).sense(egui::Sense::click()));
            if user.id == self.my_id {
                ui.weak("(Du)");
            }
            if user.muted {
                ui.label(egui::RichText::new(icons::regular::MICROPHONE_SLASH).color(COLOR_MUTED).size(13.0));
            }
            if user.deafened {
                ui.label(egui::RichText::new(icons::regular::SPEAKER_SLASH).color(COLOR_MUTED).size(13.0));
            }
            if locally_muted {
                ui.weak("lokal stumm");
            }

            if user.id != self.my_id {
                resp.context_menu(|ui| {
                    let mute_label = if locally_muted {
                        format!("{}  Lokale Stummschaltung aufheben", icons::regular::CHECK)
                    } else {
                        format!("{}  Lokal stummschalten", icons::regular::SPEAKER_SLASH)
                    };
                    if ui.button(mute_label).clicked() {
                        let mut set = self.local_muted.lock().unwrap();
                        if locally_muted {
                            set.remove(&user.id);
                        } else {
                            set.insert(user.id);
                        }
                        ui.close_menu();
                    }
                });
            }
        });
    }

    /// Mitte: Text-Channel (Chat).
    fn ui_text_channel(
        &mut self,
        ui: &mut egui::Ui,
        ch_id: u64,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        let ch_name = self.channel_name(ch_id);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(icons::regular::HASH).size(20.0).color(COLOR_IDLE));
            ui.heading(egui::RichText::new(&ch_name).color(COLOR_TEXT));
        });

        // Eingabezeile ZUERST als Bottom-Panel innerhalb der Mitte einrichten
        // (sonst schiebt die ScrollArea sie unter die Fensterkante und man
        // kann nicht schreiben).
        egui::TopBottomPanel::bottom(format!("chat_input_{ch_id}"))
            .frame(egui::Frame::default().inner_margin(egui::Margin::symmetric(0, 6)))
            .show_inside(ui, |ui| {
                ui.separator();
                self.chat_input_row(ui, ch_id, cmd_tx);
            });

        // Danach fuellt die ScrollArea nur noch den verbleibenden Platz.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| match self.chat.get(&ch_id) {
                Some(lines) if !lines.is_empty() => {
                    for line in lines {
                        chat_line(ui, line, &mut self.textures);
                    }
                }
                _ => {
                    ui.add_space(8.0);
                    ui.weak(format!("Willkommen in #{ch_name} – noch keine Nachrichten."));
                }
            });
    }

    /// Geteilte Chat-Eingabezeile: 4-zeilige Textbox links,
    /// rechts Buttons (Bild anhaengen, GIF, Senden). Wird von Text- und
    /// Voice-Channels genutzt.
    fn chat_input_row(
        &mut self,
        ui: &mut egui::Ui,
        ch_id: u64,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        // Angehaengtes Bild (Vorschau-Chip).
        if let Some((name, len_kb)) =
            self.pending_image.as_ref().map(|(n, b)| (n.clone(), b.len() / 1024))
        {
            let mut clear = false;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(icons::regular::IMAGE).color(COLOR_ACCENT));
                ui.label(egui::RichText::new(name).color(COLOR_TEXT));
                ui.weak(format!("{len_kb} KB"));
                if ui.small_button(icons::regular::X).clicked() {
                    clear = true;
                }
            });
            if clear {
                self.pending_image = None;
            }
        }

        let ch_name = self.channel_name(ch_id);
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::multiline(&mut self.chat_input)
                    .hint_text(format!("Nachricht an #{ch_name}..."))
                    .desired_rows(4)
                    .desired_width(f32::INFINITY),
            );
            // Enter = senden, Shift+Enter = neue Zeile.
            let enter = resp.has_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
            if enter {
                resp.request_focus();
            }
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button(egui::RichText::new(icons::regular::IMAGE).size(15.0))
                        .on_hover_text("Bild anhaengen")
                        .clicked()
                    {
                        self.pick_image();
                    }
                    let _ = ui
                        .add_enabled(
                            false,
                            egui::Button::new(egui::RichText::new(icons::regular::GIF).size(15.0)),
                        )
                        .on_hover_text("GIF-Suche (GIPHY): aktuell kaputt – Fix in Arbeit (WIP)");
                    let send = ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(icons::regular::PAPER_PLANE_TILT)
                                    .size(16.0)
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(COLOR_ACCENT),
                        )
                        .on_hover_text("Senden (Enter)")
                        .clicked();
                    if send || enter {
                        self.send_chat(ch_id, cmd_tx);
                    }
                });
            });
        });
    }

    /// Sendet Text + ggf. angehaengtes Bild.
    fn send_chat(&mut self, ch_id: u64, cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>) {
        let text = self.chat_input.trim().to_string();
        let image = self.pending_image.take().map(|(_, b)| b);
        if text.is_empty() && image.is_none() {
            return;
        }
        let _ = cmd_tx.send(Command::SendChat { channel_id: ch_id, text, image });
        self.chat_input.clear();
    }

    /// Bild-Datei waehlen und anhaengen.
    fn pick_image(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Bilder", &["png", "jpg", "jpeg", "gif", "webp"])
            .pick_file()
        {
            match std::fs::read(&path) {
                Ok(bytes) if bytes.len() <= MAX_MEDIA_BYTES => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "bild".into());
                    self.pending_image = Some((name, bytes));
                }
                Ok(_) => self.status = "Bild zu gross (max. 4 MiB)".into(),
                Err(e) => self.status = format!("Bild nicht lesbar: {e}"),
            }
        }
    }

    /// Stream im Vollbild zeigen (App-Fenster geht in den Vollbildmodus).
    fn enter_fullscreen(&mut self, key: (u64, VideoKind)) {
        self.fullscreen = Some(key);
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
    }

    fn leave_fullscreen(&mut self) {
        if self.fullscreen.take().is_some() {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }

    /// Vollbild: Stream fuellt das Fenster, unten die Mitglieder des Channels
    /// (gruener Punkt = spricht). Esc / Doppelklick / Button beendet.
    fn ui_fullscreen(&mut self, ctx: &egui::Context, key: (u64, VideoKind)) {
        let (uid, kind) = key;
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.leave_fullscreen();
            return;
        }
        let streamer = self.users.get(&uid).map(|u| u.name.clone()).unwrap_or_default();
        let members: Vec<UserInfo> = self
            .users
            .get(&uid)
            .and_then(|u| self.channels.get(&u.channel_id))
            .map(|c| c.user_ids.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| self.users.get(&id).cloned())
            .collect();
        let talking: HashMap<u64, Instant> = self.talking.lock().unwrap().clone();
        let self_transmitting = self.control.transmitting.load(Ordering::Relaxed);
        let mut switch_to: Option<(u64, VideoKind)> = None;
        let mut leave = false;

        // --- Unten: User-Leiste ---
        egui::TopBottomPanel::bottom("fs_users")
            .frame(
                egui::Frame::default().fill(COLOR_DARK).inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for user in &members {
                            let talking_now = talking
                                .get(&user.id)
                                .is_some_and(|t| t.elapsed() < Duration::from_millis(250))
                                || (user.id == self.my_id && self_transmitting);
                            ui.allocate_ui(egui::vec2(84.0, 86.0), |ui| {
                                ui.vertical_centered(|ui| {
                                    let r = draw_avatar(ui, &user.name, 40.0, talking_now);
                                    if talking_now {
                                        draw_talk_dot(ui, r);
                                    }
                                    let mut name = egui::RichText::new(&user.name)
                                        .size(12.0)
                                        .color(COLOR_TEXT);
                                    if user.id == uid {
                                        name = name.strong();
                                    }
                                    ui.label(name);
                                    ui.horizontal(|ui| {
                                        if user.muted {
                                            ui.label(
                                                egui::RichText::new(icons::regular::MICROPHONE_SLASH)
                                                    .color(COLOR_MUTED),
                                            );
                                        }
                                        if user.deafened {
                                            ui.label(
                                                egui::RichText::new(icons::regular::SPEAKER_SLASH)
                                                    .color(COLOR_MUTED),
                                            );
                                        }
                                        // Streams dieses Users: Klick wechselt die Ansicht.
                                        for k in [VideoKind::Screen, VideoKind::Camera] {
                                            if !self.video_frames.contains_key(&(user.id, k)) {
                                                continue;
                                            }
                                            let active = (user.id, k) == key;
                                            let btn = egui::Button::new(
                                                egui::RichText::new(kind_icon(k)).color(
                                                    if active { egui::Color32::WHITE } else { COLOR_IDLE },
                                                ),
                                            )
                                            .small()
                                            .fill(if active { COLOR_ACCENT } else { COLOR_HOVER });
                                            if ui
                                                .add(btn)
                                                .on_hover_text(format!(
                                                    "{} von {} ansehen",
                                                    kind_label(k),
                                                    user.name
                                                ))
                                                .clicked()
                                            {
                                                switch_to = Some((user.id, k));
                                            }
                                        }
                                    });
                                });
                            });
                        }
                    });
                });
            });

        // --- Mitte: Stream ---
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                if let Some(data) = self.video_frames.get(&key).cloned() {
                    if let Some(tex) =
                        self.textures.texture_for(ctx, &data, &format!("video-{uid}-{kind:?}"))
                    {
                        let fit = fit_size(tex.size_vec2(), rect.size());
                        ui.painter().image(
                            tex.id(),
                            egui::Rect::from_center_size(rect.center(), fit),
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                }
                let resp = ui.interact(rect, ui.id().with("fs_video"), egui::Sense::click());
                if resp.double_clicked() {
                    leave = true;
                }
            });

        // --- Overlays: Titel links oben, Beenden rechts oben ---
        let overlay = egui::Frame::default()
            .fill(egui::Color32::from_black_alpha(170))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(10, 6));
        egui::Area::new(egui::Id::new("fs_title"))
            .anchor(egui::Align2::LEFT_TOP, egui::vec2(12.0, 12.0))
            .show(ctx, |ui| {
                overlay.show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{}  {} – {}",
                            kind_icon(kind),
                            streamer,
                            kind_label(kind)
                        ))
                        .color(egui::Color32::WHITE),
                    );
                });
            });
        egui::Area::new(egui::Id::new("fs_exit"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 12.0))
            .show(ctx, |ui| {
                overlay.show(ui, |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(format!(
                                    "{}  Vollbild verlassen (Esc)",
                                    icons::regular::CORNERS_IN
                                ))
                                .color(egui::Color32::WHITE),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        leave = true;
                    }
                });
            });

        if leave {
            self.leave_fullscreen();
        } else if let Some(k) = switch_to {
            self.fullscreen = Some(k);
        }
    }

    /// Kamera-Button: live -> ausschalten, sonst Vorschau-Dialog oeffnen.
    fn toggle_cam(&mut self, cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>) {
        if self.cam.as_ref().is_some_and(|c| c.is_sending()) {
            self.cam = None;
            self.after_source_stopped(VideoKind::Camera, cmd_tx);
            return;
        }
        if self.cam_dialog.is_some() {
            return;
        }
        let (devices, error) = match video::list_cameras() {
            Ok(d) if d.is_empty() => (d, Some("Keine Kamera gefunden.".to_string())),
            Ok(d) => (d, None),
            Err(e) => (Vec::new(), Some(format!("{e:#}"))),
        };
        let selected = devices
            .iter()
            .find(|d| Some(&d.name) == self.config.camera.as_ref())
            .or(devices.first())
            .map(|d| d.index);
        self.cam_dialog = Some(CamDialog { devices, selected, error });
        if let Some(index) = selected {
            self.start_cam_preview(index, cmd_tx);
        }
    }

    /// Kamera im Vorschau-Modus starten (sendet noch nichts).
    fn start_cam_preview(
        &mut self,
        index: u32,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        self.cam = None; // alten Thread stoppen
        *self.local_preview.lock().unwrap() = None;
        if let Some(d) = &mut self.cam_dialog {
            d.error = None;
        }
        match video::start_camera(
            index,
            self.config.video_quality,
            self.make_frame_sink(VideoKind::Camera, cmd_tx),
            self.make_cam_error_sink(),
        ) {
            Ok(cap) => self.cam = Some(cap),
            Err(e) => {
                if let Some(d) = &mut self.cam_dialog {
                    d.error = Some(format!("{e:#}"));
                }
            }
        }
    }

    /// Bildschirm-Button: teilt -> beenden, sonst Auswahl-Dialog oeffnen.
    fn toggle_screen(&mut self, cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>) {
        if self.screen.is_some() {
            self.screen = None;
            self.after_source_stopped(VideoKind::Screen, cmd_tx);
            return;
        }
        if self.screen_picker.is_none() {
            self.screen_picker = Some(ScreenPicker::open());
        }
    }

    /// Ausgewaehlte Quelle teilen (eine laufende Kamera bleibt an).
    fn start_screen_share(
        &mut self,
        source: ShareSource,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        self.screen = None;
        // Erst Sharing melden, dann senden (Server relayed nur Sharer).
        let _ = cmd_tx.send(Command::SetSharing(true));
        match video::start_screen(
            source,
            self.config.video_quality,
            self.make_frame_sink(VideoKind::Screen, cmd_tx),
            self.make_screen_error_sink(),
        ) {
            Ok(cap) => self.screen = Some(cap),
            Err(e) => {
                self.status = format!("Bildschirm-Fehler: {e:#}");
                let still = self.cam.as_ref().is_some_and(|c| c.is_sending());
                let _ = cmd_tx.send(Command::SetSharing(still));
            }
        }
    }

    /// Nach dem Stoppen einer Quelle: Zuschauern den einzelnen Stream
    /// abmelden, eigene Kachel weg, Sharing-Status aktualisieren.
    fn after_source_stopped(
        &mut self,
        kind: VideoKind,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        // Reihenfolge wichtig: StopVideo vor SetSharing(false), sonst hat der
        // Server keine Relay-Ziele mehr.
        let _ = cmd_tx.send(Command::StopVideo(kind));
        let still = self.screen.is_some() || self.cam.as_ref().is_some_and(|c| c.is_sending());
        let _ = cmd_tx.send(Command::SetSharing(still));
        self.clear_video_of(self.my_id, Some(kind));
        if kind == VideoKind::Camera {
            *self.local_preview.lock().unwrap() = None;
            self.local_preview_tex = None;
        }
    }

    /// Video-Kacheln eines Users entfernen (eine Art oder alle).
    fn clear_video_of(&mut self, user_id: u64, kind: Option<VideoKind>) {
        for k in [VideoKind::Camera, VideoKind::Screen] {
            if kind.is_some_and(|want| want != k) {
                continue;
            }
            if let Some(old) = self.video_frames.remove(&(user_id, k)) {
                self.textures.remove_bytes(&old);
            }
        }
    }

    /// Frame-Senke fuer Capture-Threads. Kamera-Frames landen zusaetzlich in
    /// der lokalen Vorschau (Kamera-Dialog); nur wenn `sending` -> an den
    /// Server + eigene Kachel. Stoesst sofort ein Neuzeichnen an.
    fn make_frame_sink(
        &self,
        kind: VideoKind,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) -> impl FnMut(Vec<u8>, bool) + Send + 'static {
        let tx = cmd_tx.clone();
        let events = self.event_tx.clone();
        let preview = self.local_preview.clone();
        let ctx = self.ctx.clone();
        let backlog = self.upload_backlog.clone();
        let my_id = self.my_id;
        move |jpeg: Vec<u8>, sending: bool| {
            if kind == VideoKind::Camera {
                *preview.lock().unwrap() = Some(jpeg.clone());
            }
            if sending {
                // Upload staut (langsames WLAN o. ae.) -> Frame nicht senden,
                // statt Latenz und Speicher endlos wachsen zu lassen.
                let len = jpeg.len();
                if backlog.load(Ordering::Relaxed) + len <= MAX_UPLOAD_BACKLOG {
                    backlog.fetch_add(len, Ordering::Relaxed);
                    if tx.send(Command::VideoFrame(kind, jpeg.clone())).is_err() {
                        backlog.fetch_sub(len, Ordering::Relaxed);
                    }
                }
                let _ = events.send(NetEvent::Message(ServerMessage::VideoFrame {
                    from_id: my_id,
                    kind,
                    data: jpeg,
                }));
            }
            ctx.request_repaint();
        }
    }

    /// Fehler des Bildschirm-Threads: Thread ist beendet -> in `update`
    /// wird das Teilen sauber abgemeldet.
    fn make_screen_error_sink(&self) -> impl FnMut(String) + Send + 'static {
        let events = self.event_tx.clone();
        let failed = self.screen_failed.clone();
        move |msg: String| {
            failed.store(true, Ordering::Relaxed);
            let _ = events.send(NetEvent::Status(msg));
        }
    }

    /// Fehler des Kamera-Threads: Statuszeile + Anzeige im Kamera-Dialog.
    fn make_cam_error_sink(&self) -> impl FnMut(String) + Send + 'static {
        let events = self.event_tx.clone();
        let errors = self.cam_error.clone();
        move |msg: String| {
            *errors.lock().unwrap() = Some(msg.clone());
            let _ = events.send(NetEvent::Status(msg));
        }
    }

    /// Eigene Vorschau als Textur (eine Textur, wird pro Frame aktualisiert).
    fn local_preview_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let data = self.local_preview.lock().unwrap().clone()?;
        let key = media::hash_bytes(&data);
        if let Some((k, tex)) = &self.local_preview_tex {
            if *k == key {
                return Some(tex.clone());
            }
        }
        let img = image::load_from_memory(&data).ok()?.to_rgba8();
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            &img,
        );
        match &mut self.local_preview_tex {
            Some((k, tex)) => {
                tex.set(color, egui::TextureOptions::LINEAR);
                *k = key;
                Some(tex.clone())
            }
            None => {
                let tex = ctx.load_texture("local-preview", color, egui::TextureOptions::LINEAR);
                self.local_preview_tex = Some((key, tex.clone()));
                Some(tex)
            }
        }
    }

    /// Kamera-Dialog: Geraet waehlen, Live-Vorschau, dann "Kamera teilen".
    fn ui_camera_dialog(&mut self, ctx: &egui::Context) {
        if self.cam_dialog.is_none() {
            return;
        }
        let Some(cmd_tx) = self.cmd_tx.clone() else {
            self.cam_dialog = None;
            return;
        };
        // Fehler aus dem Kamera-Thread in den Dialog uebernehmen.
        if let Some(err) = self.cam_error.lock().unwrap().take() {
            if let Some(d) = &mut self.cam_dialog {
                d.error = Some(err);
            }
        }
        let preview = self.local_preview_texture(ctx);
        let mut open = true;
        let mut action: Option<bool> = None; // Some(true) = teilen, Some(false) = abbrechen
        let mut switch_to: Option<u32> = None;
        egui::Window::new(format!("{}  Kamera", icons::regular::VIDEO_CAMERA))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size([480.0, 360.0])
            .show(ctx, |ui| {
                let Some(dialog) = self.cam_dialog.as_mut() else { return };
                if !dialog.devices.is_empty() {
                    let current = dialog
                        .devices
                        .iter()
                        .find(|d| Some(d.index) == dialog.selected)
                        .map(|d| d.name.clone())
                        .unwrap_or_default();
                    egui::ComboBox::from_label("Kamera")
                        .selected_text(current)
                        .width(320.0)
                        .show_ui(ui, |ui| {
                            for d in &dialog.devices {
                                if ui
                                    .selectable_label(dialog.selected == Some(d.index), &d.name)
                                    .clicked()
                                    && dialog.selected != Some(d.index)
                                {
                                    dialog.selected = Some(d.index);
                                    switch_to = Some(d.index);
                                }
                            }
                        });
                }
                ui.add_space(8.0);
                let size = egui::vec2(480.0, 270.0);
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().rect_filled(rect, 8.0, COLOR_DARK);
                if let Some(tex) = &preview {
                    let fit = fit_size(tex.size_vec2(), size);
                    let img_rect = egui::Rect::from_center_size(rect.center(), fit);
                    egui::Image::new(tex).corner_radius(8.0).paint_at(ui, img_rect);
                } else {
                    let text = match &dialog.error {
                        Some(e) => e.clone(),
                        None => "Kamera startet...".to_string(),
                    };
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::proportional(14.0),
                        COLOR_IDLE,
                    );
                }
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            preview.is_some(),
                            egui::Button::new(
                                egui::RichText::new(format!(
                                    "{}  Kamera teilen",
                                    icons::regular::VIDEO_CAMERA
                                ))
                                .color(egui::Color32::WHITE),
                            )
                            .fill(COLOR_ACCENT),
                        )
                        .clicked()
                    {
                        action = Some(true);
                    }
                    if ui.button("Abbrechen").clicked() {
                        action = Some(false);
                    }
                });
            });
        if let Some(index) = switch_to {
            self.start_cam_preview(index, &cmd_tx);
        }
        match action {
            Some(true) => {
                if let Some(d) = &self.cam_dialog {
                    self.config.camera = d
                        .devices
                        .iter()
                        .find(|x| Some(x.index) == d.selected)
                        .map(|x| x.name.clone());
                    self.config.save();
                }
                // Bildschirm-Teilen laeuft ggf. parallel weiter.
                if let Some(cam) = &self.cam {
                    // Erst Sharing melden, dann senden (Server relayed nur Sharer).
                    let _ = cmd_tx.send(Command::SetSharing(true));
                    cam.set_sending(true);
                }
                self.cam_dialog = None;
                return;
            }
            Some(false) => open = false,
            None => {}
        }
        if !open {
            // Abbrechen: Vorschau-Kamera aus (eine live laufende bleibt).
            self.cam_dialog = None;
            if !self.cam.as_ref().is_some_and(|c| c.is_sending()) {
                self.cam = None;
                *self.local_preview.lock().unwrap() = None;
            }
        }
    }

    /// Bildschirm/Fenster-Auswahl mit Reitern "Anwendungen" / "Gesamter Bildschirm".
    fn ui_screen_picker(&mut self, ctx: &egui::Context) {
        let Some(cmd_tx) = self.cmd_tx.clone() else {
            self.screen_picker = None;
            return;
        };
        let Some(picker) = &mut self.screen_picker else { return };
        if let Ok(list) = picker.rx.try_recv() {
            picker.loading = false;
            picker.selected = None;
            picker.entries = list
                .into_iter()
                .enumerate()
                .map(|(i, e)| {
                    let tex = e.thumbnail.as_ref().map(|img| {
                        ctx.load_texture(
                            format!("share-thumb-{i}"),
                            egui::ColorImage::from_rgba_unmultiplied(
                                [img.width() as usize, img.height() as usize],
                                img,
                            ),
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    (e, tex)
                })
                .collect();
            if !picker.entries.iter().any(|(e, _)| matches!(e.source, ShareSource::Window(_))) {
                picker.tab = ShareTab::Screens;
            }
        }

        let mut open = true;
        let mut cancel = false;
        let mut share: Option<ShareSource> = None;
        let mut refresh = false;
        egui::Window::new(format!("{}  Bildschirm teilen", icons::regular::MONITOR))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size([700.0, 520.0])
            .show(ctx, |ui| {
                // Reiter
                ui.horizontal(|ui| {
                    let tab_w = (ui.available_width() - 8.0) / 2.0;
                    for (tab, icon, label) in [
                        (ShareTab::Apps, icons::regular::APP_WINDOW, "Anwendungen"),
                        (ShareTab::Screens, icons::regular::MONITOR, "Gesamter Bildschirm"),
                    ] {
                        let active = picker.tab == tab;
                        let btn = egui::Button::new(
                            egui::RichText::new(format!("{icon}  {label}"))
                                .size(15.0)
                                .color(if active { egui::Color32::WHITE } else { COLOR_IDLE }),
                        )
                        .fill(if active { COLOR_HOVER } else { COLOR_DARK })
                        .min_size(egui::vec2(tab_w, 34.0));
                        if ui.add(btn).clicked() && !active {
                            picker.tab = tab;
                            picker.selected = None;
                        }
                    }
                });
                ui.add_space(8.0);

                let list_h = 420.0;
                if picker.loading {
                    ui.allocate_ui(egui::vec2(ui.available_width(), list_h), |ui| {
                        ui.centered_and_justified(|ui| {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Fenster und Bildschirme werden gesucht...");
                            });
                        });
                    });
                } else {
                    let wanted_window = picker.tab == ShareTab::Apps;
                    egui::ScrollArea::vertical()
                        .max_height(list_h)
                        .min_scrolled_height(list_h)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let items: Vec<usize> = picker
                                .entries
                                .iter()
                                .enumerate()
                                .filter(|(_, (e, _))| {
                                    matches!(e.source, ShareSource::Window(_)) == wanted_window
                                })
                                .map(|(i, _)| i)
                                .collect();
                            if items.is_empty() {
                                ui.weak(if wanted_window {
                                    "Keine teilbaren Fenster gefunden."
                                } else {
                                    "Keine Bildschirme gefunden."
                                });
                            }
                            let card_w = (ui.available_width() - 12.0) / 2.0;
                            for row in items.chunks(2) {
                                ui.horizontal(|ui| {
                                    for &i in row {
                                        let (entry, tex) = &picker.entries[i];
                                        let selected = picker.selected == Some(entry.source);
                                        let resp =
                                            share_card(ui, entry, tex.as_ref(), selected, card_w);
                                        if resp.clicked() {
                                            picker.selected = Some(entry.source);
                                        }
                                        if resp.double_clicked() {
                                            share = Some(entry.source);
                                        }
                                    }
                                });
                                ui.add_space(10.0);
                            }
                        });
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !picker.loading,
                            egui::Button::new(format!(
                                "{}  Aktualisieren",
                                icons::regular::ARROWS_CLOCKWISE
                            )),
                        )
                        .clicked()
                    {
                        refresh = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                picker.selected.is_some(),
                                egui::Button::new(
                                    egui::RichText::new(format!(
                                        "{}  Teilen",
                                        icons::regular::BROADCAST
                                    ))
                                    .color(egui::Color32::WHITE),
                                )
                                .fill(COLOR_ACCENT),
                            )
                            .clicked()
                        {
                            share = picker.selected;
                        }
                        if ui.button("Abbrechen").clicked() {
                            cancel = true;
                        }
                    });
                });
            });

        if refresh {
            let tab = picker.tab;
            let mut fresh = ScreenPicker::open();
            fresh.tab = tab;
            *picker = fresh;
        }
        if let Some(source) = share {
            self.screen_picker = None;
            self.start_screen_share(source, &cmd_tx);
        } else if !open || cancel {
            self.screen_picker = None;
        }
    }

    /// Mitte: Spawn-Channel (Join-Log, kein Sprechen).
    fn ui_spawn_channel(&mut self, ui: &mut egui::Ui, ch_id: u64) {
        let ch_name = self.channel_name(ch_id);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(icons::regular::HOUSE).size(20.0).color(COLOR_IDLE));
            ui.heading(egui::RichText::new(&ch_name).color(COLOR_TEXT));
            ui.weak("Hier landet jeder – Sprechen verboten");
        });
        ui.separator();
        ui.label(egui::RichText::new("Verbindungs-Log").strong().color(COLOR_TEXT));
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| match self.chat.get(&ch_id) {
                Some(lines) if !lines.is_empty() => {
                    for line in lines {
                        chat_line(ui, line, &mut self.textures);
                    }
                }
                _ => {
                    ui.weak("Noch keine Eintraege.");
                }
            });
    }

    /// Mitte: Sprach-Channel (Belegung + Aktivitaet, kein Text-Input).
    fn ui_voice_channel(
        &mut self,
        ui: &mut egui::Ui,
        ch_id: u64,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        let ch_name = self.channel_name(ch_id);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(icons::regular::SPEAKER_HIGH).size(20.0).color(COLOR_IDLE));
            ui.heading(egui::RichText::new(&ch_name).color(COLOR_TEXT));
        });
        ui.separator();

        // Belegung als Avatar-Grid.
        let members: Vec<UserInfo> = self
            .channels
            .get(&ch_id)
            .map(|c| c.user_ids.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| self.users.get(&id).cloned())
            .collect();
        let i_am_here = self.users.get(&self.my_id).is_some_and(|u| u.channel_id == ch_id);
        let talking: HashMap<u64, Instant> = self.talking.lock().unwrap().clone();

        if members.is_empty() {
            ui.weak("Niemand in diesem Channel.");
        } else {
            let self_transmitting = self.control.transmitting.load(Ordering::Relaxed);
            ui.horizontal_wrapped(|ui| {
                for user in &members {
                    let talking_now = (talking
                        .get(&user.id)
                        .is_some_and(|t| t.elapsed() < Duration::from_millis(250)))
                        || (user.id == self.my_id && self_transmitting);
                    ui.vertical(|ui| {
                        ui.add_space(6.0);
                        draw_avatar(ui, &user.name, 44.0, talking_now);
                        let mut name = egui::RichText::new(&user.name).color(COLOR_TEXT);
                        if user.id == self.my_id {
                            name = name.strong();
                        }
                        ui.label(name);
                        ui.horizontal(|ui| {
                            if user.muted {
                                ui.label(egui::RichText::new(icons::regular::MICROPHONE_SLASH).color(COLOR_MUTED));
                            }
                            if user.deafened {
                                ui.label(egui::RichText::new(icons::regular::SPEAKER_SLASH).color(COLOR_MUTED));
                            }
                        });
                    });
                    ui.add_space(10.0);
                }
            });
        }
        if !i_am_here {
            ui.add_space(8.0);
            if ui
                .add(egui::Button::new(
                    egui::RichText::new(format!("{}  Beitreten", icons::regular::SIGN_IN))
                        .color(egui::Color32::WHITE),
                ).fill(COLOR_ACCENT))
                .clicked()
            {
                let _ = cmd_tx.send(Command::JoinChannel(ch_id));
            }
        }

        // Video-Grid: teilende Mitglieder (Kamera/Bildschirm) als Kacheln.
        // Pro User bis zu zwei Kacheln (Kamera + Bildschirm). Klick = Vollbild.
        let tiles: Vec<(UserInfo, VideoKind)> = members
            .iter()
            .flat_map(|u| {
                [VideoKind::Screen, VideoKind::Camera]
                    .into_iter()
                    .filter(|k| self.video_frames.contains_key(&(u.id, *k)))
                    .map(|k| (u.clone(), k))
                    .collect::<Vec<_>>()
            })
            .collect();
        if !tiles.is_empty() {
            ui.add_space(8.0);
            let mut open_fs = None;
            ui.horizontal_wrapped(|ui| {
                for (user, kind) in tiles {
                    let Some(data) = self.video_frames.get(&(user.id, kind)).cloned() else {
                        continue;
                    };
                    let name = format!("video-{}-{kind:?}", user.id);
                    let Some(tex) = self.textures.texture_for(ui.ctx(), &data, &name) else {
                        continue;
                    };
                    ui.vertical(|ui| {
                        let resp = ui
                            .add(
                                egui::Image::new(&tex)
                                    .max_width(320.0)
                                    .max_height(200.0)
                                    .corner_radius(6.0)
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_text("Klicken fuer Vollbild")
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.clicked() {
                            open_fs = Some((user.id, kind));
                        }
                        ui.label(
                            egui::RichText::new(format!(
                                "{}  {}",
                                kind_icon(kind),
                                user.name
                            ))
                            .color(COLOR_TEXT),
                        );
                    });
                    ui.add_space(8.0);
                }
            });
            if let Some(key) = open_fs {
                self.enter_fullscreen(key);
            }
            ui.separator();
        }

        // Chat-Feed (Nachrichten + Aktivitaet) mit Eingabezeile unten.
        egui::TopBottomPanel::bottom(format!("voice_input_{ch_id}"))
            .frame(egui::Frame::default().inner_margin(egui::Margin::symmetric(0, 6)))
            .show_inside(ui, |ui| {
                ui.separator();
                self.chat_input_row(ui, ch_id, cmd_tx);
            });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| match self.chat.get(&ch_id) {
                Some(lines) if !lines.is_empty() => {
                    for line in lines {
                        chat_line(ui, line, &mut self.textures);
                    }
                }
                _ => {
                    ui.add_space(8.0);
                    ui.weak("Noch keine Aktivitaet. Sprich, schreib oder teile ein Bild!");
                }
            });
    }

    /// Einstellungen als eigenes OS-Fenster (separater Viewport).
    fn ui_settings(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("settings_viewport"),
            egui::ViewportBuilder::default()
                .with_title("RustTalk – Einstellungen")
                .with_inner_size([460.0, 560.0])
                .with_min_inner_size([360.0, 320.0]),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    self.show_settings = false;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    self.settings_content(ui);
                });
            },
        );
    }

    /// Inhalt des Einstellungs-Fensters.
    fn settings_content(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new(format!("{}  Profil", icons::regular::USER)).strong());
        ui.horizontal(|ui| {
            ui.label("Benutzername (gilt beim naechsten Verbinden):");
            ui.text_edit_singleline(&mut self.config.username);
        });
        ui.separator();

        ui.label(egui::RichText::new(format!("{}  Spracheingabe", icons::regular::MICROPHONE)).strong());
        ui.horizontal(|ui| {
            ui.label("Push-to-Talk-Taste:");
            ui.text_edit_singleline(&mut self.config.ptt_key);
        });
        if ui
            .checkbox(&mut self.config.voice_activation, "Sprachaktivierung statt PTT")
            .changed()
        {
            self.control
                .voice_activation
                .store(self.config.voice_activation, Ordering::Relaxed);
        }
        if self.config.voice_activation {
            ui.horizontal(|ui| {
                ui.label("Empfindlichkeit:");
                if ui
                    .add(
                        egui::Slider::new(&mut self.config.vad_threshold, 0.001..=0.2)
                            .logarithmic(true),
                    )
                    .changed()
                {
                    self.control.set_vad_threshold(self.config.vad_threshold);
                }
            });
        }
        // Live-Pegel-Anzeige mit Schwellen-Marker (wie Discords Mikro-Test).
        ui.add_space(4.0);
        ui.label("Mikro-Pegel:");
        let level = if self.audio.is_some() {
            self.control.mic_level()
        } else {
            self.mic_monitor.as_ref().map(|m| m.level()).unwrap_or(0.0)
        };
        mic_meter(ui, level, self.config.vad_threshold);
        if self.audio.is_none() && self.mic_monitor.is_none() {
            ui.weak("Kein Mikrofonzugriff (Monitor konnte nicht gestartet werden).");
        }
        ui.weak("Gruen = oberhalb der Schwelle (wird gesendet).");
        if ui
            .checkbox(
                &mut self.config.noise_suppression,
                "Rauschunterdrueckung (KI, RNNoise)",
            )
            .changed()
        {
            self.control
                .noise_suppression
                .store(self.config.noise_suppression, Ordering::Relaxed);
        }
        ui.weak("Filtert Hintergrundgeraeusche vor dem Senden heraus.");
        if ui
            .checkbox(&mut self.config.auto_gain, "Automatische Lautstaerke")
            .changed()
        {
            self.control.auto_gain.store(self.config.auto_gain, Ordering::Relaxed);
        }
        ui.weak("Hebt leise Mikros an und daempft zu laute – andere hoeren dich gleichmaessig.");
        ui.separator();

        ui.label(egui::RichText::new(format!("{}  Geraete", icons::regular::SPEAKER_HIGH)).strong());
        egui::ComboBox::from_label("Eingabegeraet")
            .selected_text(self.config.input_device.clone().unwrap_or_else(|| "Standard".into()))
            .show_ui(ui, |ui| {
                if ui.selectable_label(self.config.input_device.is_none(), "Standard").clicked() {
                    self.config.input_device = None;
                }
                for name in &self.input_devices {
                    if ui
                        .selectable_label(self.config.input_device.as_deref() == Some(name), name)
                        .clicked()
                    {
                        self.config.input_device = Some(name.clone());
                    }
                }
            });
        egui::ComboBox::from_label("Ausgabegeraet")
            .selected_text(self.config.output_device.clone().unwrap_or_else(|| "Standard".into()))
            .show_ui(ui, |ui| {
                if ui.selectable_label(self.config.output_device.is_none(), "Standard").clicked() {
                    self.config.output_device = None;
                }
                for name in &self.output_devices {
                    if ui
                        .selectable_label(self.config.output_device.as_deref() == Some(name), name)
                        .clicked()
                    {
                        self.config.output_device = Some(name.clone());
                    }
                }
            });

        ui.separator();
        ui.weak("Geraetewechsel greifen beim naechsten Verbinden.");
        ui.separator();
        ui.label(egui::RichText::new(format!("{}  Video", icons::regular::VIDEO_CAMERA)).strong());
        ui.horizontal(|ui| {
            ui.label("Qualitaet beim Teilen:");
            egui::ComboBox::from_id_salt("video_quality")
                .selected_text(self.config.video_quality.label())
                .show_ui(ui, |ui| {
                    for q in [video::VideoQuality::High, video::VideoQuality::Balanced] {
                        ui.selectable_value(&mut self.config.video_quality, q, q.label());
                    }
                });
        });
        ui.weak("Gilt ab dem naechsten Start von Kamera/Bildschirm. \"Hoch\" fuer LAN, \"Ausgewogen\" fuer WLAN.");
        ui.separator();
        ui.label(egui::RichText::new(format!("{}  Integrationen", icons::regular::GIF)).strong());
        ui.horizontal(|ui| {
            ui.label("GIPHY API-Key:");
            let mut key = self.config.giphy_api_key.clone().unwrap_or_default();
            if ui.add(egui::TextEdit::singleline(&mut key).desired_width(220.0)).changed() {
                self.config.giphy_api_key =
                    if key.trim().is_empty() { None } else { Some(key.trim().to_string()) };
            }
        });
        ui.weak("Kostenlos unter developers.giphy.com (Beta-Key reicht).");
        if ui
            .button(egui::RichText::new(format!("{}  Speichern & schliessen", icons::regular::CHECK)))
            .clicked()
        {
            self.config.save();
            self.show_settings = false;
        }
    }

    /// GIPHY-Fenster: suchen, Vorschau-Grid, Klick = anhaengen.
    fn ui_giphy(&mut self, ctx: &egui::Context) {
        if !self.giphy.open {
            return;
        }
        // Hintergrund-Nachrichten einsammeln.
        while let Ok(msg) = self.giphy_rx.try_recv() {
            match msg {
                GiphyMsg::Results(Ok(results)) => {
                    self.giphy.loading = false;
                    self.giphy.error = None;
                    self.giphy.results = results;
                }
                GiphyMsg::Results(Err(e)) => {
                    self.giphy.loading = false;
                    self.giphy.error = Some(e);
                }
                GiphyMsg::Preview(url, bytes) => {
                    let _ = self.textures.texture_for(ctx, &bytes, &url);
                }
                GiphyMsg::Full(bytes) => {
                    self.pending_image = Some(("giphy.gif".to_string(), bytes));
                    self.giphy.open = false;
                }
            }
        }
        let mut open = self.giphy.open;
        egui::Window::new(format!("{}  GIF suchen (GIPHY)", icons::regular::GIF))
            .open(&mut open)
            .collapsible(false)
            .default_size([440.0, 400.0])
            .show(ctx, |ui| {
                let Some(key) = self.config.giphy_api_key.clone() else {
                    ui.weak(
                        "Kein GIPHY-API-Key hinterlegt.\nKostenlos unter developers.giphy.com erstellen und in den Einstellungen eintragen.",
                    );
                    return;
                };
                let search = ui
                    .horizontal(|ui| {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.giphy.query)
                                .hint_text("Suchbegriff...")
                                .desired_width(f32::INFINITY),
                        );
                        let enter =
                            resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if enter {
                            resp.request_focus();
                        }
                        enter
                            || ui
                                .button(format!("{}  Suchen", icons::regular::MAGNIFYING_GLASS))
                                .clicked()
                    })
                    .inner;
                if search && !self.giphy.query.trim().is_empty() {
                    self.giphy.loading = true;
                    self.giphy.results.clear();
                    self.giphy.requested.clear();
                    let q = self.giphy.query.trim().to_string();
                    let tx = self.giphy_tx.clone();
                    std::thread::spawn(move || {
                        let res = media::giphy_search(&key, &q).map_err(|e| format!("{e:#}"));
                        let _ = tx.send(GiphyMsg::Results(res));
                    });
                }
                if self.giphy.loading {
                    ui.weak("Suche laeuft...");
                }
                if let Some(e) = &self.giphy.error {
                    ui.label(egui::RichText::new(e).color(COLOR_MUTED));
                }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let results = self.giphy.results.clone();
                        for gif in results {
                            let tex =
                                self.textures.texture_for(ctx, gif.preview_url.as_bytes(), &gif.id);
                            match tex {
                                Some(tex) => {
                                    if ui
                                        .add(egui::ImageButton::new(&tex).frame(true))
                                        .on_hover_text("Anhaengen")
                                        .clicked()
                                    {
                                        let tx = self.giphy_tx.clone();
                                        let url = gif.full_url.clone();
                                        std::thread::spawn(move || {
                                            if let Ok(bytes) = media::download(&url) {
                                                let _ = tx.send(GiphyMsg::Full(bytes));
                                            }
                                        });
                                    }
                                }
                                None => {
                                    // Vorschau einmalig nachladen.
                                    if self.giphy.requested.insert(gif.preview_url.clone()) {
                                        let tx = self.giphy_tx.clone();
                                        let url = gif.preview_url.clone();
                                        std::thread::spawn(move || {
                                            if let Ok(bytes) = media::download(&url) {
                                                let _ = tx.send(GiphyMsg::Preview(url, bytes));
                                            }
                                        });
                                    }
                                    ui.add(
                                        egui::Button::new("...")
                                            .min_size(egui::vec2(90.0, 70.0)),
                                    );
                                }
                            }
                        }
                    });
                });
            });
        self.giphy.open = open && self.giphy.open;
    }

    /// Dialog: Channel umbenennen.
    fn ui_rename(&mut self, ctx: &egui::Context) {
        let Some((id, mut name)) = self.renaming_channel.take() else { return };
        let mut open = true;
        let mut done = false;
        egui::Window::new(format!("{}  Channel umbenennen", icons::regular::PENCIL_SIMPLE))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.text_edit_singleline(&mut name);
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() && !name.trim().is_empty() {
                        if let Some(tx) = &self.cmd_tx {
                            let _ = tx.send(Command::RenameChannel(id, name.trim().to_string()));
                        }
                        done = true;
                    }
                    if ui.button("Abbrechen").clicked() {
                        done = true;
                    }
                });
            });
        if open && !done {
            self.renaming_channel = Some((id, name));
        }
    }

    /// Dialog: neuen Channel erstellen (mit Art + Kategorie).
    fn ui_new_channel(&mut self, ctx: &egui::Context) {
        if !self.show_new_channel {
            return;
        }
        let mut open = true;
        egui::Window::new(format!("{}  Channel erstellen", icons::regular::PLUS))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Name:");
                let resp = ui.text_edit_singleline(&mut self.new_channel_name);
                if !resp.has_focus() {
                    resp.request_focus();
                }
                ui.add_space(4.0);

                ui.label("Art:");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.new_channel_kind, ChannelKind::Text,
                        format!("{} Text", icons::regular::HASH));
                    ui.selectable_value(&mut self.new_channel_kind, ChannelKind::Voice,
                        format!("{} Sprache", icons::regular::SPEAKER_HIGH));
                    ui.selectable_value(&mut self.new_channel_kind, ChannelKind::Category,
                        format!("{} Kategorie", icons::regular::FOLDER));
                });
                ui.add_space(4.0);

                // Kategorie-Auswahl (nicht fuer Kategorien selbst).
                if self.new_channel_kind != ChannelKind::Category {
                    let mut categories: Vec<(u64, String)> = self
                        .channels
                        .values()
                        .filter(|c| c.kind == ChannelKind::Category)
                        .map(|c| (c.id, c.name.clone()))
                        .collect();
                    categories.sort_by_key(|(id, _)| *id);
                    egui::ComboBox::from_label("Kategorie")
                        .selected_text(
                            self.new_channel_parent
                                .and_then(|id| self.channels.get(&id).map(|c| c.name.clone()))
                                .unwrap_or_else(|| "Keine".into()),
                        )
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(self.new_channel_parent.is_none(), "Keine").clicked() {
                                self.new_channel_parent = None;
                            }
                            for (id, name) in categories {
                                if ui
                                    .selectable_label(self.new_channel_parent == Some(id), &name)
                                    .clicked()
                                {
                                    self.new_channel_parent = Some(id);
                                }
                            }
                        });
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let create = ui.button("Erstellen").clicked()
                        || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                    if create && !self.new_channel_name.trim().is_empty() {
                        if let Some(tx) = &self.cmd_tx {
                            let parent = if self.new_channel_kind == ChannelKind::Category {
                                None
                            } else {
                                self.new_channel_parent
                            };
                            let _ = tx.send(Command::CreateChannel {
                                name: self.new_channel_name.trim().to_string(),
                                kind: self.new_channel_kind,
                                parent,
                            });
                        }
                        self.new_channel_name.clear();
                        self.show_new_channel = false;
                    }
                    if ui.button("Abbrechen").clicked() {
                        self.show_new_channel = false;
                    }
                });
            });
        self.show_new_channel = open && self.show_new_channel;
    }
}

// ---------------------------------------------------------------------------
// Anzeige-Helfer
// ---------------------------------------------------------------------------

/// Icon-Button fuer die Bottom-Bar (aktiv = rot hinterlegt).
/// Kamera/Bildschirm-Button: gruen hinterlegt, solange live geteilt wird.
fn share_button(ui: &mut egui::Ui, icon: &str, live: bool, tooltip: &str) -> egui::Response {
    let text = egui::RichText::new(icon).size(18.0).color(egui::Color32::WHITE);
    let btn = egui::Button::new(text)
        .fill(if live { COLOR_TALKING } else { egui::Color32::TRANSPARENT })
        .min_size(egui::vec2(34.0, 28.0));
    ui.add(btn).on_hover_text(tooltip)
}

/// Seitenverhaeltnis-treue Groesse von `img` innerhalb von `bounds`.
fn fit_size(img: egui::Vec2, bounds: egui::Vec2) -> egui::Vec2 {
    if img.x <= 0.0 || img.y <= 0.0 {
        return bounds;
    }
    let scale = (bounds.x / img.x).min(bounds.y / img.y);
    img * scale
}

/// Karte im Bildschirm-Auswahldialog: Vorschaubild + Titel (+ App-Name).
fn share_card(
    ui: &mut egui::Ui,
    entry: &ShareEntry,
    tex: Option<&egui::TextureHandle>,
    selected: bool,
    width: f32,
) -> egui::Response {
    let thumb_h = width * 9.0 / 16.0;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(width, thumb_h + 44.0), egui::Sense::click());
    let painter = ui.painter_at(rect);
    let bg = if selected || resp.hovered() { COLOR_HOVER } else { COLOR_DARK };
    painter.rect_filled(rect, 8.0, bg);
    let thumb_rect = egui::Rect::from_min_size(rect.min, egui::vec2(width, thumb_h)).shrink(6.0);
    painter.rect_filled(thumb_rect, 6.0, egui::Color32::BLACK);
    match tex {
        Some(tex) => {
            let fit = fit_size(tex.size_vec2(), thumb_rect.size());
            let r = egui::Rect::from_center_size(thumb_rect.center(), fit);
            painter.image(
                tex.id(),
                r,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            painter.text(
                thumb_rect.center(),
                egui::Align2::CENTER_CENTER,
                "Keine Vorschau",
                egui::FontId::proportional(13.0),
                COLOR_IDLE,
            );
        }
    }
    let text_y = thumb_rect.bottom() + 8.0;
    let max_chars = ((width - 16.0) / 7.5) as usize;
    let shorten = |t: &str| -> String {
        if t.chars().count() > max_chars {
            let mut s: String = t.chars().take(max_chars.saturating_sub(1)).collect();
            s.push('…');
            s
        } else {
            t.to_string()
        }
    };
    painter.text(
        egui::pos2(rect.left() + 8.0, text_y),
        egui::Align2::LEFT_TOP,
        shorten(&entry.title),
        egui::FontId::proportional(14.0),
        egui::Color32::WHITE,
    );
    if !entry.subtitle.is_empty() {
        painter.text(
            egui::pos2(rect.left() + 8.0, text_y + 17.0),
            egui::Align2::LEFT_TOP,
            shorten(&entry.subtitle),
            egui::FontId::proportional(12.0),
            COLOR_IDLE,
        );
    }
    if selected {
        painter.rect_stroke(
            rect.shrink(1.0),
            8.0,
            egui::Stroke::new(2.0_f32, COLOR_ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    resp.on_hover_text(&entry.title)
}

fn icon_button(ui: &mut egui::Ui, icon: &str, active: bool, tooltip: &str) -> egui::Response {
    let text = egui::RichText::new(icon).size(18.0).color(egui::Color32::WHITE);
    let btn = egui::Button::new(text)
        .fill(if active { COLOR_MUTED } else { egui::Color32::TRANSPARENT })
        .min_size(egui::vec2(34.0, 28.0));
    ui.add(btn).on_hover_text(tooltip)
}

/// Kreis-Avatar mit Initial; gruener Ring, wenn `talking`.
fn draw_avatar(ui: &mut egui::Ui, name: &str, size: f32, talking: bool) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let radius = size / 2.0;
    painter.circle_filled(center, radius, avatar_color(name));
    if talking {
        painter.circle_stroke(center, radius + 2.0, egui::Stroke::new(2.0, COLOR_TALKING));
    }
    let initial: String = name.chars().next().unwrap_or('?').to_uppercase().collect();
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(size * 0.52),
        egui::Color32::WHITE,
    );
    rect
}

fn kind_icon(kind: VideoKind) -> &'static str {
    match kind {
        VideoKind::Camera => icons::regular::VIDEO_CAMERA,
        VideoKind::Screen => icons::regular::MONITOR,
    }
}

fn kind_label(kind: VideoKind) -> &'static str {
    match kind {
        VideoKind::Camera => "Kamera",
        VideoKind::Screen => "Bildschirm",
    }
}

/// Gruener Punkt unten rechts am Avatar (spricht gerade).
fn draw_talk_dot(ui: &egui::Ui, avatar: egui::Rect) {
    let r = avatar.width() * 0.16;
    let c = avatar.right_bottom() - egui::vec2(r * 0.6, r * 0.6);
    ui.painter().circle_filled(c, r + 2.0, COLOR_DARK);
    ui.painter().circle_filled(c, r, COLOR_TALKING);
}

/// Deterministische Avatar-Farbe aus dem Namen.
fn avatar_color(name: &str) -> egui::Color32 {
    let hash = name
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    AVATAR_PALETTE[(hash as usize) % AVATAR_PALETTE.len()]
}

/// Eine Chat-Zeile (Nachricht mit optionalem Bild oder System-Meldung).
fn chat_line(ui: &mut egui::Ui, line: &ChatLine, textures: &mut TextureCache) {
    match &line.kind {
        ChatKind::System => {
            ui.horizontal(|ui| {
                ui.add_space(28.0);
                ui.weak(format_time(line.timestamp));
                ui.label(
                    egui::RichText::new(format!("{}  {}", icons::regular::CHAT_TEXT, line.text))
                        .weak()
                        .italics(),
                );
            });
        }
        ChatKind::Message { from, own } => {
            ui.horizontal(|ui| {
                draw_avatar(ui, from, 24.0, false);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let name_color = if *own { COLOR_ACCENT } else { COLOR_TEXT };
                        ui.label(egui::RichText::new(from).strong().color(name_color));
                        ui.weak(format_time(line.timestamp));
                    });
                    if !line.text.is_empty() {
                        ui.label(egui::RichText::new(&line.text).color(COLOR_TEXT));
                    }
                    if let Some(img) = &line.image {
                        match textures.texture_for(ui.ctx(), img, "chat-img") {
                            Some(tex) => {
                                ui.add(
                                    egui::Image::new(&tex)
                                        .max_width(320.0)
                                        .max_height(240.0)
                                        .rounding(6.0),
                                );
                            }
                            None => {
                                ui.weak("[Bild nicht dekodierbar]");
                            }
                        }
                    }
                });
            });
            ui.add_space(2.0);
        }
    }
}

/// Pegel-Balken mit Schwellen-Marker (weisser Strich) – wie Discords Mikro-Test.
/// Skala ist perceptual (sqrt), damit leise Signale sichtbar sind.
fn mic_meter(ui: &mut egui::Ui, level: f32, threshold: f32) {
    let scale = |v: f32| (v.sqrt() * 1.6).clamp(0.0, 1.0);
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 16.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, COLOR_DARK);
    let lw = rect.width() * scale(level);
    if lw > 0.5 {
        let fill = egui::Rect::from_min_size(rect.min, egui::vec2(lw, rect.height()));
        let color = if level >= threshold { COLOR_TALKING } else { COLOR_IDLE };
        p.rect_filled(fill, 4.0, color);
    }
    let tx = rect.left() + rect.width() * scale(threshold);
    p.line_segment(
        [egui::pos2(tx, rect.top()), egui::pos2(tx, rect.bottom())],
        egui::Stroke::new(2.0, egui::Color32::WHITE),
    );
}

/// Unix-Timestamp -> "HH:MM" (UTC; ausreichend fuer den MVP).
fn format_time(ts: u64) -> String {
    let secs = ts % 86_400;
    format!("{:02}:{:02}", secs / 3600, (secs / 60) % 60)
}

/// Mappt einen Konfigurations-String auf eine egui-Taste.
fn key_from_name(name: &str) -> Option<egui::Key> {
    let n = name.trim();
    if n.chars().count() == 1 {
        let c = n.chars().next().unwrap().to_ascii_uppercase();
        return Some(match c {
            'A' => egui::Key::A,
            'B' => egui::Key::B,
            'C' => egui::Key::C,
            'D' => egui::Key::D,
            'E' => egui::Key::E,
            'F' => egui::Key::F,
            'G' => egui::Key::G,
            'H' => egui::Key::H,
            'I' => egui::Key::I,
            'J' => egui::Key::J,
            'K' => egui::Key::K,
            'L' => egui::Key::L,
            'M' => egui::Key::M,
            'N' => egui::Key::N,
            'O' => egui::Key::O,
            'P' => egui::Key::P,
            'Q' => egui::Key::Q,
            'R' => egui::Key::R,
            'S' => egui::Key::S,
            'T' => egui::Key::T,
            'U' => egui::Key::U,
            'V' => egui::Key::V,
            'W' => egui::Key::W,
            'X' => egui::Key::X,
            'Y' => egui::Key::Y,
            'Z' => egui::Key::Z,
            '0' => egui::Key::Num0,
            '1' => egui::Key::Num1,
            '2' => egui::Key::Num2,
            '3' => egui::Key::Num3,
            '4' => egui::Key::Num4,
            '5' => egui::Key::Num5,
            '6' => egui::Key::Num6,
            '7' => egui::Key::Num7,
            '8' => egui::Key::Num8,
            '9' => egui::Key::Num9,
            _ => return None,
        });
    }
    match n.to_ascii_uppercase().as_str() {
        "SPACE" => Some(egui::Key::Space),
        "TAB" => Some(egui::Key::Tab),
        "ENTER" => Some(egui::Key::Enter),
        "ESCAPE" => Some(egui::Key::Escape),
        "F1" => Some(egui::Key::F1),
        "F2" => Some(egui::Key::F2),
        "F3" => Some(egui::Key::F3),
        "F4" => Some(egui::Key::F4),
        "F5" => Some(egui::Key::F5),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_mapping_letters() {
        assert_eq!(key_from_name("v"), Some(egui::Key::V));
        assert_eq!(key_from_name("A"), Some(egui::Key::A));
        assert_eq!(key_from_name("5"), Some(egui::Key::Num5));
        assert_eq!(key_from_name("SPACE"), Some(egui::Key::Space));
        assert_eq!(key_from_name("space"), Some(egui::Key::Space));
        assert_eq!(key_from_name("??"), None);
    }

    #[test]
    fn time_formatting() {
        assert_eq!(format_time(0), "00:00");
        assert_eq!(format_time(3_661), "01:01");
        assert_eq!(format_time(86_399), "23:59");
        assert_eq!(format_time(90_061), "01:01"); // naechster Tag (UTC)
    }

    #[test]
    fn avatar_color_is_deterministic() {
        assert_eq!(avatar_color("alice"), avatar_color("alice"));
        assert!(AVATAR_PALETTE.contains(&avatar_color("alice")));
        assert!(AVATAR_PALETTE.contains(&avatar_color("bob")));
    }
}
