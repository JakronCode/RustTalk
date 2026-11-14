use super::*;

fn fixture() -> (tempfile::TempDir, ServerConfig, State) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg: ServerConfig = toml::from_str("").unwrap();
    cfg.persist_state = true;
    cfg.db_path = dir.path().join("state.db").to_string_lossy().into_owned();
    let state = State::new(&cfg).unwrap();
    (dir, cfg, state)
}

fn live(state: &State) -> Vec<u8> {
    bincode::serialize(&(
        state.all_channels(),
        state.settings(),
        state.all_users(),
        state.chat_history(),
    ))
    .unwrap()
}

fn stored(path: &str) -> Vec<u8> {
    let db = Db::open(path).unwrap();
    let channels: Vec<_> = db
        .load_channels()
        .unwrap()
        .into_iter()
        .map(|c| (c.id, c.name, c.kind, c.parent, c.position))
        .collect();
    bincode::serialize(&(
        channels,
        db.load_settings().unwrap(),
        db.recent_messages().unwrap(),
    ))
    .unwrap()
}

#[test]
fn failed_voice_or_text_deletion_preserves_state_and_retry_survives_reopen() {
    for kind in [ChannelKind::Voice, ChannelKind::Text] {
        for failure in [
            "DELETE ON channels",
            "DELETE ON messages",
            "UPDATE ON settings",
        ] {
            let (_dir, cfg, mut state) = fixture();
            let id = state
                .create_channel("Keep until commit", kind, None)
                .unwrap()
                .id;
            let mut settings = state.settings().clone();
            if kind == ChannelKind::Voice {
                settings.afk_channel = Some(id);
            } else {
                settings.announce_channel = Some(id);
            }
            state.update_settings(settings).unwrap();
            state.post_message(id, "synthetic", "retained", 1, None, None);
            let (tx, mut rx) = mpsc::unbounded_channel();
            let (client, _) = state.add_client("synthetic".into(), tx);
            if kind == ChannelKind::Voice {
                state.move_client(client, id).unwrap();
                assert!(state.set_sharing(client, true));
                state.stream_frame(client, VideoKind::Screen);
            }
            while rx.try_recv().is_ok() {}
            let before_live = live(&state);
            let before_stored = stored(&cfg.db_path);
            let conn = rusqlite::Connection::open(&cfg.db_path).unwrap();
            conn.execute_batch(&format!(
                "CREATE TRIGGER fail_delete BEFORE {failure} BEGIN SELECT RAISE(ABORT, 'synthetic'); END;"
            ))
            .unwrap();
            assert_eq!(
                state.delete_channel(id).unwrap_err(),
                "Could not save channel deletion"
            );
            assert_eq!(live(&state), before_live);
            assert_eq!(stored(&cfg.db_path), before_stored);
            assert!(rx.try_recv().is_err(), "no stream teardown before commit");
            conn.execute_batch("DROP TRIGGER fail_delete").unwrap();
            let moved = state.delete_channel(id).unwrap();
            assert_eq!(
                moved,
                if kind == ChannelKind::Voice {
                    vec![client]
                } else {
                    vec![]
                }
            );
            assert!(!state.client(client).unwrap().sharing);
            if kind == ChannelKind::Voice {
                let mut stopped = false;
                while let Ok(message) = rx.try_recv() {
                    stopped |= matches!(
                        message,
                        ServerMessage::SharingChanged { sharing: false, .. }
                    );
                }
                assert!(stopped);
            }
            assert_ne!(live(&state), before_live);
            assert_ne!(stored(&cfg.db_path), before_stored);
            drop(state);
            let reopened = State::new(&cfg).unwrap();
            assert!(reopened.channel_info(id).is_none());
            assert!(!reopened.chat_history().iter().any(|m| m.channel_id == id));
            assert_ne!(reopened.settings().afk_channel, Some(id));
            assert_ne!(reopened.settings().announce_channel, Some(id));
        }
    }
}

#[test]
fn failed_child_reparenting_rolls_back_deleted_category() {
    let (_dir, cfg, mut state) = fixture();
    let category = state
        .create_channel("Category", ChannelKind::Category, None)
        .unwrap()
        .id;
    let child = state
        .create_channel("Child", ChannelKind::Text, Some(category))
        .unwrap()
        .id;
    state.post_message(child, "synthetic", "retained child message", 1, None, None);
    let before_live = live(&state);
    let before_stored = stored(&cfg.db_path);
    let conn = rusqlite::Connection::open(&cfg.db_path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_reparent BEFORE UPDATE OF parent ON channels BEGIN SELECT RAISE(ABORT, 'synthetic'); END;").unwrap();
    assert!(state.delete_channel(category).is_err());
    assert_eq!(live(&state), before_live);
    assert_eq!(stored(&cfg.db_path), before_stored);
    conn.execute_batch("DROP TRIGGER fail_reparent").unwrap();
    state.delete_channel(category).unwrap();
    drop(state);
    let reopened = State::new(&cfg).unwrap();
    assert!(reopened.channel_info(category).is_none());
    assert_eq!(reopened.channel_info(child).unwrap().parent, None);
    assert!(reopened
        .chat_history()
        .iter()
        .any(|m| m.channel_id == child));
}
