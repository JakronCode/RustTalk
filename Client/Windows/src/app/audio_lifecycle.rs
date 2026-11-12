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

fn wants_preview(show_settings: bool, page: SettingsPage, in_voice: bool) -> bool {
    show_settings && page == SettingsPage::Voice && !in_voice
}

impl RustTalkApp {
    /// Called only after server state changes (and explicit device changes), never every frame.
    pub(super) fn sync_voice_audio(&mut self) {
        let in_voice = self.connected && self.my_voice_channel().is_some();
        if in_voice {
            self.mic_monitor = None; // release local preview before opening live capture
        } else {
            self.screen_audio = None;
        }
        let session = self.audio_session.as_ref();
        let result = set_active(&mut self.audio, in_voice && session.is_some(), || {
            let session = session.expect("checked above");
            self.sfx.lock().unwrap().clear();
            AudioEngine::start(
                session.udp.try_clone()?,
                session.token,
                session.cipher.clone(),
                self.control.clone(),
                self.talking.clone(),
                self.local_muted.clone(),
                self.user_volumes.clone(),
                self.sfx.clone(),
                self.config.input_device.as_deref(),
                self.config.output_device.as_deref(),
            )
        });
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

    pub(super) fn sync_mic_preview(&mut self) {
        let preview = wants_preview(
            self.show_settings,
            self.settings_page,
            self.my_voice_channel().is_some(),
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
    use std::cell::Cell;

    struct Capture<'a>(&'a Cell<usize>);
    impl Drop for Capture<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }

    #[test]
    fn lobby_voice_exit_device_change_and_reconnect_release_capture() {
        let live = Cell::new(0);
        let starts = Cell::new(0);
        let start = || {
            starts.set(starts.get() + 1);
            live.set(live.get() + 1);
            Ok(Capture(&live))
        };
        let mut audio = None;
        // Lobby connect, device changes and reconnect must never call the input factory.
        for _ in 0..3 {
            set_active(&mut audio, false, start).unwrap();
        }
        assert_eq!(starts.get(), 0);
        set_active(&mut audio, true, start).unwrap(); // server-confirmed voice join
        assert_eq!(live.get(), 1);
        set_active(&mut audio, true, start).unwrap(); // voice-to-voice: keep engine
        assert_eq!(starts.get(), 1);
        for _ in 0..3 {
            // lobby/text move, kick/disconnect: drop synchronously
            set_active(&mut audio, false, start).unwrap();
            assert_eq!(live.get(), 0);
            set_active(&mut audio, true, start).unwrap();
            assert_eq!(live.get(), 1);
        }
        drop(audio);
        assert_eq!(live.get(), 0);
    }

    #[test]
    fn failed_capture_stays_absent_and_next_join_retries() {
        let mut audio: Option<()> = None;
        assert!(set_active(&mut audio, true, || anyhow::bail!("input unavailable")).is_err());
        assert!(audio.is_none());
        set_active(&mut audio, false, || panic!("lobby must not open input")).unwrap();
        set_active(&mut audio, true, || Ok(())).unwrap();
        assert!(audio.is_some());
    }

    #[test]
    fn preview_only_on_voice_page_and_never_alongside_live_voice() {
        let live = Cell::new(0);
        let mut monitor = None;
        for (show, page, voice, expected) in [
            (true, SettingsPage::Appearance, false, 0),
            (true, SettingsPage::Voice, false, 1),
            (true, SettingsPage::Notifications, false, 0),
            (true, SettingsPage::Voice, false, 1),
            (false, SettingsPage::Voice, false, 0),
            (true, SettingsPage::Voice, false, 1),
            (true, SettingsPage::Voice, true, 0),
        ] {
            set_active(&mut monitor, wants_preview(show, page, voice), || {
                live.set(live.get() + 1);
                Ok(Capture(&live))
            })
            .unwrap();
            assert_eq!(live.get(), expected);
        }
    }
}
