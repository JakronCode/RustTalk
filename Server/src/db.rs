//! SQLite-Persistenz: Channels, Chat-Verlauf (Text-Channels) und Spawn-Log.
//!
//! Design:
//! - Eine Datei (WAL-Modus), Schreibvolumen ist winzig (Chat-Nachrichten).
//! - Retention: pro Text-Channel werden nur die letzten `CHANNEL_MSG_CAP`
//!   Nachrichten behalten -> die DB waechst gedeckelt, auch nach Jahren nicht.
//! - Beim Start werden Channels + Spawn-Log geladen; der Chat-Verlauf wird
//!   pro Verbindung (Welcome) abgefragt.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use rusttalk_shared::protocol::{ChannelKind, ChatRecord, LogEntry};

/// Aufbewahrung: letzte N Nachrichten pro Text-Channel.
const CHANNEL_MSG_CAP: i64 = 10_000;

/// Wie viele Nachrichten pro Channel beim Welcome mitgeschickt werden.
pub const HISTORY_PER_CHANNEL: i64 = 100;

/// Spawn-Log: beim Start die letzten N Eintraege laden.
const EVENT_HISTORY: i64 = 500;

#[derive(Debug, Clone)]
pub struct StoredChannel {
    pub id: u64,
    pub name: String,
    pub kind: ChannelKind,
    pub parent: Option<u64>,
}

pub struct Db {
    conn: Connection,
}

fn kind_str(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::Spawn => "spawn",
        ChannelKind::Category => "category",
        ChannelKind::Text => "text",
        ChannelKind::Voice => "voice",
    }
}

fn parse_kind(s: &str) -> ChannelKind {
    match s {
        "category" => ChannelKind::Category,
        "text" => ChannelKind::Text,
        "voice" => ChannelKind::Voice,
        _ => ChannelKind::Spawn,
    }
}

impl Db {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("SQLite oeffnen: {path}"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS channels (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                parent INTEGER
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_id INTEGER NOT NULL,
                from_name TEXT NOT NULL,
                text TEXT NOT NULL,
                timestamp INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_messages_channel ON messages(channel_id, id);
            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                text TEXT NOT NULL
            );",
        )?;
        // Migration: image-Spalte nachruesten, wenn die Tabelle aelter ist.
        let has_image = conn
            .prepare("PRAGMA table_info(messages)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .any(|c| c.map(|name| name == "image").unwrap_or(false));
        if !has_image {
            conn.execute("ALTER TABLE messages ADD COLUMN image BLOB", [])?;
        }
        Ok(Self { conn })
    }

    // --- Channels ---

    pub fn load_channels(&self) -> Result<Vec<StoredChannel>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, kind, parent FROM channels ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok(StoredChannel {
                id: r.get::<_, i64>(0)? as u64,
                name: r.get(1)?,
                kind: parse_kind(&r.get::<_, String>(2)?),
                parent: r.get::<_, Option<i64>>(3)?.map(|p| p as u64),
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn upsert_channel(&self, id: u64, name: &str, kind: ChannelKind, parent: Option<u64>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO channels (id, name, kind, parent) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET name = ?2, kind = ?3, parent = ?4",
            params![id as i64, name, kind_str(kind), parent.map(|p| p as i64)],
        )?;
        Ok(())
    }

    /// Loescht den Channel; Kinder werden Top-Level; Chat-Verlauf geht mit.
    pub fn delete_channel(&self, id: u64) -> Result<()> {
        self.conn.execute("DELETE FROM channels WHERE id = ?1", params![id as i64])?;
        self.conn.execute(
            "UPDATE channels SET parent = NULL WHERE parent = ?1",
            params![id as i64],
        )?;
        self.conn.execute("DELETE FROM messages WHERE channel_id = ?1", params![id as i64])?;
        Ok(())
    }

    // --- Spawn-Log (Events) ---

    pub fn log_event(&self, entry: &LogEntry) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events (timestamp, text) VALUES (?1, ?2)",
            params![entry.timestamp as i64, entry.text],
        )?;
        Ok(())
    }

    pub fn load_events(&self) -> Result<Vec<LogEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, text FROM (SELECT id, timestamp, text FROM events
             ORDER BY id DESC LIMIT ?1) ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![EVENT_HISTORY], |r| {
            Ok(LogEntry { timestamp: r.get::<_, i64>(0)? as u64, text: r.get(1)? })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // --- Chat-Nachrichten ---

    pub fn add_message(
        &self,
        channel_id: u64,
        from_name: &str,
        text: &str,
        timestamp: u64,
        image: Option<&[u8]>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO messages (channel_id, from_name, text, timestamp, image)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![channel_id as i64, from_name, text, timestamp as i64, image],
        )?;
        // Retention: nur die letzten CHANNEL_MSG_CAP je Channel behalten.
        self.conn.execute(
            "DELETE FROM messages WHERE channel_id = ?1 AND id NOT IN (
                SELECT id FROM messages WHERE channel_id = ?1 ORDER BY id DESC LIMIT ?2
            )",
            params![channel_id as i64, CHANNEL_MSG_CAP],
        )?;
        Ok(())
    }

    /// Letzte `HISTORY_PER_CHANNEL` Nachrichten pro Channel (aelteste zuerst),
    /// sortiert nach Channel. Bewusst ohne Window-Functions (SQLite < 3.25
    /// kann die nicht; so geht es ueberall).
    pub fn recent_messages(&self) -> Result<Vec<ChatRecord>> {
        let mut ids_stmt =
            self.conn.prepare("SELECT DISTINCT channel_id FROM messages ORDER BY channel_id")?;
        let channel_ids: Vec<i64> = ids_stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;

        let mut stmt = self.conn.prepare(
            "SELECT channel_id, from_name, text, timestamp, image FROM messages
             WHERE channel_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let mut out = Vec::new();
        for cid in channel_ids {
            let rows = stmt.query_map(params![cid, HISTORY_PER_CHANNEL], |r| {
                Ok(ChatRecord {
                    channel_id: r.get::<_, i64>(0)? as u64,
                    from_name: r.get(1)?,
                    text: r.get(2)?,
                    timestamp: r.get::<_, i64>(3)? as u64,
                    image: r.get::<_, Option<Vec<u8>>>(4)?,
                })
            })?;
            let mut per_channel: Vec<ChatRecord> = rows.collect::<Result<_, _>>()?;
            per_channel.reverse(); // DESC -> ASC
            out.extend(per_channel);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> String {
        let path = std::env::temp_dir().join(format!("rusttalk-db-test-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path.to_string_lossy().to_string()
    }

    #[test]
    fn channels_roundtrip_and_delete() {
        let path = temp_db("channels");
        let db = Db::open(&path).unwrap();
        db.upsert_channel(1, "Spawn", ChannelKind::Spawn, None).unwrap();
        db.upsert_channel(2, "Talk", ChannelKind::Category, None).unwrap();
        db.upsert_channel(3, "Funkturm", ChannelKind::Voice, Some(2)).unwrap();

        let channels = db.load_channels().unwrap();
        assert_eq!(channels.len(), 3);
        assert_eq!(channels[2].name, "Funkturm");
        assert_eq!(channels[2].parent, Some(2));

        // Umbenennen via upsert:
        db.upsert_channel(3, "Funkturm 116", ChannelKind::Voice, Some(2)).unwrap();
        let channels = db.load_channels().unwrap();
        assert_eq!(channels[2].name, "Funkturm 116");

        // Kategorie loeschen -> Kind wird Top-Level:
        db.delete_channel(2).unwrap();
        let channels = db.load_channels().unwrap();
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[1].parent, None);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn events_roundtrip() {
        let path = temp_db("events");
        let db = Db::open(&path).unwrap();
        db.log_event(&LogEntry { timestamp: 100, text: "alice ist erschienen".into() }).unwrap();
        db.log_event(&LogEntry { timestamp: 200, text: "bob ist erschienen".into() }).unwrap();
        let events = db.load_events().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].text, "alice ist erschienen");
        assert_eq!(events[1].timestamp, 200);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn messages_roundtrip_and_order() {
        let path = temp_db("messages");
        let db = Db::open(&path).unwrap();
        db.add_message(1, "alice", "erste", 1000, None).unwrap();
        db.add_message(1, "bob", "zweite", 1001, None).unwrap();
        db.add_message(2, "alice", "anderer channel", 1002, None).unwrap();
        let msgs = db.recent_messages().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].channel_id, 1);
        assert_eq!(msgs[0].text, "erste");
        assert_eq!(msgs[1].text, "zweite");
        assert!(msgs.iter().all(|m| m.image.is_none()));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn messages_with_image_roundtrip() {
        let path = temp_db("images");
        let db = Db::open(&path).unwrap();
        let png = vec![0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        db.add_message(1, "alice", "schau", 1000, Some(&png)).unwrap();
        let msgs = db.recent_messages().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].image.as_deref(), Some(&png[..]));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn migration_adds_image_column() {
        // Alte Tabelle ohne image-Spalte simulieren, dann Db::open migrieren.
        let path = temp_db("migration");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    channel_id INTEGER NOT NULL,
                    from_name TEXT NOT NULL,
                    text TEXT NOT NULL,
                    timestamp INTEGER NOT NULL
                );
                INSERT INTO messages (channel_id, from_name, text, timestamp) VALUES (1, 'alt', 'msg', 1);",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap(); // Migration laeuft hier
        let msgs = db.recent_messages().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "msg");
        assert!(msgs[0].image.is_none());
        db.add_message(1, "neu", "mit bild", 2, Some(&[1, 2, 3])).unwrap();
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn history_limit_per_channel() {
        let path = temp_db("limit");
        let db = Db::open(&path).unwrap();
        for i in 0..(HISTORY_PER_CHANNEL + 25) {
            db.add_message(1, "a", &format!("msg {i}"), i as u64, None).unwrap();
        }
        let msgs = db.recent_messages().unwrap();
        assert_eq!(msgs.len() as i64, HISTORY_PER_CHANNEL);
        // Die neuesten ueberleben:
        assert!(msgs.iter().any(|m| m.text == format!("msg {}", HISTORY_PER_CHANNEL + 24)));
        assert!(!msgs.iter().any(|m| m.text == "msg 0"));
        std::fs::remove_file(&path).ok();
    }
}
