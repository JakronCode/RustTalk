//! Several servers at once: which one is viewed, connecting and disconnecting
//! (persistent per server), reconnecting enrolled servers at startup, retries
//! within limits, and the single voice session.
//!
//! API for the server switcher (`server_switcher.rs`): `server_rows`,
//! `select_server`, `connect_server`, `disconnect_server`, `voice_server`.

use super::session::{ServerRow, ServerStatus};
use super::*;

impl RustTalkApp {
    // ----- Sessions -----------------------------------------------------

    pub(super) fn all_sessions(&self) -> impl Iterator<Item = &ServerSession> {
        std::iter::once(&self.srv).chain(self.others.iter())
    }

    pub(super) fn session(&self, id: SessionId) -> Option<&ServerSession> {
        self.all_sessions().find(|s| s.id == id)
    }

    pub(super) fn session_mut(&mut self, id: SessionId) -> Option<&mut ServerSession> {
        if self.srv.id == id {
            Some(&mut self.srv)
        } else {
            self.others.iter_mut().find(|s| s.id == id)
        }
    }

    fn session_ids(&self) -> Vec<SessionId> {
        self.all_sessions().map(|s| s.id).collect()
    }

    pub(super) fn session_by_key(&self, key: &str) -> Option<SessionId> {
        self.all_sessions()
            .find(|s| !key.is_empty() && s.key == key)
            .map(|s| s.id)
    }

    /// Some server is logged in (global shortcuts, update warnings).
    pub(super) fn any_connected(&self) -> bool {
        self.all_sessions().any(|s| s.connected)
    }

    pub(super) fn bookmark_for(&self, key: &str) -> Option<&ServerBookmark> {
        self.config
            .bookmarks
            .iter()
            .find(|b| !key.is_empty() && net::normalize_addr(b.address.trim()) == key)
    }

    /// Name for messages about server `id`: its name, nickname or address.
    pub(super) fn server_label(&self, id: SessionId) -> String {
        let Some(s) = self.session(id) else {
            return String::new();
        };
        s.display_name(self.bookmark_for(&s.key).and_then(|b| b.nickname()))
    }

    /// Text from server `id`, prefixed with its name unless it is the viewed one.
    pub(super) fn server_text(&self, id: SessionId, text: String) -> String {
        if self.srv.id == id {
            text
        } else {
            format!("{}: {text}", self.server_label(id))
        }
    }

    /// The session for `address`, created (disconnected) if there is none.
    pub(super) fn ensure_session(&mut self, address: &str) -> Option<SessionId> {
        let address = address.trim();
        if address.is_empty() {
            return None;
        }
        let key = net::normalize_addr(address);
        if let Some(id) = self.session_by_key(&key) {
            return Some(id);
        }
        let id = self.next_session_id;
        self.next_session_id += 1;
        let mut s = ServerSession::new(id, address);
        s.password = self.bookmark_for(&key).and_then(|b| b.password.clone());
        self.others.push(s);
        Some(id)
    }

    /// Network events of every server, each applied to its own session.
    pub(super) fn poll_sessions(&mut self) {
        for id in self.session_ids() {
            for n in 0.. {
                if n == EVENTS_PER_FRAME {
                    self.ctx.request_repaint(); // the rest next frame
                    break;
                }
                let Some(ev) = self.session(id).and_then(|s| s.event_rx.try_recv().ok()) else {
                    break;
                };
                self.handle_event(id, ev);
            }
        }
    }

    /// Forgets background sessions that hold nothing (rows come from bookmarks).
    pub(super) fn prune_sessions(&mut self) {
        let keep = [self.voice_owner, self.audio_owner];
        self.others
            .retain(|s| !s.is_disposable() || keep.contains(&Some(s.id)));
    }

    // ----- Server switcher ------------------------------------------------

    /// Saved servers first (in their order), then live servers that are not saved.
    pub(super) fn server_rows(&self) -> Vec<ServerRow> {
        let voice = self.voice().map(|s| s.id);
        let row = |key: String,
                   address: &str,
                   nickname: Option<&str>,
                   saved: bool,
                   s: Option<&ServerSession>| ServerRow {
            name: s
                .map(|s| s.display_name(nickname))
                .unwrap_or_else(|| nickname.unwrap_or(address).to_string()),
            address: address.to_string(),
            saved,
            status: s.map_or(ServerStatus::Disconnected, ServerSession::status),
            unread: s.is_some_and(|s| !s.unread.is_empty()),
            voice: s.is_some_and(|s| Some(s.id) == voice),
            viewed: s.is_some_and(|s| s.id == self.srv.id),
            auto_connect: self.config.auto_connect.contains(&key),
            error: s.and_then(|s| match s.status() {
                ServerStatus::Retrying | ServerStatus::LimitReached | ServerStatus::Failed => {
                    s.last_error.clone()
                }
                _ => None,
            }),
            key,
        };
        let mut rows: Vec<ServerRow> = Vec::new();
        for bm in &self.config.bookmarks {
            let key = net::normalize_addr(bm.address.trim());
            if key.is_empty() || rows.iter().any(|r| r.key == key) {
                continue;
            }
            let s = self.session_by_key(&key).and_then(|id| self.session(id));
            rows.push(row(key, &bm.address, bm.nickname(), true, s));
        }
        for s in self.all_sessions().filter(|s| !s.is_placeholder()) {
            if !rows.iter().any(|r| r.key == s.key) {
                rows.push(row(s.key.clone(), &s.address, None, false, Some(s)));
            }
        }
        rows
    }

    /// Shows another server. Never connects, disconnects or moves voice.
    pub(super) fn select_server(&mut self, key: &str) {
        let id = match self.session_by_key(key) {
            Some(id) => Some(id),
            None => match self.bookmark_for(key).map(|b| b.address.clone()) {
                Some(address) => self.ensure_session(&address),
                None => None,
            },
        };
        if let Some(id) = id {
            self.view_session(id);
        }
    }

    pub(super) fn view_session(&mut self, id: SessionId) {
        if self.srv.id == id {
            return;
        }
        let Some(i) = self.others.iter().position(|s| s.id == id) else {
            return;
        };
        self.reset_view_ui();
        self.srv.draft = std::mem::take(&mut self.chat_input);
        let next = self.others.remove(i);
        let prev = std::mem::replace(&mut self.srv, next);
        if !prev.is_disposable() || [self.voice_owner, self.audio_owner].contains(&Some(prev.id)) {
            self.others.push(prev);
        }
        self.chat_input = std::mem::take(&mut self.srv.draft);
        // The connect form follows the viewed server.
        self.config.server_address = self.srv.address.clone();
        self.config.server_password = self.srv.password.clone();
        self.config.invite = self.srv.invite.clone();
        self.server_nickname = self
            .bookmark_for(&self.srv.key)
            .and_then(|b| b.nickname())
            .unwrap_or_default()
            .to_string();
        self.editing_bookmark = None;
        if self.srv.connected {
            self.prepare_all_names();
        }
    }

    /// Dialogs, drafts and pickers that point at one server's channels,
    /// messages or users: closed when the view or the connection changes, so
    /// they can never act on another server.
    pub(super) fn reset_view_ui(&mut self) {
        self.channel_settings = None;
        self.server_settings_dialog = None;
        self.poke_dialog = None;
        self.confirm_delete = None;
        self.show_new_channel = false;
        self.reply_to = None;
        self.editing = None;
        self.edit_text.clear();
        self.confirm_delete_msg = None;
        self.chat_search = None;
        self.chat_action = None;
        self.close_gif_picker();
        self.gif_target = None;
        self.mod_dialog = None;
        self.pending_image = None;
        self.connection_panel = None;
        self.cam_dialog = None;
        self.screen_picker = None;
        self.audio_picker = None;
        if !self.cam.as_ref().is_some_and(|c| c.is_sending()) {
            self.cam = None; // camera preview of the closed dialog
            *self.local_preview.lock().unwrap() = None;
        }
        self.leave_fullscreen();
    }

    /// Connects a server. With `remember` (the user chose "Connect and
    /// remember") a successful login enrolls the saved server for startup;
    /// otherwise it is not enrolled. An unknown certificate still needs the
    /// user's trust on the connect screen before anything is sent.
    pub(super) fn connect_server(&mut self, key: &str, remember: bool) {
        let address = match self.session_by_key(key) {
            Some(id) => self.session(id).map(|s| s.address.clone()),
            None => self.bookmark_for(key).map(|b| b.address.clone()),
        };
        let Some(id) = address.and_then(|a| self.ensure_session(&a)) else {
            return;
        };
        let Some(s) = self.session_mut(id) else {
            return;
        };
        if s.cmd_tx.is_some() {
            return;
        }
        s.remember = remember;
        if let Some(rc) = s.reconnect.as_mut() {
            rc.next_at = None; // a manual attempt replaces the scheduled one
        }
        if !self.begin_connect(id) {
            let msg = self
                .session(id)
                .and_then(|s| s.last_error.clone())
                .unwrap_or_default();
            let msg = self.server_text(id, msg);
            self.set_error(msg);
        }
    }

    /// Connect screen: connects (or shows, if it is open already) the server
    /// in the address field and views it. `remember` as in `connect_server`.
    pub(super) fn start_connect(&mut self, remember: bool) {
        // An invite link in the address field: address + invite code.
        if let Some((addr, code)) = moderation_ui::parse_invite_link(&self.config.server_address) {
            self.config.server_address = addr;
            self.config.invite = code;
        }
        let address = self.config.server_address.trim().to_string();
        let Some(id) = self.ensure_session(&address) else {
            self.set_error("Enter a server address");
            return;
        };
        let (password, invite) = (
            self.config.server_password.clone(),
            self.config.invite.clone(),
        );
        if let Some(s) = self.session_mut(id) {
            s.address = address;
            s.password = password;
            s.invite = invite;
        }
        self.view_session(id);
        if self.srv.cmd_tx.is_some() {
            return; // connected or connecting already
        }
        self.srv.remember = remember;
        if self.begin_connect(id) {
            self.set_status(format!("Connecting to {}...", self.srv.address));
        } else {
            let msg = self.srv.last_error.clone().unwrap_or_default();
            self.set_error(msg);
        }
        self.config.save();
    }

    /// Starts a connection attempt for `id` (fails at the session limit).
    fn begin_connect(&mut self, id: SessionId) -> bool {
        let active = self
            .all_sessions()
            .filter(|s| s.id != id && s.is_active())
            .count();
        self.control.set_vad_threshold(self.config.vad_threshold);
        self.control
            .voice_activation
            .store(self.config.voice_activation, Ordering::Relaxed);
        self.control
            .noise_suppression
            .store(self.config.noise_suppression, Ordering::Relaxed);
        self.control
            .auto_gain
            .store(self.config.auto_gain, Ordering::Relaxed);
        self.control
            .echo_cancellation
            .store(self.config.echo_cancellation, Ordering::Relaxed);
        let mut cfg = self.config.clone();
        let ctx = self.ctx.clone();
        let Some(s) = self.session_mut(id) else {
            return false;
        };
        if active >= MAX_ACTIVE_SESSIONS {
            s.reconnect = None;
            s.remember = false;
            s.overflow = true;
            s.last_error = Some(format!(
                "Not connected: at most {MAX_ACTIVE_SESSIONS} servers can be open at once"
            ));
            return false;
        }
        if let Some(tx) = s.cmd_tx.take() {
            let _ = tx.send(Command::Disconnect);
        }
        s.reset_events();
        s.generation += 1;
        s.connected = false;
        s.overflow = false;
        s.removed = None;
        s.cert_check = None;
        s.join_error = None;
        s.last_error = None;
        cfg.server_address = s.address.clone();
        cfg.server_password = s.password.clone();
        cfg.invite = s.invite.clone();
        // New counter per connection (the old queue is gone with it).
        s.upload_backlog = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        s.cmd_tx = Some(net::connect(
            cfg,
            s.event_tx.clone(),
            ctx,
            s.upload_backlog.clone(),
        ));
        true
    }

    /// A login the user asked for: reconnect this saved server on start from now on.
    pub(super) fn enroll(&mut self, key: &str, name: &str) {
        if self.bookmark_for(key).is_none() || !self.config.auto_connect.insert(key.to_string()) {
            return;
        }
        self.config.save();
        self.set_status(format!("Connected to {name}. RustTalk connects to it again when it starts, until you disconnect it."));
    }

    /// App start, and mobile resume: connects every saved server the user
    /// joined (and did not disconnect) whose certificate is pinned. Lobby and
    /// chat only, never voice or media. Repeating it starts nothing twice and
    /// leaves servers alone that wait for the user (password, ban, certificate).
    pub(super) fn connect_enrolled_servers(&mut self) {
        let bookmarks = self.config.bookmarks.clone();
        let keys: HashSet<String> = bookmarks
            .iter()
            .map(|b| net::normalize_addr(b.address.trim()))
            .collect();
        self.config.auto_connect.retain(|k| keys.contains(k)); // saved server removed: consent gone too
        let mut first = None;
        for bm in &bookmarks {
            let key = net::normalize_addr(bm.address.trim());
            if !self.config.auto_connect.contains(&key)
                || !self.config.known_servers.contains_key(&key)
            {
                continue;
            }
            let Some(id) = self.ensure_session(&bm.address) else {
                continue;
            };
            first.get_or_insert(id);
            let active = self.all_sessions().filter(|s| s.is_active()).count();
            let Some(s) = self.session_mut(id) else {
                continue;
            };
            let needs_user =
                s.cert_check.is_some() || s.join_error.as_ref().is_some_and(|e| e.is_fatal());
            if s.is_active() || needs_user {
                continue;
            }
            if active >= MAX_ACTIVE_SESSIONS {
                s.overflow = true;
                s.last_error = Some(format!(
                    "Not connected: at most {MAX_ACTIVE_SESSIONS} servers can be open at once"
                ));
                continue;
            }
            s.overflow = false;
            s.password = bm.password.clone();
            s.reconnect = Some(Reconnect {
                attempt: 1,
                next_at: Some(Instant::now()),
                channel: None,
            });
        }
        // Nothing picked yet: show the last used server, else the first one.
        if self.srv.is_placeholder() {
            let last = self.session_by_key(&net::normalize_addr(self.config.server_address.trim()));
            if let Some(id) = last
                .filter(|id| self.session(*id).is_some_and(|s| s.is_active()))
                .or(first)
            {
                self.view_session(id);
            }
        }
        self.poll_reconnect();
    }

    /// Starts automatic attempts that are due, at most `MAX_AUTO_CONNECTING` at once.
    pub(super) fn poll_reconnect(&mut self) {
        let now = Instant::now();
        let mut running = self.all_sessions().filter(|s| s.is_connecting()).count();
        for id in self.session_ids() {
            if running >= MAX_AUTO_CONNECTING {
                break;
            }
            let Some(s) = self.session_mut(id) else {
                continue;
            };
            let due = s.cmd_tx.is_none()
                && s.reconnect
                    .as_ref()
                    .and_then(|rc| rc.next_at)
                    .is_some_and(|t| now >= t);
            if !due {
                continue;
            }
            if let Some(rc) = s.reconnect.as_mut() {
                rc.next_at = None;
            }
            if self.begin_connect(id) {
                running += 1;
            }
        }
    }

    /// Cancels a pending automatic reconnect of the viewed server (connect screen).
    pub(super) fn cancel_reconnect(&mut self) {
        if self.srv.reconnect.take().is_some() {
            if self.srv.cmd_tx.is_some() && !self.srv.connected {
                self.drop_connection(self.srv.id); // stop the running attempt
            }
            self.set_status("Reconnect cancelled");
        }
    }

    // ----- Disconnecting ------------------------------------------------

    /// Ends one connection and what depended on it (voice, media, dialogs).
    /// Other servers stay connected.
    fn drop_connection(&mut self, id: SessionId) {
        if self.voice_owner == Some(id) {
            self.end_voice_media();
            self.set_voice_owner(None);
        }
        if self.srv.id == id {
            self.reset_view_ui();
        }
        if let Some(s) = self.session_mut(id) {
            s.reset_events(); // late events of this connection are dropped
            if let Some(tx) = s.cmd_tx.take() {
                let _ = tx.send(Command::Disconnect);
            }
            s.clear_server_state();
        }
        if self.audio_owner == Some(id) {
            self.stop_shared_audio();
            self.audio = None; // releases capture and joins the audio threads
            self.audio_owner = None;
        }
        if self.link_conn.is_some_and(|(owner, _)| owner == id) {
            self.link_conn = None; // its replay window ends with the connection
        }
        self.sync_voice_audio(); // notification sounds move to a server still connected
    }

    /// Server switcher and Disconnect: ends the connection and keeps the
    /// server disconnected, also after a restart, until the user connects again.
    pub(super) fn disconnect_server(&mut self, key: &str) {
        if self.config.auto_connect.remove(key) {
            self.config.save();
        }
        let Some(id) = self.session_by_key(key) else {
            return;
        };
        self.drop_connection(id);
        if let Some(s) = self.session_mut(id) {
            s.reconnect = None;
            s.remember = false;
            s.overflow = false;
            s.cert_check = None;
            s.join_error = None;
            s.last_error = None;
        }
        if self.srv.id == id {
            self.set_status("Disconnected");
            // Stay with a community that is still connected.
            if let Some(next) = self.others.iter().find(|s| s.connected).map(|s| s.id) {
                self.view_session(next);
            }
        }
    }

    /// Every server, without changing what reconnects at startup (update restart).
    pub(super) fn disconnect_all(&mut self) {
        for id in self.session_ids() {
            self.drop_connection(id);
            if let Some(s) = self.session_mut(id) {
                s.reconnect = None;
            }
        }
        self.mic_monitor = None;
        self.sfx.lock().unwrap().clear();
    }

    /// Error about server `id`: toast when viewed, prefixed with its name otherwise.
    fn report(&mut self, id: SessionId, msg: String) {
        let msg = self.server_text(id, msg);
        self.set_error(msg);
    }

    /// Unknown or changed certificate: never go on by ourselves - the user decides.
    pub(super) fn on_cert_check(&mut self, id: SessionId, check: join_error::CertCheck) {
        if self.session(id).is_none_or(|s| s.cmd_tx.is_none()) {
            return; // attempt was cancelled meanwhile
        }
        self.drop_connection(id);
        let msg = if check.changed {
            "The server's identity changed"
        } else {
            "New server - trust it to connect"
        };
        if let Some(s) = self.session_mut(id) {
            s.reconnect = None;
            s.join_error = Some(join_error::JoinError::cert_check(
                &check.fingerprint,
                check.changed,
            ));
            s.last_error = Some(msg.to_string());
            s.cert_check = Some(check);
        }
        self.report(id, msg.to_string());
    }

    /// Network thread of server `id` ended. After a successful login this
    /// starts the automatic reconnect; a failed attempt schedules the next one.
    pub(super) fn on_connection_lost(&mut self, id: SessionId, reason: String) {
        let viewed = self.srv.id == id;
        let voice = self.voice_owner == Some(id);
        let Some(s) = self.session_mut(id) else {
            return;
        };
        // Kicked or banned: no automatic reconnect.
        if let Some(why) = s.removed.take() {
            s.reconnect = None;
            self.drop_connection(id);
            if let Some(s) = self.session_mut(id) {
                s.join_error = Some(join_error::JoinError::classify(&why));
                s.last_error = Some(why.clone());
            }
            self.report(id, why);
            return;
        }
        let was_connected = s.connected;
        if was_connected && s.reconnect.is_none() {
            let channel = s
                .my_voice_channel()
                .filter(|_| voice)
                .map(|c| (c, s.channel_name(c)));
            s.reconnect = Some(Reconnect {
                attempt: 1,
                next_at: None,
                channel,
            });
        }
        let failure = join_error::JoinError::classify(&reason);
        // Wrong password, ban, version mismatch: retrying won't help.
        if failure.is_fatal() {
            s.reconnect = None;
        }
        self.drop_connection(id);
        let Some(s) = self.session_mut(id) else {
            return;
        };
        s.last_error = Some(reason.clone());
        let Some(rc) = s.reconnect.as_mut() else {
            s.join_error = Some(failure);
            self.report(id, format!("Disconnected: {reason}"));
            return;
        };
        if !was_connected {
            rc.attempt += 1; // the running attempt failed
        }
        if rc.attempt > MAX_RECONNECT_ATTEMPTS {
            s.reconnect = None;
            let msg = format!("{reason} (gave up reconnecting)");
            s.join_error = Some(join_error::JoinError::classify(&msg));
            self.report(id, format!("Disconnected: {msg}"));
            return;
        }
        let delay = reconnect_delay(rc.attempt) + retry_stagger(id);
        rc.next_at = Some(Instant::now() + delay);
        let attempt = rc.attempt;
        // Background servers retry quietly; the switcher shows their state.
        if viewed {
            self.set_error(format!(
                "Connection lost ({reason}) - reconnecting in {} s (attempt {attempt}/{MAX_RECONNECT_ATTEMPTS})",
                delay.as_secs()
            ));
        }
    }

    /// After a successful reconnect: back into the old voice channel, unless
    /// voice moved to another server meanwhile. Never after a startup connect.
    pub(super) fn restore_after_reconnect(&mut self, id: SessionId, rc: Reconnect) {
        let viewed = self.srv.id == id;
        if let (Some((channel, name)), false) = (rc.channel, self.voice_busy_elsewhere(id)) {
            let target = self.session(id).and_then(|s| {
                let ch = s
                    .channels
                    .get(&channel)
                    .filter(|c| c.kind == ChannelKind::Voice && c.name == name)
                    .or_else(|| {
                        s.channels
                            .values()
                            .find(|c| c.kind == ChannelKind::Voice && c.name == name)
                    })?;
                Some((ch.id, s.cmd_tx.clone()?))
            });
            if let Some((target, tx)) = target {
                self.set_voice_owner(Some(id));
                let _ = tx.send(Command::JoinChannel(target));
                if viewed {
                    self.select_channel(target);
                } else if let Some(s) = self.session_mut(id) {
                    s.selected_channel = Some(target);
                }
            }
        }
        if viewed {
            let name = self.srv.server_settings.name.clone();
            self.set_status(format!("Reconnected to {name}"));
        }
    }

    // ----- Voice --------------------------------------------------------

    /// The server we are in a voice channel on (at most one).
    pub(super) fn voice(&self) -> Option<&ServerSession> {
        self.voice_owner
            .and_then(|id| self.session(id))
            .filter(|s| s.connected && s.my_voice_channel().is_some())
    }

    /// Name of the server that has voice, for "voice on ..." indicators.
    pub(super) fn voice_server(&self) -> Option<(String, String)> {
        let s = self.voice()?;
        Some((s.key.clone(), self.server_label(s.id)))
    }

    pub(super) fn in_voice(&self) -> bool {
        self.voice().is_some()
    }

    pub(super) fn voice_channel(&self) -> Option<u64> {
        self.voice()?.my_voice_channel()
    }

    /// Commands for the voice server (camera, screen, watching, keyframes).
    pub(super) fn voice_tx(&self) -> Option<tokio::sync::mpsc::UnboundedSender<Command>> {
        self.voice_owner
            .and_then(|id| self.session(id))
            .and_then(|s| s.cmd_tx.clone())
    }

    /// Event queue of the voice server's connection (local media errors and own tiles).
    pub(super) fn voice_events(&self) -> Option<std::sync::mpsc::SyncSender<NetEvent>> {
        self.voice_owner
            .and_then(|id| self.session(id))
            .map(|s| s.event_tx.clone())
    }

    /// Our client id on the voice server (0 = none).
    pub(super) fn voice_my_id(&self) -> u64 {
        self.voice_owner
            .and_then(|id| self.session(id))
            .map_or(0, |s| s.my_id)
    }

    /// The viewed server is the voice server: video tiles, talking rings and
    /// per-user mixer controls describe its users.
    pub(super) fn viewing_voice(&self) -> bool {
        self.voice_owner == Some(self.srv.id)
    }

    /// Talking times and local mutes for drawing the viewed server's users:
    /// empty unless it is the voice server (they are keyed by its client ids).
    pub(super) fn voice_maps(&self) -> (HashMap<u64, Instant>, HashSet<u64>) {
        if !self.viewing_voice() {
            return Default::default();
        }
        (
            self.talking.lock().unwrap().clone(),
            self.local_muted.lock().unwrap().clone(),
        )
    }

    /// Another server than `id` has us in voice.
    pub(super) fn voice_busy_elsewhere(&self, id: SessionId) -> bool {
        self.voice_owner.is_some_and(|owner| owner != id) && self.in_voice()
    }

    /// Makes `id` the voice server. Mixer state is keyed by client ids, so it
    /// starts fresh for that server's users.
    pub(super) fn set_voice_owner(&mut self, id: Option<SessionId>) {
        if self.voice_owner == id {
            return;
        }
        self.voice_owner = id;
        self.talking.lock().unwrap().clear();
        self.local_muted.lock().unwrap().clear();
        self.user_volumes.lock().unwrap().clear();
        let users: Vec<(u64, String)> = id
            .and_then(|id| self.session(id))
            .map(|s| s.users.values().map(|u| (u.id, u.name.clone())).collect())
            .unwrap_or_default();
        for (uid, name) in users {
            self.apply_user_volume(uid, &name);
            self.prepare_name(&name);
        }
    }

    /// The UI asks to join `channel_id` on the viewed server. Joining voice
    /// here first leaves voice on any other server; its camera, screen and
    /// audio sharing stop and do not restart.
    pub(super) fn join_channel(&mut self, channel_id: u64) {
        let id = self.srv.id;
        let Some(tx) = self.srv.cmd_tx.clone().filter(|_| self.srv.connected) else {
            return;
        };
        if self.srv.channel_kind(channel_id) == Some(ChannelKind::Voice) {
            if let Some(owner) = self.voice_owner.filter(|owner| *owner != id) {
                if !self.leave_voice(owner) {
                    return;
                }
            }
            self.set_voice_owner(Some(id));
        }
        let _ = tx.send(Command::JoinChannel(channel_id));
    }

    /// Leaves voice on `owner`: media stops first, then its server moves us to the lobby.
    pub(super) fn leave_voice(&mut self, owner: SessionId) -> bool {
        let in_voice = self
            .session(owner)
            .filter(|s| s.connected && s.my_voice_channel().is_some());
        let lobby = in_voice.map(|s| (s.cmd_tx.clone(), s.spawn_channel_id()));
        if let Some((None, _) | (_, None)) = lobby {
            let msg = format!("Could not leave voice on {}", self.server_label(owner));
            self.set_error(msg);
            return false;
        }
        self.end_voice_media();
        if let Some((Some(tx), Some(spawn))) = lobby {
            let _ = tx.send(Command::JoinChannel(spawn));
        }
        if let Some(rc) = self.session_mut(owner).and_then(|s| s.reconnect.as_mut()) {
            rc.channel = None; // a pending reconnect must not take voice back
        }
        self.set_voice_owner(None);
        self.sync_voice_audio(); // notification sounds keep playing meanwhile
        true
    }

    /// Stops everything that belongs to voice: own camera, screen and audio
    /// sharing (the voice server is told), capture, video tiles, watching.
    pub(super) fn end_voice_media(&mut self) {
        self.stop_own_sources();
        self.cam = None;
        self.screen = None;
        self.cam_dialog = None;
        self.screen_picker = None;
        self.audio_picker = None;
        self.dev_pending_share = None;
        *self.local_preview.lock().unwrap() = None;
        self.local_preview_tex = None;
        self.video_frames.clear();
        self.watching.clear();
        self.video_stopped.clear();
        for e in self.video_epoch.iter() {
            e.fetch_add(1, Ordering::Relaxed);
        }
        self.leave_fullscreen();
        self.connection_panel = None;
        if self.audio_owner.is_some() && self.audio_owner == self.voice_owner {
            self.audio = None; // capture of this server; sync starts output-only again
        }
        self.talking.lock().unwrap().clear();
        self.control.transmitting.store(false, Ordering::Relaxed);
    }

    /// Mute and deafen are global (one microphone): every connected server shows them.
    pub(super) fn broadcast_mute(&self) {
        let muted = self.control.self_muted.load(Ordering::Relaxed);
        let deafened = self.control.self_deafened.load(Ordering::Relaxed);
        for tx in self
            .all_sessions()
            .filter(|s| s.connected)
            .filter_map(|s| s.cmd_tx.as_ref())
        {
            let _ = tx.send(Command::SetMute { muted, deafened });
        }
    }

    /// The audio engine about to start uses connection (`id`, `generation`):
    /// hand it that connection's replay window, keep the previous one's.
    pub(super) fn adopt_link(&mut self, id: SessionId, generation: u64) {
        if self.link_conn == Some((id, generation)) {
            return;
        }
        let window = self
            .session_mut(id)
            .and_then(|s| s.replay.take())
            .unwrap_or_default();
        let old = self.control.link.switch(window);
        if let Some((prev, prev_generation)) = self.link_conn {
            if let Some(p) = self
                .session_mut(prev)
                .filter(|p| p.generation == prev_generation && p.connected)
            {
                p.replay = Some(old);
            }
        }
        self.link_conn = Some((id, generation));
    }

    /// Admin tabs of server `id` need fresh lists.
    pub(super) fn request_admin_data_for(&self, id: SessionId) {
        let Some(s) = self
            .session(id)
            .filter(|s| s.can(rusttalk_shared::protocol::Role::Admin))
        else {
            return;
        };
        if let Some(tx) = &s.cmd_tx {
            let _ = tx.send(Command::Raw(
                rusttalk_shared::protocol::ClientMessage::RequestAdminData,
            ));
        }
    }
}

#[cfg(test)]
#[path = "servers_tests.rs"]
pub(in crate::app) mod tests;
