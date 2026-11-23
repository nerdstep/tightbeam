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
             CREATE INDEX IF NOT EXISTS messages_by_time ON messages (ts, author, seq);
             -- The highest sequence number erased per author by `clear`. Numbering and
             -- sync carry on from here, as if the messages were still stored.
             CREATE TABLE IF NOT EXISTS cleared (
                 author TEXT    PRIMARY KEY,
                 up_to  INTEGER NOT NULL
             );",
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

    /// Highest sequence number held for `author`, counting erased ones, or 0 if none.
    pub fn max_seq(&self, author: &str) -> Result<u64> {
        let n: i64 = self.0.query_row(
            "SELECT MAX(
                 COALESCE((SELECT MAX(seq) FROM messages WHERE author = ?1), 0),
                 COALESCE((SELECT up_to FROM cleared WHERE author = ?1), 0))",
            [author],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    }

    /// Erases the history, except `keep_author`'s messages after `keep_after` (mine that
    /// haven't been delivered yet). The space is zeroed rather than just freed. Returns
    /// how many messages were erased.
    pub fn clear(&self, keep_author: &str, keep_after: Option<u64>) -> Result<usize> {
        let keep_after = keep_after.map(|n| n as i64);
        let erase = "?2 IS NULL OR NOT (author = ?1 AND seq > ?2)";
        self.0.execute_batch("PRAGMA secure_delete = ON; BEGIN")?;
        let result = (|| -> Result<usize> {
            self.0.execute(
                &format!(
                    "INSERT INTO cleared (author, up_to)
                     SELECT author, MAX(seq) FROM messages WHERE {erase} GROUP BY author
                     ON CONFLICT(author) DO UPDATE SET up_to = MAX(up_to, excluded.up_to)"
                ),
                params![keep_author, keep_after],
            )?;
            Ok(self.0.execute(
                &format!("DELETE FROM messages WHERE {erase}"),
                params![keep_author, keep_after],
            )?)
        })();
        self.0.execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
        // Old copies of the pages also sit in the write-ahead log until a checkpoint.
        self.0.execute_batch("PRAGMA secure_delete = OFF; PRAGMA wal_checkpoint(TRUNCATE);")?;
        result
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

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(author: &str, seq: u64) -> Message {
        Message { author: author.into(), seq, ts: seq as i64, text: format!("{author}{seq}") }
    }

    #[test]
    fn clear_keeps_numbering_and_undelivered_messages() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        db.insert_many(&[msg("me", 1), msg("me", 2), msg("me", 3), msg("peer", 1), msg("peer", 2)])
            .unwrap();

        // The peer has my first two; the third is still waiting.
        assert_eq!(db.clear("me", Some(2)).unwrap(), 4);
        let left: Vec<_> = db.all().unwrap().into_iter().map(|m| m.text).collect();
        assert_eq!(left, ["me3"]);
        assert_eq!(db.max_seq("me").unwrap(), 3);
        assert_eq!(db.max_seq("peer").unwrap(), 2, "sync must not ask for erased messages again");

        // Clearing again never lowers what was recorded.
        assert_eq!(db.clear("me", None).unwrap(), 1);
        assert!(db.all().unwrap().is_empty());
        assert_eq!(db.max_seq("me").unwrap(), 3);
        assert_eq!(db.max_seq("peer").unwrap(), 2);

        // New messages carry on numbering, and paging still works.
        db.insert(&msg("peer", 3)).unwrap();
        assert_eq!(db.max_seq("peer").unwrap(), 3);
        assert_eq!(db.page(None, 10).unwrap().messages, [msg("peer", 3)]);
    }
}
