//! Microphone ownership follows confirmed channel membership, not the TCP connection.

use super::*;

fn set_active<T>(
    slot: &mut Option<T>,
    active: bool,
    start: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<()> {
    if !active {
        *slot = None;
    } else if slot.is_none() {
        *slot = Some(start()?);
    }
    Ok(())
}

fn wants_shared_audio(audio_only: bool, screen: bool, share_screen_audio: bool) -> bool {
    audio_only || (cfg!(any(windows, target_os = "linux")) && screen && share_screen_audio)
}

fn wants_preview(show_settings: bool, page: SettingsPage, in_voice: bool) -> bool {
    show_settings && page == SettingsPage::Voice && !in_voice
}

impl RustTalkApp {
    /// Called only after server state changes (and explicit device changes), never every frame.
    /// Capture exists only for the voice server's confirmed voice channel.
    pub(super) fn sync_voice_audio(&mut self) {
        let voice = self.voice().map(|s| (s.id, s.my_voice_channel()));
        let voice_channel = voice.and_then(|(_, ch)| ch);
        // Without voice the engine only plays notification sounds: keep it on a connected server.
        let usable = |s: &ServerSession| s.connected && s.audio_session.is_some();
        let owner = match voice {
            Some((id, _)) => Some(id),
            None => self
                .audio_owner
                .filter(|id| self.session(*id).is_some_and(usable))
                .or_else(|| self.all_sessions().find(|s| usable(s)).map(|s| s.id)),
        };
        if voice_channel.is_some() {
            self.mic_monitor = None; // release local preview before opening live capture
        } else {
            self.stop_shared_audio();
        }
        if self.audio_owner != owner
            || self.audio.as_ref().is_some_and(|audio| audio.voice_channel != voice_channel)
        {
            self.stop_shared_audio();
            self.audio = None;
            self.talking.lock().unwrap().clear();
        }
        self.audio_owner = owner;
        let result = self.start_audio(voice_channel);
        if result.is_err() && voice_channel.is_some() {
            // A broken microphone must not silence lobby/chat notifications.
            let _ = self.start_audio(None);
        }
        if self.audio.is_none() {
            self.control.transmitting.store(false, Ordering::Relaxed);
            self.talking.lock().unwrap().clear();
            self.sfx.lock().unwrap().clear();
        }
        if let Err(e) = result {
            self.set_error(format!(
                "Audio error: {e:#} (chat still works; rejoin voice to retry)"
            ));
        }
    }

    /// Starts the engine on `audio_owner`'s UDP session (if none is running).
    pub(super) fn start_audio(&mut self, voice_channel: Option<u64>) -> anyhow::Result<()> {
        if self.audio.is_some() {
            return Ok(());
        }
        let Some(id) = self.audio_owner else { return Ok(()) };
        let Some((udp, token, cipher, generation)) = self.session(id).filter(|s| s.connected).and_then(|s| {
            let a = s.audio_session.as_ref()?;
            Some((a.udp.try_clone(), a.token, a.cipher.clone(), s.generation))
        }) else {
            return Ok(());
        };
        let udp = udp?;
        self.adopt_link(id, generation);
        self.audio = Some(AudioEngine::start(
            udp,
            token,
            cipher,
            self.control.clone(),
            self.talking.clone(),
            self.local_muted.clone(),
            self.user_volumes.clone(),
            self.sfx.clone(),
            self.config.input_device.as_deref(),
            self.config.output_device.as_deref(),
            voice_channel,
        )?);
        Ok(())
    }

    pub(super) fn stop_shared_audio(&mut self) {
        self.screen_audio = None;
        self.audio_only = false;
        self.audio_picker = None;
        self.shared_audio_source = Default::default();
        self.screen_audio_failed = true;
    }

    #[cfg(test)]
    pub(super) fn toggle_audio_only(&mut self) {
        if self.audio_only {
            self.stop_shared_audio();
        } else if self.in_voice() {
            self.audio_only = true;
            self.screen_audio_failed = false;
            self.sync_shared_audio();
        }
    }

    pub(super) fn sync_shared_audio(&mut self) {
        if !self.in_voice() {
            self.stop_shared_audio();
            return;
        }
        if self
            .screen_audio
            .as_ref()
            .is_some_and(|audio| audio.failed())
        {
            self.stop_shared_audio();
            self.set_error("Computer audio stopped: capture device or encoder failed. Start sharing again to retry.");
            return;
        }
        let wanted = wants_shared_audio(
            self.audio_only,
            self.screen
                .as_ref()
                .is_some_and(|screen| screen.is_sending()),
            self.config.share_screen_audio,
        );
        if !wanted {
            self.screen_audio = None;
        } else if self.screen_audio.is_none() && !self.screen_audio_failed {
            let result = self
                .audio
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Voice audio is unavailable"))
                .and_then(|engine| {
                    engine.start_shared_audio(
                        self.config.output_device.as_deref(),
                        &self.shared_audio_source,
                    )
                });
            match result {
                Ok(audio) => self.screen_audio = Some(audio),
                Err(e) => {
                    self.stop_shared_audio();
                    self.set_error(format!("Computer audio not shared: {e:#}"));
                }
            }
        }
    }

    /// Tells the voice server whether we share computer audio. Tracks what was queued on this
    /// connection (not the server's echo, which can arrive late), so every change is sent.
    pub(super) fn sync_audio_flag(&mut self) {
        if self.voice().is_none() {
            self.audio_flag_sent = false; // a new voice connection starts as "not sharing"
            return;
        }
        let wanted = self.screen_audio.is_some();
        if wanted != self.audio_flag_sent {
            if let Some(tx) = self.voice_tx() {
                if tx.send(Command::SetAudioSharing(wanted)).is_ok() {
                    self.audio_flag_sent = wanted;
                }
            }
        }
    }

    /// Shared audio of `user` is audible right now (lights up the music note).
    pub(super) fn shared_audio_lit(&self, user: &UserInfo) -> bool {
        if !user.audio_sharing || !self.viewing_voice() {
            return false;
        }
        if user.id == self.srv.my_id {
            return self.screen_audio.as_ref().is_some_and(|a| a.sending());
        }
        self.control
            .stream_meters
            .lock()
            .unwrap()
            .get(&StreamKind::Shared.key(user.id))
            .is_some_and(|m| m.heard.elapsed() < Duration::from_millis(crate::audio::screen_audio::SOUND_LIT_MS))
    }

    pub(super) fn sync_mic_preview(&mut self) {
        let preview = wants_preview(
            self.show_settings,
            self.settings_page,
            self.in_voice(),
        );
        let _ = set_active(&mut self.mic_monitor, preview, || {
            capture::MicMonitor::start(
                self.config.input_device.as_deref(),
                Some(self.control.input_volume_handle()),
            )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::test_devices::Devices;
    use std::net::UdpSocket;

    fn app() -> RustTalkApp {
        RustTalkApp::with_config(
            egui::Context::default(),
            ClientConfig {
                announce_names: false,
                desktop_notifications: false,
                ..ClientConfig::default()
            },
            false,
        )
    }

    fn connected() -> NetEvent {
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        // A connected local socket with no peer traffic is sufficient for event tests.
        udp.connect(udp.local_addr().unwrap()).unwrap();
        NetEvent::Connected {
            my_id: 1,
            settings: ServerSettings::default(),
            users: vec![UserInfo {
                id: 1,
                name: "me".into(),
                channel_id: 1,
                muted: false,
                deafened: false,
                sharing: false,
                audio_sharing: false,
                role: Default::default(),
                streams: vec![],
            }],
            channels: [
                (1, ChannelKind::Spawn),
                (2, ChannelKind::Voice),
                (3, ChannelKind::Text),
                (4, ChannelKind::Voice),
            ]
            .into_iter()
            .map(|(id, kind)| ChannelInfo {
                id,
                name: format!("channel {id}"),
                kind,
                parent: None,
                position: 0,
                user_ids: vec![],
            })
            .collect(),
            udp_socket: Some(udp),
            udp_cipher: Some(Arc::new(
                rusttalk_shared::secure::PacketCipher::new(&[7; 32]).unwrap(),
            )),
            fingerprint: String::new(),
            udp_token: 42,
            spawn_log: vec![],
            chat_history: vec![],
            gif_search: false,
            open_permissions: false,
        }
    }

    fn connect(app: &mut RustTalkApp) -> crate::net::TestCommands {
        if app.srv.is_placeholder() {
            let id = app.ensure_session("voice.invalid:7331").unwrap();
            app.view_session(id);
        }
        let (conn, rx) = crate::net::Connection::test_pair();
        app.srv.cmd_tx = Some(conn);
        app.handle_event(app.srv.id, connected());
        rx
    }

    fn event(app: &mut RustTalkApp, ev: NetEvent) {
        app.handle_event(app.srv.id, ev);
    }

    fn move_to(app: &mut RustTalkApp, channel_id: u64) {
        event(app, NetEvent::Message(ServerMessage::UserMoved {
            client_id: 1,
            channel_id,
        }));
    }

    /// Our own voice join, confirmed by the server (the only way into voice).
    fn join(app: &mut RustTalkApp, channel_id: u64) {
        app.join_channel(channel_id);
        move_to(app, channel_id);
    }

    fn assert_sound(app: &RustTalkApp, sound: Sound) {
        let expected = sfx::render(sound);
        let mut actual = vec![0.0; expected.len()];
        sfx::mix_into(&app.sfx, &mut actual);
        assert_eq!(actual, expected);
        app.sfx.lock().unwrap().clear();
    }

    #[test]
    fn automatic_screen_audio_requires_live_screen_and_opt_in() {
        assert_eq!(
            wants_shared_audio(false, true, true),
            cfg!(any(windows, target_os = "linux"))
        );
        assert!(!wants_shared_audio(false, false, true));
        assert!(!wants_shared_audio(false, true, false));
        assert!(wants_shared_audio(true, false, false));
    }

    #[test]
    fn audio_picker_needs_voice_and_explicit_share_action() {
        use crate::audio::share_source::AudioShareSource;
        let devices = Devices::new();
        let mut app = app();
        let _commands = connect(&mut app);
        app.open_audio_picker();
        app.start_audio_selection(AudioShareSource::System);
        assert!(app.audio_picker.is_none());
        assert_eq!(devices.0.borrow().systems, 0);
        join(&mut app, 2);
        app.open_audio_picker();
        assert!(app.audio_picker.is_some());
        app.sync_shared_audio();
        assert_eq!(devices.0.borrow().systems, 0);
        app.start_audio_selection(AudioShareSource::System);
        assert_eq!(devices.0.borrow().systems, 1);
        assert!(app.audio_picker.is_none());
        app.open_audio_picker();
        move_to(&mut app, 1);
        assert!(app.audio_picker.is_none());
        assert_eq!(devices.0.borrow().systems, 0);
    }

    #[test]
    fn audio_button_opens_the_dialog_while_sharing_and_stop_lives_there() {
        use crate::audio::share_source::AudioShareSource;
        let devices = Devices::new();
        let mut app = app();
        let _commands = connect(&mut app);
        join(&mut app, 2);
        app.toggle_audio_sharing();
        assert!(app.audio_picker.is_some());
        app.start_audio_selection(AudioShareSource::System);
        assert!(app.audio_sharing_active());
        assert_eq!(devices.0.borrow().systems, 1);
        app.toggle_audio_sharing();
        assert!(app.audio_picker.is_some(), "button opens the dialog instead of stopping");
        assert!(app.audio_sharing_active());
        assert_eq!(devices.0.borrow().systems, 1);
        app.toggle_audio_sharing();
        assert!(app.audio_picker.is_none() && app.audio_sharing_active());
        app.stop_shared_audio();
        assert!(app.audio_picker.is_none());
        assert!(!app.audio_sharing_active());
        assert_eq!(devices.0.borrow().systems, 0);
        app.sync_shared_audio();
        assert_eq!(devices.0.borrow().systems, 0, "no automatic resume after stop");
    }

    #[test]
    fn audio_sharing_status_is_sent_once_per_change_and_lights_up_on_sound() {
        use crate::audio::share_source::AudioShareSource;
        let _devices = Devices::new();
        let mut app = app();
        let mut commands = connect(&mut app);
        join(&mut app, 2);
        while commands.try_recv().is_some() {}
        app.sync_audio_flag();
        assert!(commands.try_recv().is_none(), "nothing shared, nothing sent");
        app.start_audio_selection(AudioShareSource::System);
        for _ in 0..3 {
            app.sync_audio_flag();
        }
        assert!(matches!(commands.try_recv(), Some(Command::SetAudioSharing(true))));
        assert!(commands.try_recv().is_none(), "sent once, not every frame");
        event(&mut app, NetEvent::Message(ServerMessage::AudioSharingChanged { client_id: 1, sharing: true }));
        app.sync_audio_flag();
        assert!(commands.try_recv().is_none(), "server already agrees");
        let me = app.srv.users[&1].clone();
        assert!(me.audio_sharing && !app.shared_audio_lit(&me), "sharing but silent");
        app.stop_shared_audio();
        app.sync_audio_flag();
        assert!(matches!(commands.try_recv(), Some(Command::SetAudioSharing(false))));
    }

    #[test]
    fn stopping_before_the_server_echo_still_sends_the_stop() {
        use crate::audio::share_source::AudioShareSource;
        let _devices = Devices::new();
        let mut app = app();
        let mut commands = connect(&mut app);
        join(&mut app, 2);
        while commands.try_recv().is_some() {}
        app.start_audio_selection(AudioShareSource::System);
        app.sync_audio_flag();
        assert!(matches!(commands.try_recv(), Some(Command::SetAudioSharing(true))));
        app.stop_shared_audio(); // before the server answered
        app.sync_audio_flag();
        assert!(matches!(commands.try_recv(), Some(Command::SetAudioSharing(false))));
        event(&mut app, NetEvent::Message(ServerMessage::AudioSharingChanged { client_id: 1, sharing: true })); // late echo
        app.sync_audio_flag();
        assert!(commands.try_recv().is_none(), "the stop is already queued, the server's final state follows it");
    }

    #[test]
    fn audio_only_is_explicit_and_stops_at_membership_and_device_boundaries() {
        let devices = Devices::new();
        let mut app = app();
        let _commands = connect(&mut app);
        app.toggle_audio_only(); // lobby cannot capture
        assert_eq!(devices.0.borrow().systems, 0);
        join(&mut app, 2);
        app.sync_shared_audio();
        assert_eq!(devices.0.borrow().systems, 0); // joining is not consent
        app.control.self_muted.store(true, Ordering::Relaxed);
        app.toggle_audio_only();
        assert_eq!(devices.0.borrow().systems, 1);
        assert_eq!(devices.0.borrow().inputs, 1);
        assert!(app.screen.is_none() && app.cam.is_none());
        app.toggle_audio_only();
        assert_eq!(devices.0.borrow().systems, 0);
        app.toggle_audio_only();
        app.screen_audio.as_ref().unwrap().simulate_failure();
        app.sync_shared_audio();
        assert_eq!(devices.0.borrow().systems, 0);
        assert!(app.status_is_error);
        app.sync_shared_audio();
        assert_eq!(devices.0.borrow().systems, 0);
        app.toggle_audio_only();
        move_to(&mut app, 4);
        app.sync_shared_audio();
        assert_eq!(devices.0.borrow().systems, 0);
        app.toggle_audio_only();
        app.apply_devices((None, None));
        app.sync_shared_audio();
        assert_eq!(devices.0.borrow().systems, 0);
        app.toggle_audio_only();
        move_to(&mut app, 1);
        assert_eq!(devices.0.borrow().systems, 0);
        join(&mut app, 2);
        app.toggle_audio_only();
        event(&mut app, NetEvent::Disconnected {
            reason: "lost".into(),
        });
        assert_eq!(devices.0.borrow().systems, 0);
        let _commands = connect(&mut app);
        join(&mut app, 2);
        app.sync_shared_audio();
        assert_eq!(devices.0.borrow().systems, 0);
        devices.0.borrow_mut().fail_system = true;
        app.toggle_audio_only();
        assert!(app.status_is_error && !app.audio_only);
        devices.0.borrow_mut().fail_system = false;
        app.sync_shared_audio();
        assert_eq!(devices.0.borrow().systems, 0); // failure never retries automatically
        app.toggle_audio_only();
        assert_eq!(devices.0.borrow().systems, 1);
    }

    #[test]
    fn real_events_gate_capture_keep_output_and_restore_leave_chime() {
        let devices = Devices::new();
        let mut app = app();
        let _commands = connect(&mut app);
        assert_eq!(devices.0.borrow().input_starts, 0);
        assert_eq!(devices.0.borrow().outputs, 1);
        assert!(app
            .audio
            .as_ref()
            .unwrap()
            .start_screen_audio(None)
            .is_err());
        for sound in Sound::ALL {
            app.notify(sound);
            assert_sound(&app, sound);
        }
        event(&mut app, NetEvent::Message(ServerMessage::Poked {
            from_id: 2,
            from_name: "peer".into(),
            message: String::new(),
        }));
        assert_sound(&app, Sound::Poke);
        let cipher = app.srv.audio_session.as_ref().unwrap().cipher.clone();
        let session = app.control.link.session();
        app.select_channel(2); // UI selection/request is not server confirmation.
        assert_eq!(devices.0.borrow().input_starts, 0);
        move_to(&mut app, 2); // nor is a server move we did not ask for
        assert_eq!(devices.0.borrow().input_starts, 0);
        move_to(&mut app, 1); // server confirms the bounce to the lobby
        app.srv.bounced_at = None;
        app.join_channel(2);
        assert_eq!(devices.0.borrow().input_starts, 0);
        move_to(&mut app, 2);
        assert_eq!(devices.0.borrow().inputs, 1);
        assert_sound(&app, Sound::Join);
        move_to(&mut app, 4);
        assert_eq!(devices.0.borrow().inputs, 1);
        assert_sound(&app, Sound::Join);
        move_to(&mut app, 1);
        assert_eq!(devices.0.borrow().inputs, 0);
        assert_eq!(devices.0.borrow().outputs, 1);
        assert_sound(&app, Sound::Leave);
        join(&mut app, 2);
        move_to(&mut app, 3); // text channels cannot retain capture
        assert_eq!(devices.0.borrow().inputs, 0);
        join(&mut app, 2);
        event(&mut app, NetEvent::Message(ServerMessage::ChannelDeleted {
            channel_id: 2,
        }));
        assert_eq!(devices.0.borrow().inputs, 0);
        move_to(&mut app, 2); // stale move to a deleted channel
        assert_eq!(devices.0.borrow().inputs, 0);
        assert!(Arc::ptr_eq(
            &cipher,
            &app.srv.audio_session.as_ref().unwrap().cipher
        ));
        assert_eq!(app.control.link.session(), session);
        move_to(&mut app, 4); // after the deletion: needs a new join
        assert_eq!(devices.0.borrow().inputs, 0);
        join(&mut app, 4);
        assert_eq!(devices.0.borrow().inputs, 1);
        event(&mut app, NetEvent::Message(ServerMessage::UserLeft { client_id: 1 }));
        assert_eq!(devices.0.borrow().inputs, 0);
        app.disconnect_all();
        assert_eq!(devices.0.borrow().outputs, 0);
    }

    #[test]
    fn disconnect_reconnect_and_late_events_require_new_confirmation() {
        let devices = Devices::new();
        let mut app = app();
        let _commands = connect(&mut app);
        join(&mut app, 2);
        let old_tx = app.srv.event_tx.clone();
        let session = app.control.link.session();
        let cipher = app.srv.audio_session.as_ref().unwrap().cipher.clone();
        event(&mut app, NetEvent::Disconnected {
            reason: "connection lost".into(),
        });
        assert_eq!(devices.0.borrow().inputs, 0);
        assert_eq!(devices.0.borrow().outputs, 0);
        assert!(old_tx.try_send(connected(), 0).is_err());
        event(&mut app, connected()); // cancelled attempt, no command channel
        move_to(&mut app, 2);
        assert!(!app.srv.connected);
        assert_eq!(devices.0.borrow().inputs, 0);
        let mut commands = connect(&mut app);
        assert!(commands.try_recv().is_none(), "a reconnect never rejoins voice by itself");
        move_to(&mut app, 2); // the server still has us in voice
        assert_eq!(devices.0.borrow().inputs, 0);
        assert_ne!(app.control.link.session(), session);
        assert!(!Arc::ptr_eq(
            &cipher,
            &app.srv.audio_session.as_ref().unwrap().cipher
        ));
        app.srv.bounced_at = None;
        join(&mut app, 2);
        assert_eq!(devices.0.borrow().inputs, 1);
        event(&mut app, NetEvent::Message(ServerMessage::Removed {
            reason: "kicked".into(),
            banned: false,
        }));
        event(&mut app, NetEvent::Disconnected {
            reason: "removed".into(),
        });
        assert_eq!(devices.0.borrow().inputs, 0);
        assert!(app.srv.reconnect.is_none());
    }

    #[test]
    fn device_fallback_and_start_failure_never_open_lobby_capture() {
        let devices = Devices::new();
        let mut app = app();
        let _commands = connect(&mut app);
        app.config.input_device = Some("broken".into());
        app.apply_devices((None, None));
        assert_eq!(devices.0.borrow().input_starts, 0);
        app.config.output_device = Some("broken".into());
        app.apply_devices((Some("broken".into()), None));
        assert!(app.audio.is_some()); // previous output restored
        assert_eq!(devices.0.borrow().input_starts, 0);
        join(&mut app, 2); // input fails; output-only fallback remains available
        assert_eq!(devices.0.borrow().inputs, 0);
        assert_eq!(devices.0.borrow().outputs, 1);
        assert!(app.status_is_error);
        app.sfx.lock().unwrap().clear();
        app.notify(Sound::Poke);
        assert_sound(&app, Sound::Poke);
        app.config.input_device = None;
        app.apply_devices((Some("broken".into()), None)); // recover capture without leaving voice
        assert_eq!(devices.0.borrow().inputs, 1);
        app.config.input_device = Some("broken".into());
        app.apply_devices((None, None));
        assert_eq!(devices.0.borrow().inputs, 1); // old device restored
        assert!(app.config.input_device.is_none());
        devices.0.borrow_mut().fail_input = true;
        app.apply_devices((None, None));
        assert_eq!(devices.0.borrow().inputs, 0);
        assert_eq!(devices.0.borrow().outputs, 1);
        devices.0.borrow_mut().fail_input = false;
        app.apply_devices((None, None));
        assert_eq!(devices.0.borrow().inputs, 1);
        devices.0.borrow_mut().fail_output = true;
        app.apply_devices((None, None));
        assert_eq!(devices.0.borrow().inputs, 0); // both attempts failed; no capture survives
        assert!(app.audio.is_none());
        devices.0.borrow_mut().fail_output = false;
        move_to(&mut app, 1);
        assert_eq!(devices.0.borrow().inputs, 0);
        assert_eq!(devices.0.borrow().outputs, 1);
    }

    #[test]
    fn real_preview_factory_only_opens_on_explicit_voice_page() {
        let devices = Devices::new();
        let mut app = app();
        let _commands = connect(&mut app);
        app.show_settings = true;
        for (page, expected) in [
            (SettingsPage::Appearance, 0),
            (SettingsPage::Notifications, 0),
            (SettingsPage::Voice, 1),
            (SettingsPage::Notifications, 0),
            (SettingsPage::Voice, 1),
        ] {
            app.settings_page = page;
            app.sync_mic_preview();
            assert_eq!(devices.0.borrow().inputs, expected);
        }
        join(&mut app, 2);
        app.sync_mic_preview();
        assert!(app.mic_monitor.is_none());
        assert_eq!(devices.0.borrow().inputs, 1);
        move_to(&mut app, 1);
        app.sync_mic_preview();
        assert!(app.mic_monitor.is_some());
        app.show_settings = false;
        app.sync_mic_preview();
        assert_eq!(devices.0.borrow().inputs, 0);
        assert_eq!(devices.0.borrow().outputs, 1);
    }
}
