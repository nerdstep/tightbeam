//! Local storage: settings (keys, names, pairing) and the message history.

use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Message {
    /// Endpoint id (public key) of whoever wrote it.
    pub author: String,
    /// Per-author sequence number, starting at 1 with no gaps.
    pub seq: u64,
    /// Milliseconds since the Unix epoch, from the author's clock.
    pub ts: i64,
    pub text: String,
}

/// A position in display order (`ts`, then `author`, then `seq`): page before this message.
#[derive(Debug, Clone, Deserialize)]
pub struct Cursor {
    pub ts: i64,
    pub author: String,
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Page {
    /// Oldest first.
    pub messages: Vec<Message>,
    /// Whether there are messages older than the first one here.
    pub has_more: bool,
}

pub struct Db(Connection);

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS settings (
                 key   TEXT PRIMARY KEY,
                 value BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS messages (
                 author TEXT    NOT NULL,
                 seq    INTEGER NOT NULL,
                 ts     INTEGER NOT NULL,
                 text   TEXT    NOT NULL,
                 PRIMARY KEY (author, seq)
             );
             -- Display order, for paging through history.
             CREATE INDEX IF NOT EXISTS messages_by_time ON messages (ts, author, seq);",
        )?;
        Ok(Self(conn))
    }

    /// Up to `limit` messages before `before` in display order (or the newest, if `None`),
    /// returned oldest first, plus whether anything older remains.
    pub fn page(&self, before: Option<&Cursor>, limit: usize) -> Result<Page> {
        let fetch = limit as i64 + 1; // one extra row tells us whether there's more
        let mut messages = match before {
            Some(c) => self.query(
                "SELECT author, seq, ts, text FROM messages
                 WHERE (ts, author, seq) < (?1, ?2, ?3)
                 ORDER BY ts DESC, author DESC, seq DESC LIMIT ?4",
                params![c.ts, c.author, c.seq as i64, fetch],
            )?,
            None => self.query(
                "SELECT author, seq, ts, text FROM messages
                 ORDER BY ts DESC, author DESC, seq DESC LIMIT ?1",
                params![fetch],
            )?,
        };
        let has_more = messages.len() > limit;
        messages.truncate(limit);
        messages.reverse();
        Ok(Page { messages, has_more })
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .0
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn get_str(&self, key: &str) -> Result<Option<String>> {
        Ok(self.get(key)?.and_then(|v| String::from_utf8(v).ok()))
    }

    pub fn set(&self, key: &str, value: impl AsRef<[u8]>) -> Result<()> {
        self.0.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value.as_ref()],
        )?;
        Ok(())
    }

    pub fn delete(&self, key: &str) -> Result<()> {
        self.0.execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }

    /// Highest sequence number stored for `author`, or 0 if none.
    pub fn max_seq(&self, author: &str) -> Result<u64> {
        let n: i64 = self.0.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE author = ?1",
            [author],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    }

    /// Stores a message. Returns false if it was already stored.
    pub fn insert(&self, m: &Message) -> Result<bool> {
        let changed = self.0.execute(
            "INSERT OR IGNORE INTO messages (author, seq, ts, text) VALUES (?1, ?2, ?3, ?4)",
            params![m.author, m.seq as i64, m.ts, m.text],
        )?;
        Ok(changed > 0)
    }

    /// Stores many messages in one transaction (for tests and seeding).
    pub fn insert_many(&self, messages: &[Message]) -> Result<()> {
        self.0.execute_batch("BEGIN")?;
        let result = messages.iter().try_for_each(|m| self.insert(m).map(drop));
        self.0.execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
        result
    }

    /// Messages by `author` with a sequence number above `after`, oldest first.
    pub fn since(&self, author: &str, after: u64) -> Result<Vec<Message>> {
        self.query(
            "SELECT author, seq, ts, text FROM messages
             WHERE author = ?1 AND seq > ?2 ORDER BY seq",
            params![author, after as i64],
        )
    }

    /// Every message, in display order.
    pub fn all(&self) -> Result<Vec<Message>> {
        self.query("SELECT author, seq, ts, text FROM messages ORDER BY ts, author, seq", [])
    }

    fn query(&self, sql: &str, p: impl rusqlite::Params) -> Result<Vec<Message>> {
        let mut stmt = self.0.prepare(sql)?;
        let rows = stmt.query_map(p, |r| {
            Ok(Message {
                author: r.get(0)?,
                seq: r.get::<_, i64>(1)? as u64,
                ts: r.get(2)?,
                text: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
