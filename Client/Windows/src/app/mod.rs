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
//! - `connection_ui` voice connection popup (ping graph, packet loss)
//!
//! Channel kinds: Lobby (house, pinned, join log, no talking), Category
//! (collapsible group), Voice (speaker, click = join), Text (speech bubble).

mod audio_lifecycle;
mod audio_picker;
mod channel_settings;
mod connect;
mod connection_ui;
#[cfg(debug_assertions)]
mod dev_input;
mod emoji;
mod join_error;
mod lifecycle;
mod limits;
mod links;
mod media_ui;
mod mixer;
mod markdown;
mod mention;
mod moderation_ui;
mod poke;
mod server_settings;
mod server_switcher;
mod servers;
mod session;
mod settings;
mod sidebar;
mod stage;
mod theme;
mod update_ui;
mod views;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use rusttalk_shared::protocol::{
    GifInfo, ChannelInfo, ChannelKind, ServerMessage, ServerSettings, UserInfo, VideoKind, MAX_MEDIA_BYTES,
};

use crate::audio::sfx::{self, Sound};
use crate::audio::{capture, playback, AudioControl, AudioEngine, AudioSession, LocalMuteSet, StreamKind, TalkingMap, UserVolumeMap};
use crate::config::{ClientConfig, ServerBookmark};
use crate::hotkey;
use crate::logo;
use crate::media::{self, TextureCache};
use crate::net::{self, Command, NetEvent};
use crate::video::{self, CameraDevice, ShareEntry, ShareSource, VideoCapture};

use channel_settings::ChannelSettings;
use session::{
    reconnect_delay, retry_stagger, Reconnect, ServerSession, SessionId, BOUNCE_INTERVAL, EVENTS_PER_FRAME,
    MAX_ACTIVE_SESSIONS, MAX_AUTO_CONNECTING, MAX_LINES_PER_CHANNEL, MAX_RECONNECT_ATTEMPTS,
};
use settings::SettingsPage;
use mixer::volume_slider;

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

/// GIF picker popup (Discord style): trending on open, search while typing,
/// two-column grid, click = send. Searches go through the server.
#[derive(Default)]
struct GifPicker {
    /// Bumped whenever the picker is reset: previews downloaded for an
    /// older picker (another server's results) are dropped.
    epoch: u64,
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
    at: egui::Pos2,
    include_audio: bool,
    tab: ShareTab,
    entries: Vec<(ShareEntry, Option<egui::TextureHandle>)>,
    selected: Option<ShareSource>,
    loading: bool,
    rx: std::sync::mpsc::Receiver<Vec<ShareEntry>>,
}

impl ScreenPicker {
    fn open(at: egui::Pos2, include_audio: bool) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("share-sources".into())
            .spawn(move || {
                let _ = tx.send(video::list_share_sources());
            });
        Self { at, include_audio, tab: ShareTab::Apps, entries: Vec::new(), selected: None, loading: true, rx }
    }
}

/// Nachrichten aus Hintergrund-Threads (GIPHY-Suche/Downloads), tagged
/// with what they were started for: late ones never touch newer state.
enum GiphyMsg {
    Preview { epoch: u64, url: String, bytes: Vec<u8> },
    Full { request: u64, bytes: Vec<u8> },
    /// Download of the chosen GIF failed.
    FullFailed { request: u64, error: String },
}

/// Where the GIF that is downloading goes, bound to one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GifTarget {
    request: u64,
    session: SessionId,
    generation: u64,
    channel: u64,
    reply_to: Option<u64>,
}

pub struct RustTalkApp {
    config: ClientConfig,
    /// The viewed server (a placeholder before any was picked). UI code reads
    /// the server it shows from here.
    srv: ServerSession,
    /// All other servers of this run (connected, connecting, retrying or failed).
    others: Vec<ServerSession>,
    next_session_id: SessionId,
    /// The only server we may be in voice on. Camera, screen, audio sharing,
    /// video tiles, talking and per-user mixer state belong to it. Set only
    /// by our own voice join (see `servers::join_channel`), never by a server.
    voice_owner: Option<SessionId>,
    /// Connection (generation) of `voice_owner` the voice join was made on.
    voice_generation: u64,
    /// Voice channel we asked for and the server has not confirmed yet.
    voice_pending: Option<u64>,
    /// A matching explicit join was acknowledged on this connection.
    voice_confirmed: bool,
    /// Session whose UDP socket the running audio engine (`audio`) uses.
    audio_owner: Option<SessionId>,
    /// Connection (session, attempt) whose replay window is in `control.link`.
    link_conn: Option<(SessionId, u64)>,
    bandwidth: Option<crate::bandwidth::Call>,
    /// Reconnect enrolled servers on the first frame (never in tests).
    startup_pending: bool,

    // UI-State
    chat_input: String,
    markdown: markdown::Cache,
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
    /// @mention popup above the composer.
    mention: mention::MentionPopup,
    new_channel_name: String,
    new_channel_kind: ChannelKind,
    new_channel_parent: Option<u64>,
    show_new_channel: bool,
    /// Put the cursor in the name field (once, when the dialog opens).
    new_channel_focus: bool,
    show_settings: bool,
    settings_page: SettingsPage,
    settings_nav: settings::SettingsNav,
    /// Settings: waiting for a key press to bind push-to-talk.
    recording_key: Option<hotkey::Action>,
    keybind_error: Option<String>,
    /// Camera list for the settings (lazy, `None` = reload).
    cameras: Option<Vec<CameraDevice>>,
    channel_settings: Option<ChannelSettings>,
    /// Channel waiting for "really delete?" confirmation.
    confirm_delete: Option<u64>,
    /// Set from a menu; disconnects the viewed server at the start of the next frame.
    pending_disconnect: bool,
    /// Open "Server Settings" dialog (edit copy).
    server_settings_dialog: Option<server_settings::ServerSettingsDialog>,
    /// "Poke ..." dialog (target + message being written).
    poke_dialog: Option<poke::PokeDialog>,
    /// Received pokes not yet dismissed (popup, newest last).
    poke_inbox: Vec<poke::PokeNote>,
    /// Voice connection popup, anchored at this point (`None` = closed).
    connection_panel: Option<egui::Pos2>,
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
    /// Speaks names after the join / leave sound.
    announcer: crate::audio::speech::Announcer,
    /// The downloadable natural voices (state of the download).
    voice_pack: crate::audio::voice_pack::Pack,
    /// Binding for the global PTT poller (`None` = off / focused window only).
    global_ptt: hotkey::GlobalBinding,
    hotkey_events: std::sync::mpsc::Receiver<hotkey::Event>,
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
    video_frames: HashMap<(u64, VideoKind), media_ui::VideoView>,
    /// Streams of others we chose to watch (opt-in; the server only sends these).
    watching: HashSet<(u64, VideoKind)>,
    /// Who the Focus / Presenter layout shows (local only, per voice session).
    stage_pick: stage::StagePick,
    /// Camera the Presenter overlay cut out this frame (applied next frame).
    cutout_key: Option<(u64, VideoKind)>,
    /// Presenter camera while it is being dragged (offset from its corner).
    cam_drag: egui::Vec2,
    /// Layout to go back to when Presenter is switched off again.
    stage_flat: crate::config::StageMode,
    /// Layout in effect on the last stage frame.
    stage_now: crate::config::StageMode,
    /// Recently ended streams: late frames must not bring the tile back
    /// (otherwise the last picture stays frozen on screen).
    video_stopped: HashMap<(u64, VideoKind), Instant>,
    /// Per own source (camera, screen): bumped when it stops; frame sinks of
    /// an older generation drop their late frames.
    video_epoch: Arc<[std::sync::atomic::AtomicU64; 2]>,
    /// Decoded pictures / keyframe needs from the decoder threads.
    video_tx: std::sync::mpsc::Sender<media_ui::DecodeMsg>,
    video_rx: std::sync::mpsc::Receiver<media_ui::DecodeMsg>,
    /// Set by the screen thread when it ends with an error: the screen epoch
    /// it was started in (a newer share ignores it).
    screen_failed: Arc<Mutex<Option<u64>>>,
    /// Numbers the video tiles: decoder results name the tile they were made for.
    next_video_view: u64,
    /// Vollbild-Ansicht eines Streams (None = normale Ansicht).
    fullscreen: Option<(u64, VideoKind)>,
    /// egui-Kontext fuer Repaints aus Capture-/Netzwerk-Threads.
    ctx: egui::Context,
    /// Kamera (Vorschau oder live, siehe `VideoCapture::is_sending`).
    cam: Option<VideoCapture>,
    screen: Option<VideoCapture>,
    /// Computer audio of the screen share (with "Share audio").
    screen_audio: Option<crate::audio::screen_audio::ScreenAudio>,
    /// Starting it failed: don't retry until the share stops.
    screen_audio_failed: bool,
    /// Last audio-sharing status sent to the voice server (resent only when it changes).
    audio_flag_sent: bool,
    audio_flag_at: Option<Instant>,
    audio_only: bool,
    shared_audio_source: crate::audio::share_source::AudioShareSource,
    audio_picker: Option<audio_picker::AudioPicker>,
    /// Debug hook `share_self`: share this once we are in a voice channel.
    dev_pending_share: Option<ShareSource>,
    /// Debug builds: scripted input (`RUSTTALK_DEV_INPUT`).
    #[cfg(debug_assertions)]
    dev_input: dev_input::DevInput,
    cam_dialog: Option<CamDialog>,
    screen_picker: Option<ScreenPicker>,
    /// Letzter eigener Frame (auch im reinen Vorschau-Modus).
    local_preview: Arc<Mutex<Option<egui::ColorImage>>>,
    /// Textur der eigenen Vorschau (wird pro Frame ueberschrieben).
    local_preview_tex: Option<egui::TextureHandle>,
    /// Fehler aus dem Kamera-Thread fuer die Anzeige im Kamera-Dialog.
    cam_error: Arc<Mutex<Option<(u64, String)>>>,
    giphy: GifPicker,
    /// Kick / ban dialog.
    mod_dialog: Option<moderation_ui::ModDialog>,
    /// Started via an invite link: connect right away.
    connect_on_start: bool,
    /// "Reconnect on start?" question for a saved server not remembered yet.
    remember_prompt: Option<server_switcher::RememberPrompt>,
    /// Clicked chat link waiting for confirmation.
    link_prompt: Option<links::Prompt>,
    threat_feed: links::FeedState,
    /// Focus the password field on the next frame.
    focus_password: bool,
    /// Original address of the saved server being edited.
    editing_bookmark: Option<String>,
    server_nickname: String,
    show_connect_form: bool,
    /// Not installed yet: the whole window is the install screen.
    install_screen: Option<lifecycle::InstallScreen>,
    update: lifecycle::UpdateState,
    /// Clicked toasts report their server and channel here.
    toast_tx: std::sync::mpsc::Sender<(SessionId, u64)>,
    toast_rx: std::sync::mpsc::Receiver<(SessionId, u64)>,
    last_toast: Option<Instant>,
    /// Where the GIF that is downloading goes.
    gif_target: Option<GifTarget>,
    next_gif_request: u64,
    /// Screen rect of the GIF button (the picker opens above it).
    gif_button_rect: Option<egui::Rect>,
    emoji: emoji::EmojiPicker,
    emoji_button_rect: Option<egui::Rect>,
    giphy_tx: std::sync::mpsc::Sender<GiphyMsg>,
    giphy_rx: std::sync::mpsc::Receiver<GiphyMsg>,
}

impl RustTalkApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        emoji::add_font(&mut fonts);
        cc.egui_ctx.set_fonts(fonts);

        let config = ClientConfig::load();
        theme::set_appearance(&cc.egui_ctx, config.accent_color, config.compact);
        cc.egui_ctx.set_zoom_factor(config.ui_scale.clamp(0.75, 1.5));
        let mut config = config;
        let launch_link = std::env::args().skip(1).find_map(|a| moderation_ui::parse_invite_link(&a));
        let connect_on_start = launch_link.is_some();
        if let Some((addr, code)) = launch_link {
            config.server_address = addr;
            config.invite = code;
        }
        let mut app = Self::with_config(cc.egui_ctx.clone(), config, connect_on_start);
        app.startup_pending = true;
        app
    }

    fn with_config(ctx: egui::Context, mut config: ClientConfig, connect_on_start: bool) -> Self {
        links::prune(&mut config.trusted_links, links::now());
        // shortcut: a large feed (max 32 MiB) is parsed on the UI thread at start, move to a worker if that shows.
        let threat_feed = links::FeedState::load(config.threat_feed.as_deref());
        let control = Arc::new(AudioControl::new(config.voice_activation, config.vad_threshold));
        control.set_output_volume(config.output_volume);
        control.set_input_volume(config.input_volume);
        let (global_ptt, hotkey_events) = if cfg!(test) {
            (Default::default(), std::sync::mpsc::channel().1)
        } else {
            hotkey::spawn_global_ptt(control.clone())
        };
        let (toast_tx, toast_rx) = std::sync::mpsc::channel::<(SessionId, u64)>();
        let (video_tx, video_rx) = std::sync::mpsc::channel::<media_ui::DecodeMsg>();
        let (giphy_tx, giphy_rx) = std::sync::mpsc::channel::<GiphyMsg>();
        let server_nickname = config.bookmarks.iter()
            .find(|b| b.address == config.server_address)
            .and_then(|b| b.nickname()).unwrap_or_default().to_string();
        let sfx: sfx::SfxQueue = Default::default();
        Self {
            control,
            config,
            srv: ServerSession::new(0, ""),
            others: Vec::new(),
            next_session_id: 1,
            voice_owner: None,
            voice_generation: 0,
            voice_pending: None,
            voice_confirmed: false,
            audio_owner: None,
            link_conn: None,
            bandwidth: None,
            startup_pending: false,
            chat_input: String::new(),
            markdown: markdown::Cache::default(),
            reply_to: None,
            editing: None,
            edit_text: String::new(),
            focus_edit: false,
            confirm_delete_msg: None,
            chat_search: None,
            chat_action: None,
            focus_composer: false,
            mention: Default::default(),
            new_channel_name: String::new(),
            new_channel_kind: ChannelKind::Text,
            new_channel_parent: None,
            show_new_channel: false,
            new_channel_focus: false,
            show_settings: false,
            settings_page: SettingsPage::Voice,
            settings_nav: Default::default(),
            recording_key: None,
            keybind_error: None,
            cameras: None,
            channel_settings: None,
            confirm_delete: None,
            pending_disconnect: false,
            server_settings_dialog: None,
            poke_dialog: None,
            connection_panel: None,
            poke_inbox: Vec::new(),
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
            hotkey_events,
            announcer: crate::audio::speech::Announcer::new(sfx.clone()),
            voice_pack: crate::audio::voice_pack::Pack::new(),
            sfx,
            user_volumes: Arc::new(Mutex::new(HashMap::new())),
            mic_monitor: None,
            textures: TextureCache::default(),
            pending_image: None,
            video_frames: HashMap::new(),
            watching: HashSet::new(),
            stage_pick: Default::default(),
            cutout_key: None,
            cam_drag: egui::Vec2::ZERO,
            stage_flat: crate::config::StageMode::Grid,
            stage_now: crate::config::StageMode::Grid,
            video_stopped: HashMap::new(),
            video_epoch: Arc::new([std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0)]),
            video_tx,
            video_rx,
            screen_failed: Arc::new(Mutex::new(None)),
            next_video_view: 0,
            fullscreen: None,
            ctx,
            cam: None,
            screen: None,
            screen_audio: None,
            screen_audio_failed: false,
            audio_flag_sent: false,
            audio_flag_at: None,
            audio_only: false,
            shared_audio_source: Default::default(),
            audio_picker: None,
            dev_pending_share: None,
            #[cfg(debug_assertions)]
            dev_input: Default::default(),
            cam_dialog: None,
            screen_picker: None,
            local_preview: Arc::new(Mutex::new(None)),
            local_preview_tex: None,
            cam_error: Arc::new(Mutex::new(None)),
            giphy: GifPicker::default(),
            mod_dialog: None,
            connect_on_start,
            remember_prompt: None,
            link_prompt: None,
            threat_feed,
            focus_password: false,
            editing_bookmark: None,
            server_nickname,
            show_connect_form: false,
            install_screen: (crate::install::mode() == crate::install::Mode::NotInstalled).then(lifecycle::InstallScreen::new),
            update: lifecycle::UpdateState::new(),
            toast_tx,
            toast_rx,
            last_toast: None,
            gif_target: None,
            next_gif_request: 0,
            gif_button_rect: None,
            emoji: Default::default(),
            emoji_button_rect: None,
            giphy_tx: giphy_tx.clone(),
            giphy_rx,
        }
    }

    /// The viewed server is logged in (Welcome received).
    fn is_connected(&self) -> bool {
        self.srv.cmd_tx.is_some() && self.srv.connected
    }

    fn spawn_channel_id(&self) -> Option<u64> {
        self.srv.spawn_channel_id()
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

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn channel_name(&self, id: u64) -> String {
        self.srv.channel_name(id)
    }

    fn channel_kind(&self, id: u64) -> Option<ChannelKind> {
        self.srv.channel_kind(id)
    }

    /// Loads the saved volume for `name` (by username, local) into the mixer.
    /// Only for users of the voice server: the mixer is keyed by its client ids.
    fn apply_user_volume(&self, id: u64, name: &str) {
        let mut map = self.user_volumes.lock().unwrap();
        for kind in StreamKind::ALL {
            // A hand-edited `nan` in the config must never reach the mixer.
            match self.saved_volumes(kind).get(name).filter(|v| v.is_finite()) {
                Some(&v) => {
                    map.insert(kind.key(id), v.clamp(0.0, crate::audio::MAX_VOLUME));
                }
                None => {
                    map.remove(&kind.key(id));
                }
            }
        }
    }

    /// Saved volumes of one stream kind, by username.
    fn saved_volumes(&self, kind: StreamKind) -> &std::collections::BTreeMap<String, f32> {
        match kind {
            StreamKind::Voice => &self.config.user_volumes,
            StreamKind::Shared | StreamKind::App(_) => &self.config.share_volumes,
        }
    }

    /// Forgets the mixer state of a user who left voice.
    fn forget_user_audio(&self, id: u64) {
        self.user_volumes.lock().unwrap().retain(|k, _| crate::audio::mixer_speaker(*k) != id);
        self.control.forget_speaker(id);
    }

    /// Ends our camera (when it is live) and screen share; they belong to the voice server.
    fn stop_own_sources(&mut self) {
        self.stop_shared_audio();
        if self.cam.as_ref().is_some_and(|c| c.is_sending()) {
            self.cam = None;
            self.after_source_stopped(VideoKind::Camera);
        }
        if self.screen.is_some() {
            self.screen = None;
            self.after_source_stopped(VideoKind::Screen);
        }
    }

    fn window_focused(&self) -> bool {
        self.ctx.input(|i| i.viewport().focused).unwrap_or(true)
    }

    /// Plays a notification sound if it is enabled in the settings.
    pub(super) fn notify(&self, sound: Sound) {
        let on = match sound {
            Sound::Join => self.config.sound_join,
            Sound::Leave => self.config.sound_leave,
            Sound::Message | Sound::Mention => self.config.sound_message,
            Sound::Mute | Sound::Unmute => self.config.sound_mute,
            // A poke is meant to wake you up: always rings.
            Sound::Poke => true,
        };
        if on && self.audio.is_some() {
            sfx::play(&self.sfx, sound);
        }
    }

    /// Join / leave sound for someone in your voice channel, followed by the
    /// spoken name (if enabled).
    fn notify_presence(&self, name: Option<&str>, joined: bool) {
        self.notify(if joined { Sound::Join } else { Sound::Leave });
        let on = if joined { self.config.sound_join } else { self.config.sound_leave };
        if let (true, true, true, Some(name)) = (on, self.config.announce_names, self.audio.is_some(), name) {
            self.announcer.announce(name, joined, self.config.announce_voice.as_deref(), &self.config.announce_language);
        }
    }

    /// Lets a natural voice make the clips for this name ahead of time, so
    /// the name follows the chime without a pause when it is needed.
    fn prepare_name(&self, name: &str) {
        if self.config.announce_names && name != self.config.username {
            self.announcer.prepare(name, self.config.announce_voice.as_deref(), &self.config.announce_language);
        }
    }

    /// The same for everyone on the server (after connecting or when the
    /// voice was changed).
    pub(super) fn prepare_all_names(&self) {
        for u in self.srv.users.values() {
            self.prepare_name(&u.name);
        }
    }

    /// Natural voices for everyone without a click: the installed app downloads
    /// the pack once per start in the background (a broken download continues
    /// where it stopped) and, when it is there, switches ONCE from the Windows
    /// voice to a natural one in the Windows display language.
    fn auto_voices(&mut self) {
        use crate::audio::voice_pack::{self, State};
        static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let allowed = cfg!(windows) && (crate::install::mode() == crate::install::Mode::Installed || std::env::var_os("RUSTTALK_AUTO_VOICES").is_some());
        if allowed && self.config.auto_download_voices && !STARTED.swap(true, Ordering::Relaxed) {
            if matches!(self.voice_pack.state(), State::Missing | State::Failed(_)) {
                self.voice_pack.download();
            }
        }
        if !self.config.voices_switched && self.voice_pack.state() == State::Installed {
            self.config.voices_switched = true;
            if self.config.announce_voice.is_none() {
                self.config.announce_voice = Some(voice_pack::DEFAULT_VOICE.to_string());
                self.config.announce_language = voice_pack::system_language().to_string();
                self.prepare_all_names();
            }
            self.config.save();
        }
    }

    /// Playback volume of one user's stream (1.0 = 100 %), saved by username.
    /// The config is the only source of truth; mixer panel and user menu both read it.
    fn user_volume(&self, name: &str, kind: StreamKind) -> f32 {
        self.saved_volumes(kind).get(name).copied().filter(|v| v.is_finite()).unwrap_or(1.0)
    }

    /// Sets the playback volume of one user's stream of the viewed server and
    /// remembers it by username. The mixer only changes while this server has
    /// voice (its client ids would name other people elsewhere).
    fn set_user_volume(&mut self, id: u64, name: &str, kind: StreamKind, volume: f32) {
        let v = volume.clamp(0.0, crate::audio::MAX_VOLUME);
        let saved = match kind {
            StreamKind::Voice => &mut self.config.user_volumes,
            StreamKind::Shared | StreamKind::App(_) => &mut self.config.share_volumes,
        };
        if (v - 1.0).abs() < 0.005 {
            saved.remove(name);
        } else {
            saved.insert(name.to_string(), v);
        }
        if self.viewing_voice() {
            self.apply_user_volume(id, name);
        }
    }

    /// Volume slider for one user's stream (user menu and mixer); saves when released.
    fn user_volume_slider(&mut self, ui: &mut egui::Ui, user: &UserInfo, kind: StreamKind) -> egui::Response {
        let mut pct = (self.user_volume(&user.name, kind) * 100.0).round();
        let r = ui.add(volume_slider(&mut pct));
        if r.changed() {
            self.set_user_volume(user.id, &user.name, kind, pct / 100.0);
        }
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            self.config.save();
        }
        r
    }

    /// Applies a picked message action (reply / edit / delete).
    fn apply_chat_action(&mut self, cmd_tx: &net::CommandSender) {
        let Some(action) = self.chat_action.take() else { return };
        match action {
            ChatAction::Reply(id) => {
                self.editing = None;
                self.reply_to = Some(id);
                self.focus_composer = true;
            }
            ChatAction::Edit(id) => {
                let text = self.srv.chat.values().flatten().find(|l| l.id == id).map(|l| l.text.clone());
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
                let text = emoji::replace_shortcodes(std::mem::take(&mut self.edit_text).trim());
                let original = self.srv.chat.values().flatten().find(|l| l.id == id).map(|l| l.text.clone());
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
        if self.srv.selected_channel != Some(id) {
            self.reply_to = None;
            self.editing = None;
            self.chat_search = None;
            self.mention = Default::default();
        }
        self.srv.selected_channel = Some(id);
        self.srv.unread.remove(&id);
    }

    /// One network event of server `id`. Only that session changes; voice and
    /// media react only to the voice server, dialogs only to the viewed one.
    fn handle_event(&mut self, id: SessionId, ev: NetEvent) {
        match ev {
            NetEvent::Connected {
                my_id,
                settings,
                users,
                channels,
                udp_socket,
                udp_cipher,
                fingerprint,
                udp_token,
                spawn_log,
                chat_history,
                gif_search,
                open_permissions,
            } => {
                let viewed = self.srv.id == id;
                let username = self.config.username.clone();
                if self.session(id).is_none_or(|s| s.cmd_tx.is_none()) {
                    return; // attempt was cancelled meanwhile
                }
                if let Err(e) = limits::welcome(&settings, &users, &channels, &spawn_log, &chat_history) {
                    return self.protocol_violation(id, e);
                }
                let Some(s) = self.session_mut(id) else { return };
                s.connected = true;
                s.gif_available = gif_search;
                // Pinned before Hello (the user trusted it), nothing to learn here.
                s.server_fingerprint = fingerprint;
                s.cert_check = None;
                s.join_error = None;
                s.last_error = None;
                s.overflow = false;
                s.open_permissions = open_permissions;
                s.invite = None; // used
                s.server_settings = settings;
                s.my_id = my_id;
                s.channels = channels.into_iter().map(|c| (c.id, c)).collect();
                s.users = users.into_iter().map(|u| (u.id, u)).collect();
                s.clear_chat();
                // Spawn-Log in die Spawn-Ansicht laden.
                if let Some(spawn_id) = s.spawn_channel_id() {
                    for e in spawn_log {
                        s.push_line(spawn_id, ChatLine::system(e.text, e.timestamp));
                    }
                    s.selected_channel = Some(spawn_id);
                }
                // Persistierten Chat-Verlauf in die Channels laden
                // ("eigene" Nachrichten per Name erkannt – kein Account-System).
                // Only the newest lines a channel keeps, so loading stays linear.
                let mut left: HashMap<u64, usize> = HashMap::new();
                for rec in &chat_history {
                    *left.entry(rec.channel_id).or_default() += 1;
                }
                for rec in chat_history {
                    let Some(n) = left.get_mut(&rec.channel_id) else { continue };
                    *n -= 1;
                    if *n >= MAX_LINES_PER_CHANNEL {
                        continue;
                    }
                    let own = rec.from_name == username;
                    s.push_line(rec.channel_id, ChatLine {
                        id: rec.id,
                        kind: ChatKind::Message { from: rec.from_name, own },
                        text: rec.text,
                        timestamp: rec.timestamp,
                        image: rec.image,
                        reply_to: rec.reply_to,
                        edited: rec.edited,
                    });
                }
                s.audio_session = match (udp_socket, udp_cipher) {
                    (Some(udp), Some(cipher)) => Some(AudioSession { udp, token: udp_token, cipher }),
                    _ => None,
                };
                s.replay = None; // new key: its receive counters start fresh
                let rc = s.reconnect.take();
                let remember = std::mem::take(&mut s.remember);
                let (key, name) = (s.key.clone(), s.server_settings.name.clone());
                if viewed {
                    self.config.invite = None;
                    self.prepare_all_names();
                    self.set_status(format!("Connected to {name}"));
                }
                // A login is chat only; mute metadata goes to the voice owner.
                if remember {
                    self.enroll(&key, &name);
                }
                if let Some(rc) = rc {
                    self.restore_after_reconnect(id, rc);
                }
                self.sync_voice_audio();
            }
            NetEvent::Message(msg) => self.handle_server_message(id, msg),
            // Errors of local media threads (camera, screen) of the voice server.
            NetEvent::Status(text) => self.set_error(text),
            NetEvent::Disconnected { reason } => self.on_connection_lost(id, reason),
            NetEvent::CertCheck { addr, fingerprint, changed } => {
                self.on_cert_check(id, join_error::CertCheck { addr, fingerprint, changed })
            }
        }
    }

    fn handle_server_message(&mut self, id: SessionId, msg: ServerMessage) {
        let viewed = self.srv.id == id;
        let voice = self.voice_authorized(id);
        let voice_requested = self.voice_connection_matches(id);
        let pending = self.voice_pending;
        let Some(s) = self.session(id) else { return };
        if let Err(e) = limits::message(s, &msg) {
            return self.protocol_violation(id, e);
        }
        let Some(s) = self.session_mut(id) else { return };
        match msg {
            ServerMessage::Welcome { .. } => {} // kommt nur via NetEvent::Connected
            ServerMessage::UserJoined { user } => {
                // Listed in one channel at most: member lists stay bounded by the users.
                for ch in s.channels.values_mut() {
                    ch.user_ids.retain(|&uid| uid != user.id);
                }
                if let Some(ch) = s.channels.get_mut(&user.channel_id) {
                    ch.user_ids.push(user.id);
                }
                let (uid, name) = (user.id, user.name.clone());
                s.users.insert(user.id, user);
                // Join-Zeile kommt serverseitig via SpawnLogEntry.
                if voice {
                    self.apply_user_volume(uid, &name);
                }
                if voice || viewed {
                    self.prepare_name(&name);
                }
            }
            ServerMessage::UserLeft { client_id } => {
                let mine = s.my_voice_channel();
                let leaving = s.users.get(&client_id).filter(|u| mine.is_some() && Some(u.channel_id) == mine).map(|u| u.name.clone());
                let me = client_id == s.my_id;
                s.users.remove(&client_id);
                for ch in s.channels.values_mut() {
                    ch.user_ids.retain(|&uid| uid != client_id);
                }
                if voice {
                    if let Some(name) = leaving {
                        self.notify_presence(Some(&name), false);
                    }
                    self.forget_user_audio(client_id);
                    self.clear_video_of(client_id, None);
                }
                if me {
                    self.revoke_voice(id);
                    self.sync_voice_audio();
                }
            }
            ServerMessage::UserMoved { client_id, channel_id } => self.on_user_moved(id, client_id, channel_id),
            ServerMessage::ChannelCreated { channel } => {
                s.channels.insert(channel.id, channel);
            }
            ServerMessage::ChannelDeleted { channel_id } => {
                // Our voice channel (or the one we asked for) is gone: so is the
                // voice join. A later move into another channel needs a new one.
                let ends_voice = voice_requested && (s.my_channel() == Some(channel_id) || pending == Some(channel_id));
                let was_category = s.channels.get(&channel_id).is_some_and(|c| c.kind.is_group());
                s.channels.remove(&channel_id);
                if was_category {
                    // Kinder der geloeschten Kategorie werden Top-Level.
                    for ch in s.channels.values_mut() {
                        if ch.parent == Some(channel_id) {
                            ch.parent = None;
                        }
                    }
                }
                s.remove_channel_chat(channel_id);
                s.unread.remove(&channel_id);
                s.collapsed.remove(&channel_id);
                if s.selected_channel == Some(channel_id) {
                    s.selected_channel = s.spawn_channel_id();
                }
                if ends_voice {
                    self.revoke_voice(id);
                } else if voice {
                    self.sync_voice_audio();
                }
            }
            ServerMessage::ChannelRenamed { channel_id, name } => {
                if let Some(ch) = s.channels.get_mut(&channel_id) {
                    ch.name = name;
                }
            }
            ServerMessage::ChannelsReordered { placements } => {
                for p in placements {
                    if let Some(ch) = s.channels.get_mut(&p.id) {
                        ch.parent = p.parent;
                        ch.position = p.position;
                    }
                }
            }
            ServerMessage::SpawnLogEntry { entry } => {
                if let Some(spawn_id) = s.spawn_channel_id() {
                    let open = viewed && s.selected_channel == Some(spawn_id);
                    s.system_line(spawn_id, entry.text, entry.timestamp, open);
                }
            }
            ServerMessage::Chat { id: message_id, channel_id, from_id, from_name, text, timestamp, image, reply_to } => {
                let own = from_id == s.my_id;
                let open = viewed && s.selected_channel == Some(channel_id);
                let has_image = image.is_some();
                let line = ChatLine {
                    id: message_id,
                    kind: ChatKind::Message { from: from_name.clone(), own },
                    text: text.clone(),
                    timestamp,
                    image,
                    reply_to,
                    edited: false,
                };
                if !s.push_line(channel_id, line) {
                    return; // unknown channel
                }
                if !open {
                    s.unread.insert(channel_id);
                }
                // from_id 0 = server announcement (no sound).
                if !own && from_id != 0 {
                    let focused = self.window_focused();
                    let mentioned = mentions(&text, &self.config.username);
                    if mentioned || !focused || !open {
                        self.notify(if mentioned { Sound::Mention } else { Sound::Message });
                    }
                    if !focused {
                        self.message_toast(id, channel_id, &from_name, &text, has_image, mentioned);
                    }
                    if !focused && self.config.flash_taskbar {
                        crate::notify::request_attention(&self.ctx, egui::UserAttentionType::Informational);
                    }
                }
            }
            ServerMessage::GifResults { query, offset, results, error } => {
                if viewed {
                    self.on_gif_results(query, offset, results, error);
                }
            }
            ServerMessage::UserRoleChanged { client_id, role } => {
                if let Some(u) = s.users.get_mut(&client_id) {
                    u.role = role;
                }
                if client_id == s.my_id {
                    if viewed {
                        self.set_status(format!("You are now {}", role.label()));
                    }
                    self.request_admin_data_for(id);
                }
            }
            ServerMessage::PermissionsChanged { open } => {
                s.open_permissions = open;
                self.request_admin_data_for(id);
            }
            ServerMessage::Removed { reason, banned: _ } => {
                s.removed = Some(reason);
            }
            ServerMessage::AdminData { roles, bans, invites, audit } => {
                s.admin = moderation_ui::AdminData { roles, bans, invites, audit, loaded: true };
            }
            ServerMessage::InviteCreated { invite } => {
                if viewed {
                    self.set_status(format!("Invite {} created - copy it under Server Settings > Invites", invite.code));
                }
                self.request_admin_data_for(id);
            }
            ServerMessage::GifSearchChanged { available } => {
                s.gif_available = available;
                if viewed {
                    self.giphy.shown_query = None; // reload the picker
                    self.giphy.results.clear();
                }
            }
            ServerMessage::ChatEdited { channel_id, message_id, text } => s.edit_line(channel_id, message_id, text),
            ServerMessage::ChatDeleted { channel_id, message_id } => {
                s.remove_line(channel_id, message_id);
                if viewed {
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
            }
            ServerMessage::MuteStateChanged { client_id, muted, deafened } => {
                if let Some(u) = s.users.get_mut(&client_id) {
                    u.muted = muted;
                    u.deafened = deafened;
                }
            }
            ServerMessage::StreamsChanged { client_id, streams } => {
                let mine = client_id == s.my_id;
                if let Some(u) = s.users.get_mut(&client_id) {
                    u.streams = streams.clone();
                }
                // Ended streams: their tile and our watching go away.
                if voice && !mine {
                    for kind in [VideoKind::Camera, VideoKind::Screen] {
                        if !streams.contains(&kind) {
                            self.watching.remove(&(client_id, kind));
                            if self.video_frames.contains_key(&(client_id, kind)) {
                                self.clear_video_of(client_id, Some(kind));
                            }
                        }
                    }
                }
            }
            ServerMessage::SharingChanged { client_id, sharing } => {
                if let Some(u) = s.users.get_mut(&client_id) {
                    u.sharing = sharing;
                }
                if voice && !sharing {
                    self.clear_video_of(client_id, None);
                }
            }
            ServerMessage::AudioStreamsRejected { stream_id } => {
                if voice && matches!(&self.shared_audio_source, crate::audio::share_source::AudioShareSource::Separate(apps) if apps.iter().any(|(_, s)| s.id == stream_id)) {
                    self.stop_shared_audio();
                }
            }
            ServerMessage::AudioStreamsChanged { client_id, streams } => {
                if voice && rusttalk_shared::protocol::valid_audio_streams(&streams) && s.users.get(&client_id).zip(s.users.get(&s.my_id)).is_some_and(|(u, me)| u.channel_id == me.channel_id) {
                    if client_id == s.my_id {
                        if let Some(audio) = &self.screen_audio { audio.register(streams.iter().map(|s| s.id).collect()); }
                    }
                    let keep = |key: &u128| crate::audio::mixer_speaker(*key) != client_id || (*key >> 1) as u32 == 0 || streams.iter().any(|s| s.id == (*key >> 1) as u32);
                    self.user_volumes.lock().unwrap().retain(|k, _| keep(k));
                    self.control.stream_meters.lock().unwrap().retain(|k, _| keep(k));
                    self.control.muted_streams.lock().unwrap().retain(keep);
                    let mut all = self.control.shared_streams.lock().unwrap();
                    if all.len() < 256 || all.contains_key(&client_id) { all.insert(client_id, streams); }
                }
            }
            ServerMessage::AudioSharingChanged { client_id, sharing } => {
                if let Some(u) = s.users.get_mut(&client_id) {
                    u.audio_sharing = sharing;
                    if voice && !sharing {
                        self.control.shared_streams.lock().unwrap().remove(&client_id);
                        self.user_volumes.lock().unwrap().retain(|k, _| crate::audio::mixer_speaker(*k) != client_id || (*k >> 1) as u32 == 0);
                        self.control.muted_streams.lock().unwrap().retain(|k| crate::audio::mixer_speaker(*k) != client_id || (*k >> 1) as u32 == 0);
                    }
                }
            }
            ServerMessage::VideoFrame { from_id, kind, data } => {
                // Video tiles belong to the voice server only.
                if !voice {
                    return;
                }
                // Frames still queued after a stream ended must not bring its
                // tile back ("Connecting..." forever).
                let live = if from_id == s.my_id {
                    match kind {
                        VideoKind::Camera => self.cam.as_ref().is_some_and(|c| c.is_sending()),
                        VideoKind::Screen => self.screen.is_some(),
                    }
                } else {
                    // Only streams we chose to watch.
                    self.watching.contains(&(from_id, kind))
                };
                if live {
                    self.feed_video((from_id, kind), data);
                }
            }
            ServerMessage::KeyframeRequest { kind } => {
                let src = match kind {
                    VideoKind::Camera => self.cam.as_ref(),
                    VideoKind::Screen => self.screen.as_ref(),
                };
                if let (true, Some(c)) = (voice, src) {
                    c.request_keyframe();
                }
            }
            ServerMessage::VideoStopped { from_id, kind } => {
                if voice {
                    self.clear_video_of(from_id, Some(kind));
                }
            }
            ServerMessage::ServerSettingsChanged { settings } => {
                s.server_settings = settings;
            }
            ServerMessage::Poked { from_id, from_name, message } => self.receive_poke(id, from_id, from_name, message),
            ServerMessage::Notice { message } => {
                let message = self.server_text(id, message);
                self.set_status(message);
            }
            ServerMessage::Error { message } => {
                let message = self.server_text(id, message);
                self.set_error(message);
            }
        }
    }

    /// `client_id` moved to `channel_id` on server `id`.
    fn on_user_moved(&mut self, id: SessionId, client_id: u64, channel_id: u64) {
        let viewed = self.srv.id == id;
        let mut voice = self.voice_authorized(id);
        let Some(s) = self.session(id) else { return };
        if !s.users.contains_key(&client_id) {
            return; // unknown users would grow the member lists without bound
        }
        let me = client_id == s.my_id;
        let name = s.users.get(&client_id).map(|u| u.name.clone());
        let old_channel = s.users.get(&client_id).map(|u| u.channel_id);
        let previous_voice = s.my_voice_channel();
        let into_voice = s.channel_kind(channel_id) == Some(ChannelKind::Voice);
        if me && into_voice {
            // Only our own voice join lets a server put us into voice; once
            // there, its moves between voice channels keep it (moderators, AFK).
            let requested = self.voice_pending == Some(channel_id)
                || (voice && previous_voice.is_some());
            if self.voice_connection_matches(id) && requested {
                self.voice_pending = None;
                self.voice_confirmed = true;
                voice = true;
            } else {
                // Never opens the microphone, also not on the viewed server.
                self.revoke_voice(id);
                voice = false;
                self.bounce_to_lobby(id);
            }
        }
        if voice {
            // Wer den Channel wechselt, schickt uns keine Frames mehr (bzw.
            // wir bekommen keine mehr, wenn wir selbst wechseln).
            if me {
                let others: Vec<u64> = self.video_frames.keys().map(|(uid, _)| *uid).filter(|uid| *uid != client_id).collect();
                for uid in others {
                    self.clear_video_of(uid, None);
                }
                // Leaving a channel ends our camera and screen share: they
                // don't follow us into the next channel (start them again there).
                // What we watched there ends too (the server forgets it as well).
                if old_channel.is_some_and(|old| old != channel_id) {
                    self.stop_own_sources();
                    self.watching.clear();
                }
            } else {
                self.clear_video_of(client_id, None);
                if previous_voice.is_some() && Some(channel_id) == previous_voice {
                    self.notify_presence(name.as_deref(), true);
                } else if previous_voice.is_some() && old_channel == previous_voice {
                    self.notify_presence(name.as_deref(), false);
                }
            }
        }
        if voice {
            if me { self.control.shared_streams.lock().unwrap().clear(); }
            else { self.control.shared_streams.lock().unwrap().remove(&client_id); }
        }
        let Some(s) = self.session_mut(id) else { return };
        if let Some(u) = s.users.get_mut(&client_id) {
            u.channel_id = channel_id;
        }
        for ch in s.channels.values_mut() {
            if ch.id == channel_id {
                if !ch.user_ids.contains(&client_id) {
                    ch.user_ids.push(client_id);
                }
            } else {
                ch.user_ids.retain(|&uid| uid != client_id);
            }
        }
        let now_voice = s.my_voice_channel();
        if let Some(name) = name {
            let line = if me {
                format!("You joined {}", s.channel_name(channel_id))
            } else {
                format!("{name} joined {}", s.channel_name(channel_id))
            };
            let open = viewed && s.selected_channel == Some(channel_id);
            s.system_line(channel_id, line, Self::now(), open);
        }
        if me && voice {
            if now_voice.is_none() && previous_voice.is_some() {
                self.revoke_voice(id); // left voice: the next voice join decides again
            }
            self.sync_voice_audio();
            if now_voice.is_some() && previous_voice != now_voice {
                self.notify(Sound::Join);
            } else if previous_voice.is_some() && now_voice.is_none() {
                self.notify(Sound::Leave);
            }
        }
    }
}

impl eframe::App for RustTalkApp {
    #[cfg(debug_assertions)]
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.dev_input.feed(ctx, raw_input);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.install_screen.is_some() {
            self.ui_install(ctx);
            return;
        }
        self.poll_background(ctx);
        self.poll_video(ctx);
        if std::mem::take(&mut self.connect_on_start) && self.srv.cmd_tx.is_none() {
            self.start_connect(false);
        }
        if std::mem::take(&mut self.startup_pending) {
            self.connect_enrolled_servers();
        }
        // Installed app: look for an update once per start.
        static CHECKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if self.config.check_updates
            && crate::install::mode() == crate::install::Mode::Installed
            && !CHECKED.swap(true, Ordering::Relaxed)
        {
            self.check_for_updates();
        }
        self.auto_voices();
        self.poll_sessions();
        #[cfg(debug_assertions)]
        self.dev_hooks(ctx);
        if std::mem::take(&mut self.pending_disconnect) {
            let key = self.srv.key.clone();
            self.disconnect_server(&key);
        }
        self.poll_reconnect();
        self.prune_sessions();
        self.media_upkeep();
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

        if self.any_connected() {
            // Push-to-talk: either the global poller (also in the background)
            // or the focused window. Never while a new key is being recorded.
            let binding = if self.recording_key.is_some() { None } else { hotkey::parse_binding(&self.config.ptt_key) };
            let global = self.config.global_ptt && binding.is_some();
            let keys = if self.recording_key.is_some() { [None; 3] } else {
                [if global { binding } else { None },
                 self.config.mute_key.as_deref().and_then(hotkey::parse_binding),
                 self.config.deafen_key.as_deref().and_then(hotkey::parse_binding)]
            };
            let generation = {
                let mut active = self.global_ptt.lock().unwrap();
                if active.set(keys) { self.control.ptt_pressed.store(false, Ordering::Relaxed); }
                active.generation
            };
            while let Ok(event) = self.hotkey_events.try_recv() {
                if event.generation != generation || self.recording_key.is_some() { continue; }
                match event.action {
                    hotkey::Action::Mute => self.toggle_mute(),
                    hotkey::Action::Deafen => self.toggle_deafen(),
                    hotkey::Action::PushToTalk => {},
                }
            }
            if !global {
                let pressed = binding.is_some_and(|b| ctx.input(|i| b.is_down_in(i)));
                self.control.ptt_pressed.store(pressed, Ordering::Relaxed);
            }
        } else {
            self.global_ptt.lock().unwrap().set([None; 3]);
            self.control.ptt_pressed.store(false, Ordering::Relaxed);
            while self.hotkey_events.try_recv().is_ok() {}
        }
        if self.is_connected() {
            self.ui_connected(ctx);
        } else {
            self.ui_connect(ctx);
        }
        if self.show_settings {
            self.ui_settings(ctx);
        }
        self.ui_new_channel(ctx);
        self.ui_channel_settings(ctx);
        self.ui_server_settings(ctx);
        self.ui_poke_dialog(ctx);
        self.ui_poke_popup(ctx);
        self.ui_connection_panel(ctx);
        self.ui_confirm_delete(ctx);
        self.ui_confirm_delete_message(ctx);
        self.ui_mod_dialog(ctx);
        self.ui_gif_picker(ctx);
        self.ui_emoji_picker(ctx);
        self.ui_camera_dialog(ctx);
        self.ui_audio_picker(ctx);
        self.ui_screen_picker(ctx);
        self.ui_update_dialog(ctx, false);
        self.ui_remember_prompt(ctx);
        self.sample_bandwidth(Instant::now());
        self.ui_links(ctx);

        self.sync_mic_preview();
        if self.audio.is_none() {
            self.sfx.lock().unwrap().clear();
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        #[cfg(target_os = "linux")]
        hotkey::shutdown_global_ptt(&self.global_ptt);
        self.config.save();
    }
}

impl RustTalkApp {
    /// Main view while connected: sidebar + content (or fullscreen stream).
    /// Every frame, whichever server is viewed: media follows the voice server.
    fn media_upkeep(&mut self) {
        // Background removal only for the camera the overlay drew last frame.
        let cutout = self.cutout_key.take();
        for (key, view) in &self.video_frames {
            view.cutout.store(Some(*key) == cutout, Ordering::Relaxed);
        }
        // Screen thread ended with an error (e.g. window closed).
        if self.take_screen_failure() && self.screen.is_some() {
            self.screen = None;
            self.after_source_stopped(VideoKind::Screen);
        }
        // No talking/sharing outside voice (e.g. after a reconnect landed us in the lobby).
        if !self.in_voice() {
            self.stop_own_sources();
        } else if let Some(src) = self.dev_pending_share.take() {
            self.start_screen_share(src);
        }
        self.sync_shared_audio();
        self.sync_audio_flag();
    }

    fn ui_connected(&mut self, ctx: &egui::Context) {
        let cmd_tx = self.srv.tx().expect("connected");
        self.apply_chat_action(&cmd_tx);

        // Fullscreen: only the stream + member bar. Ends when the stream is gone
        // (a stream just picked to watch counts while its first frame is on the way).
        // Streams belong to the voice server, so only while it is viewed.
        if let Some(key) = self.fullscreen {
            let live = self.viewing_voice()
                && (self.video_frames.contains_key(&key)
                    || (self.watching.contains(&key) && self.srv.users.get(&key.0).is_some_and(|u| u.streams.contains(&key.1))));
            if live {
                self.ui_fullscreen(ctx, key);
                return;
            }
            self.leave_fullscreen();
        }

        self.ui_sidebar(ctx, Some(&cmd_tx));
        self.ui_main(ctx, &cmd_tx);
        // Toast only without an open dialog (it would cover dialog buttons).
        let dialog_open = self.channel_settings.is_some()
            || self.server_settings_dialog.is_some()
            || self.poke_dialog.is_some()
            || !self.poke_inbox.is_empty()
            || self.show_new_channel
            || self.confirm_delete.is_some()
            || self.confirm_delete_msg.is_some()
            || self.mod_dialog.is_some()
            || self.audio_picker.is_some()
            || self.cam_dialog.is_some()
            || self.screen_picker.is_some();
        if !dialog_open {
            theme::toast(ctx, &self.status, self.status_at.elapsed().as_secs_f32(), self.status_is_error);
        }
    }

    /// Debug builds only: env-controlled shortcuts for UI screenshots/tests.
    /// `RUSTTALK_DEV_AUTOCONNECT=1` connects at start,
    /// `RUSTTALK_DEV_OPEN=settings:<page>|channel:<name>|new_channel|select:<name>|join:<name>|input:#<n>`.
    /// `RUSTTALK_DEV_SHARE=<secs>` starts sharing the first screen once we are
    /// in a voice channel and stops it again after `<secs>` (0 = keep).
    #[cfg(debug_assertions)]
    fn dev_hooks(&mut self, _ctx: &egui::Context) {
        use std::sync::atomic::AtomicU8;
        static STAGE: AtomicU8 = AtomicU8::new(0);
        static SHARE: Mutex<(u8, Option<Instant>)> = Mutex::new((0, None));
        if let Ok(secs) = std::env::var("RUSTTALK_DEV_SHARE") {
            let mut share = SHARE.lock().unwrap();
            if share.0 == 0 && self.in_voice() {
                if let Some(src) = video::list_share_sources().into_iter().find(|e| matches!(e.source, ShareSource::Monitor(_))) {
                    self.start_screen_share(src.source);
                    *share = (1, Some(Instant::now()));
                }
            } else if share.0 == 1 {
                let secs: u64 = secs.parse().unwrap_or(0);
                if secs > 0 && share.1.is_some_and(|t| t.elapsed() >= Duration::from_secs(secs)) {
                    self.stop_screen_share();
                    share.0 = 2;
                }
            }
        }
        // `RUSTTALK_DEV_FAKE_VIDEO=1`: drawn stand-ins for our own camera and
        // screen tiles (screenshots without a real webcam or desktop).
        #[cfg(debug_assertions)]
        if std::env::var("RUSTTALK_DEV_FAKE_VIDEO").is_ok() && self.in_voice() {
            for kind in [VideoKind::Camera, VideoKind::Screen] {
                let key = (self.srv.my_id, kind);
                if !self.video_frames.contains_key(&key) {
                    self.next_video_view += 1;
                    let view = media_ui::VideoView::spawn(key, self.next_video_view, self.video_tx.clone(), self.ctx.clone());
                    let _ = self.video_tx.send(media_ui::DecodeMsg::Picture(key, view.id, stage::dev_fake_picture(kind)));
                    self.video_frames.insert(key, view);
                }
            }
        }
        match STAGE.load(Ordering::Relaxed) {
            0 => {
                STAGE.store(1, Ordering::Relaxed);
                if std::env::var("RUSTTALK_DEV_AUTOCONNECT").is_ok() {
                    self.start_connect(false);
                }
            }
            1 if self.srv.connected || std::env::var("RUSTTALK_DEV_AUTOCONNECT").is_err() => {
                STAGE.store(2, Ordering::Relaxed);
                let Ok(open) = std::env::var("RUSTTALK_DEV_OPEN") else { return };
                for part in open.split(',') {
                    let (what, arg) = part.split_once(':').unwrap_or((part, ""));
                    let by_name = |s: &Self| s.srv.channels.values().find(|c| c.name == arg).map(|c| c.id);
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
                            if let Some(id) = by_name(self) {
                                self.join_channel(id);
                                self.select_channel(id);
                            }
                        }
                        "poke" => {
                            if let Some(user) = self.srv.users.values().find(|u| u.name == arg).cloned() {
                                self.open_poke(&user);
                            }
                        }
                        // Opens the "⋯" menu on that user's stage tile once it is drawn.
                        "tile_menu" => *views::DEV_TILE_MENU.lock().unwrap() = Some(arg.to_string()),
                        "new_channel" => self.open_new_channel(None, ChannelKind::Text),
                        "gif" => self.open_gif_picker(),
                        // Share the RustTalk window itself (screenshot tests without the desktop).
                        "share_self" => {
                            let sources = video::list_share_sources();
                            let src = sources.into_iter().find(|e| matches!(e.source, ShareSource::Window(_)) && e.title.contains(arg)).map(|e| e.source);
                            // Started once we are really in the voice channel
                            // (in the lobby a running share is stopped at once).
                            match src {
                                Some(src) => self.dev_pending_share = Some(src),
                                None => tracing::warn!("share_self: window '{arg}' not found"),
                            }
                        }
                        // Pick the n-th input device, as the settings dropdown does.
                        "input" => {
                            let n: usize = arg.trim_start_matches('#').parse().unwrap_or(0);
                            if let Some(name) = capture::input_device_names().into_iter().nth(n) {
                                let before = (self.config.input_device.clone(), self.config.output_device.clone());
                                self.config.input_device = Some(name);
                                self.apply_devices(before);
                            }
                        }
                        "claim_admin" => {
                            if let Some(tx) = &self.srv.cmd_tx {
                                let _ = tx.send(Command::Raw(rusttalk_shared::protocol::ClientMessage::ClaimAdmin { password: None }));
                            }
                        }
                        "invite" => {
                            if let Some(tx) = &self.srv.cmd_tx {
                                let _ = tx.send(Command::Raw(rusttalk_shared::protocol::ClientMessage::CreateInvite { max_uses: Some(5), hours: Some(24 * 7) }));
                            }
                        }
                        // Message actions on the last own / last other message of the selected channel.
                        "edit" | "delete" | "reply" => {
                            let want_own = what != "reply";
                            let id = self.srv.selected_channel.and_then(|ch| self.srv.chat.get(&ch)).and_then(|v| {
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
                            "roles" => server_settings::ServerTab::Roles,
                            "bans" => server_settings::ServerTab::Bans,
                            "invites" => server_settings::ServerTab::Invites,
                            "audit" => server_settings::ServerTab::AuditLog,
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
