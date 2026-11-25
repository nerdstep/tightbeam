//! Local storage: settings (keys, names, pairing) and the log.
//!
//! The log holds every author's numbered entries: messages, and reactions, edits and
//! deletes that refer to an earlier message by `(author, seq)`. A reply also refers to
//! the message it answers. Entries are only ever added, with one exception: a delete
//! blanks the text of the message it deletes, and of that message's edits.
//!
//! Display order comes from each entry's `hlc`, a hybrid logical clock: the author's wall
//! clock, pushed past every entry they had stored when they wrote it. So whatever someone
//! writes after seeing a message sorts after it, however far apart the two clocks are.
//!
//! The `search` table indexes each message's current text (SQLite FTS5). Storing an
//! entry keeps it up to date: an edit replaces the text, and a delete or `clear` removes
//! it. Deleted text is zeroed on disk (`secure_delete`), in the index too.
//!
//! A vanishing message carries a lifetime (`vanish`, seconds). Each side starts the
//! countdown (`expires`, its own clock) once the recipient has seen it, then erases its
//! copy like a delete. `expires` is local and never synced.
//!
//! A message can carry a file (`FileInfo`: hash, name, size, type). The bytes live in
//! the data folder, not here; the `files` table records which have arrived ('done') and
//! which the sender no longer has ('gone'). No row means still on its way. A delete
//! blanks the message's file details along with its text.

use std::path::Path;

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::entry::{DELETE, EDIT, Entry, FileInfo, Kind, MESSAGE, REACTION, Ref};
use crate::{clock::Clock, files::may_open};

/// Where a message's file has got to on this computer.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileState {
    /// Still to come from the sender.
    Pending,
    /// Here, complete and checked.
    Done,
    /// The sender no longer has it.
    Gone,
}

/// A message's file, as the transcript shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Attachment {
    #[serde(flatten)]
    pub info: FileInfo,
    pub state: FileState,
    /// Whether it may open in its default app (`files::may_open`); if not, it can only be
    /// shown in its folder.
    pub openable: bool,
}

/// A file that's here: its name in the data folder, its type, and its size in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub name: String,
    pub mime: String,
    pub size: u64,
}

/// A message as the transcript shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Message {
    pub author: String,
    pub seq: u64,
    pub ts: i64,
    pub hlc: i64,
    /// The latest edit's text, if edited. Empty once deleted. With a file, the caption.
    pub text: String,
    pub edited: bool,
    /// Deleted by its author, or vanished. Shown as a placeholder, without reply,
    /// reactions or file.
    pub deleted: bool,
    pub file: Option<Attachment>,
    /// A vanishing message: seconds it lasts once seen.
    pub vanish: Option<u64>,
    /// When it vanishes (ms, this computer's clock), once the countdown has started.
    pub expires: Option<i64>,
    /// Its time ran out. `deleted` is true too.
    pub vanished: bool,
    /// The message this one answers.
    pub reply_to: Option<Quote>,
    /// At most one per person, in author order.
    pub reactions: Vec<Reaction>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Quote {
    pub author: String,
    pub seq: u64,
    /// Its place in display order, to load the history back to it. `None` when it isn't
    /// stored here.
    pub hlc: Option<i64>,
    /// Its latest text. `None` when it isn't stored here (cleared, for example) or was
    /// deleted.
    pub text: Option<String>,
    /// Deleted, or vanished.
    pub deleted: bool,
    pub vanished: bool,
    /// Its file, unless deleted.
    pub file: Option<FileInfo>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Reaction {
    pub author: String,
    pub emoji: String,
}

/// A position in display order (`hlc`, then `author`, then `seq`): page before this message.
#[derive(Debug, Clone, Deserialize)]
pub struct Cursor {
    pub hlc: i64,
    pub author: String,
    pub seq: u64,
}

/// A message that matches a search.
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub message: Message,
    /// The matching part of its text, with each match between \u{2} and \u{3}.
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Page {
    /// Oldest first.
    pub messages: Vec<Message>,
    /// Whether there are messages older than the first one here.
    pub has_more: bool,
}

/// Values of `files.state`.
const FILE_DONE: &str = "done";
const FILE_GONE: &str = "gone";

/// Settings key: the highest `hlc` erased by `clear`, so the clock never runs backwards.
const CLEARED_CLOCK: &str = "cleared_clock";

/// What `Db::current` finds.
struct Current {
    text: String,
    edited: bool,
    deleted: bool,
}

pub struct Db(Connection, Clock);

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with(path, Clock::system())
    }

    /// Opens the log with `clock` for the time vanishing messages are judged by.
    pub(crate) fn open_with(path: &Path, clock: Clock) -> Result<Self> {
        let conn = Connection::open(path)?;
        // Freed space is zeroed, so deleted and cleared text doesn't linger in the file.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA secure_delete = ON;
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
             -- The highest sequence number erased per author by `clear`. Numbering and
             -- sync carry on from here, as if the messages were still stored.
             CREATE TABLE IF NOT EXISTS cleared (
                 author TEXT    PRIMARY KEY,
                 up_to  INTEGER NOT NULL
             );",
        )?;
        // Schema changes, in order. `user_version` counts how many have been applied.
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            // 0.4: replies and reactions. `ref_*` is the message a reply answers or a
            // reaction is on.
            conn.execute_batch(
                "BEGIN;
                 ALTER TABLE messages ADD COLUMN kind TEXT NOT NULL DEFAULT 'message';
                 ALTER TABLE messages ADD COLUMN ref_author TEXT;
                 ALTER TABLE messages ADD COLUMN ref_seq INTEGER;
                 CREATE INDEX messages_by_ref ON messages (ref_author, ref_seq);
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }
        if version < 2 {
            // 0.5: display order by hybrid logical clock. Existing entries keep the order
            // they had, by `ts`.
            conn.execute_batch(
                "BEGIN;
                 ALTER TABLE messages ADD COLUMN hlc INTEGER NOT NULL DEFAULT 0;
                 UPDATE messages SET hlc = ts;
                 DROP INDEX IF EXISTS messages_by_time;
                 CREATE INDEX messages_by_order ON messages (hlc, author, seq);
                 PRAGMA user_version = 2;
                 COMMIT;",
            )?;
        }
        let db = Self(conn, clock);
        if version < 5 {
            // One transaction, so the search index is filled using the finished schema.
            db.0.execute_batch("BEGIN")?;
            let result = (|| -> Result<()> {
                if version < 3 {
                    // 0.5: search.
                    db.0.execute_batch(
                        "CREATE VIRTUAL TABLE search USING fts5(
                             text, author UNINDEXED, seq UNINDEXED,
                             tokenize = 'unicode61 remove_diacritics 2'
                         );
                         -- Deleting from the index removes the words, not just marks them.
                         INSERT INTO search (search, rank) VALUES ('secure-delete', 1);",
                    )?;
                }
                if version < 4 {
                    // 0.6: vanishing messages. `vanish` syncs; `expires` is this computer's.
                    db.0.execute_batch(
                        "ALTER TABLE messages ADD COLUMN vanish INTEGER;
                         ALTER TABLE messages ADD COLUMN expires INTEGER;
                         CREATE INDEX messages_by_expiry ON messages (expires)
                             WHERE expires IS NOT NULL;",
                    )?;
                }
                // 0.7: files. `file` is the `FileInfo` as JSON; `file_hash` repeats its hash
                // for lookups.
                db.0.execute_batch(
                    "ALTER TABLE messages ADD COLUMN file TEXT;
                     ALTER TABLE messages ADD COLUMN file_hash TEXT;
                     CREATE INDEX messages_by_file ON messages (file_hash)
                         WHERE file_hash IS NOT NULL;
                     CREATE TABLE files (
                         hash  TEXT PRIMARY KEY,
                         -- Its name in the data folder.
                         name  TEXT NOT NULL,
                         state TEXT NOT NULL
                     );",
                )?;
                if version < 3 {
                    // Fill the search index from the history.
                    for e in db.entries("WHERE kind = 'message'", [])? {
                        db.index(&Ref { author: e.author, seq: e.seq })?;
                    }
                }
                db.0.execute_batch("PRAGMA user_version = 5")?;
                Ok(())
            })();
            db.0.execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
            result?;
        }
        if version < 6 {
            // 0.8: everyone this computer has been paired with, by their last known name,
            // so their messages keep it after pairing with someone new.
            db.0.execute_batch(
                "BEGIN;
                 CREATE TABLE people (id TEXT PRIMARY KEY, name TEXT NOT NULL);
                 INSERT INTO people (id, name)
                     SELECT CAST(id.value AS TEXT), COALESCE(CAST(name.value AS TEXT), '')
                     FROM settings id LEFT JOIN settings name ON name.key = 'peer_name'
                     WHERE id.key = 'peer_id';
                 PRAGMA user_version = 6;
                 COMMIT;",
            )?;
        }
        Ok(db)
    }

    /// Everyone this computer has been paired with, as (id, last known name).
    pub fn people(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.0.prepare_cached("SELECT id, name FROM people ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Remembers `id`'s name, replacing an older one.
    pub fn set_person(&self, id: &str, name: &str) -> Result<()> {
        self.0
            .prepare_cached(
                "INSERT INTO people (id, name) VALUES (?1, ?2)
                 ON CONFLICT (id) DO UPDATE SET name = excluded.name",
            )?
            .execute(params![id, name])?;
        Ok(())
    }

    /// Starts the countdown on `author`'s vanishing messages up to `up_to` (they've been
    /// seen): each vanishes its lifetime after `now`. Returns the ones started.
    pub fn start_vanish(&self, author: &str, up_to: u64, now: i64) -> Result<Vec<Ref>> {
        let mut stmt = self.0.prepare_cached(
            "UPDATE messages SET expires = ?3 + vanish * 1000
             WHERE author = ?1 AND seq <= ?2 AND kind = 'message'
               AND vanish IS NOT NULL AND expires IS NULL
             RETURNING seq",
        )?;
        let seqs = stmt.query_map(params![author, up_to as i64, now], |r| r.get::<_, i64>(0))?;
        let refs = seqs.map(|seq| Ok(Ref { author: author.to_string(), seq: seq? as u64 }));
        refs.collect()
    }

    /// Erases the vanishing messages whose time is up, like a delete. Returns them.
    pub fn expire(&self, now: i64) -> Result<Vec<Ref>> {
        let due = self
            .0
            .prepare_cached(
                "SELECT author, seq FROM messages
                 WHERE expires <= ?1 AND kind = 'message' AND text != ''",
            )?
            .query_map([now], |r| Ok(Ref { author: r.get(0)?, seq: r.get::<_, i64>(1)? as u64 }))?
            .collect::<Result<Vec<_>, _>>()?;
        for at in &due {
            self.scrub(at)?;
        }
        Ok(due)
    }

    /// Whether a vanishing message's time is up.
    fn vanished(&self, at: &Ref) -> Result<bool> {
        Ok(self.expires(at)?.is_some_and(|t| t <= self.1.now_ms()))
    }

    /// When a message vanishes, if its countdown has started.
    fn expires(&self, at: &Ref) -> Result<Option<i64>> {
        Ok(self
            .0
            .prepare_cached(
                "SELECT expires FROM messages WHERE author = ?1 AND seq = ?2 AND kind = 'message'",
            )?
            .query_row(params![at.author, at.seq as i64], |r| r.get(0))
            .optional()?
            .flatten())
    }

    /// Where a file has got to here.
    pub fn file_state(&self, hash: &str) -> Result<FileState> {
        let state: Option<String> = self
            .0
            .prepare_cached("SELECT state FROM files WHERE hash = ?1")?
            .query_row([hash], |r| r.get(0))
            .optional()?;
        Ok(match state.as_deref() {
            Some(FILE_DONE) => FileState::Done,
            Some(FILE_GONE) => FileState::Gone,
            _ => FileState::Pending,
        })
    }

    /// Records a file as here (`name` is where it's stored) or gone from the sender.
    pub fn set_file(&self, hash: &str, name: &str, state: FileState) -> Result<()> {
        let state = match state {
            FileState::Done => FILE_DONE,
            FileState::Gone => FILE_GONE,
            FileState::Pending => {
                self.forget_file(hash)?;
                return Ok(());
            }
        };
        self.0
            .prepare_cached(
                "INSERT INTO files (hash, name, state) VALUES (?1, ?2, ?3)
                 ON CONFLICT(hash) DO UPDATE SET name = excluded.name, state = excluded.state",
            )?
            .execute(params![hash, name, state])?;
        Ok(())
    }

    pub fn forget_file(&self, hash: &str) -> Result<()> {
        self.0.prepare_cached("DELETE FROM files WHERE hash = ?1")?.execute([hash])?;
        Ok(())
    }

    /// A file that's here and still attached to a message, so it can be shown or sent.
    pub fn stored_file(&self, hash: &str) -> Result<Option<StoredFile>> {
        Ok(self
            .0
            .prepare_cached(
                "SELECT files.name, json_extract(messages.file, '$.mime'),
                        json_extract(messages.file, '$.size')
                 FROM files JOIN messages ON messages.file_hash = files.hash
                 WHERE files.hash = ?1 AND files.state = 'done' LIMIT 1",
            )?
            .query_row([hash], |r| {
                let size: i64 = r.get(2)?;
                Ok(StoredFile { name: r.get(0)?, mime: r.get(1)?, size: size as u64 })
            })
            .optional()?)
    }

    /// Whether a message still has this file.
    pub fn file_in_use(&self, hash: &str) -> Result<bool> {
        Ok(self
            .0
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM messages WHERE file_hash = ?1)")?
            .query_row([hash], |r| r.get(0))?)
    }

    /// Files still to come, newest message first, each once.
    pub fn wanted_files(&self) -> Result<Vec<FileInfo>> {
        let mut stmt = self.0.prepare_cached(
            "SELECT file FROM messages
             WHERE file_hash IS NOT NULL AND file_hash NOT IN (SELECT hash FROM files)
             ORDER BY hlc DESC",
        )?;
        let mut wanted: Vec<FileInfo> = Vec::new();
        for json in stmt.query_map([], |r| r.get::<_, String>(0))? {
            if let Ok(file) = serde_json::from_str::<FileInfo>(&json?)
                && !wanted.iter().any(|w| w.hash == file.hash)
            {
                wanted.push(file);
            }
        }
        Ok(wanted)
    }

    /// Recorded files that no message has any more (deleted, vanished or cleared).
    pub fn unused_files(&self) -> Result<Vec<String>> {
        let mut stmt = self.0.prepare_cached(
            "SELECT hash FROM files WHERE hash NOT IN
                 (SELECT file_hash FROM messages WHERE file_hash IS NOT NULL)",
        )?;
        let hashes = stmt.query_map([], |r| r.get(0))?;
        Ok(hashes.collect::<Result<_, _>>()?)
    }

    /// The messages that have this file.
    pub fn with_file(&self, hash: &str) -> Result<Vec<Ref>> {
        let mut stmt = self.0.prepare_cached(
            "SELECT author, seq FROM messages WHERE file_hash = ?1 AND kind = 'message'",
        )?;
        let refs = stmt.query_map([hash], |r| {
            Ok(Ref { author: r.get(0)?, seq: r.get::<_, i64>(1)? as u64 })
        })?;
        Ok(refs.collect::<Result<_, _>>()?)
    }

    /// Messages matching `query`, newest first. Each word matches words starting with
    /// it, ignoring case and accents.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>> {
        // Each word becomes a quoted prefix ("word"*), so FTS5 syntax in it is just text.
        let terms: Vec<String> = query
            .split_whitespace()
            .filter(|t| t.chars().any(char::is_alphanumeric))
            .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
            .collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let found = self
            .0
            .prepare_cached(
                "SELECT search.author, search.seq,
                        snippet(search, 0, char(2), char(3), '…', 16)
                 FROM search JOIN messages
                   ON messages.author = search.author AND messages.seq = search.seq
                  AND messages.kind = 'message'
                 WHERE search MATCH ?1
                 ORDER BY messages.hlc DESC, messages.author DESC, messages.seq DESC
                 LIMIT ?2",
            )?
            .query_map(params![terms.join(" "), limit as i64], |r| {
                Ok((Ref { author: r.get(0)?, seq: r.get::<_, i64>(1)? as u64 }, r.get(2)?))
            })?
            .collect::<Result<Vec<(Ref, String)>, _>>()?;
        let mut hits = Vec::with_capacity(found.len());
        for (at, snippet) in found {
            if let Some(message) = self.message(&at)? {
                hits.push(Hit { message, snippet });
            }
        }
        Ok(hits)
    }

    /// Messages from `from` up to (not including) `before`, in display order. Fills the
    /// gap between the oldest loaded message and one further back, like a search hit.
    pub fn range(&self, from: &Cursor, before: &Cursor) -> Result<Vec<Message>> {
        let entries = self.entries(
            "WHERE kind = 'message'
               AND (hlc, author, seq) >= (?1, ?2, ?3) AND (hlc, author, seq) < (?4, ?5, ?6)
             ORDER BY hlc, author, seq",
            params![
                from.hlc,
                from.author,
                from.seq as i64,
                before.hlc,
                before.author,
                before.seq as i64
            ],
        )?;
        entries.into_iter().map(|e| self.show(e)).collect()
    }

    /// Up to `limit` messages before `before` in display order (or the newest, if `None`),
    /// returned oldest first, plus whether anything older remains.
    pub fn page(&self, before: Option<&Cursor>, limit: usize) -> Result<Page> {
        let fetch = limit as i64 + 1; // one extra row tells us whether there's more
        let entries = match before {
            Some(c) => self.entries(
                "WHERE kind = 'message' AND (hlc, author, seq) < (?1, ?2, ?3)
                 ORDER BY hlc DESC, author DESC, seq DESC LIMIT ?4",
                params![c.hlc, c.author, c.seq as i64, fetch],
            )?,
            None => self.entries(
                "WHERE kind = 'message' ORDER BY hlc DESC, author DESC, seq DESC LIMIT ?1",
                params![fetch],
            )?,
        };
        let has_more = entries.len() > limit;
        let mut messages =
            entries.into_iter().take(limit).map(|e| self.show(e)).collect::<Result<Vec<_>>>()?;
        messages.reverse();
        Ok(Page { messages, has_more })
    }

    /// Every message, in display order.
    pub fn all(&self) -> Result<Vec<Message>> {
        let entries = self.entries("WHERE kind = 'message' ORDER BY hlc, author, seq", [])?;
        entries.into_iter().map(|e| self.show(e)).collect()
    }

    /// A message as the transcript shows it, if it's stored here.
    pub fn message(&self, at: &Ref) -> Result<Option<Message>> {
        let found = self.entries(
            "WHERE kind = 'message' AND author = ?1 AND seq = ?2",
            params![at.author, at.seq as i64],
        )?;
        found.into_iter().next().map(|e| self.show(e)).transpose()
    }

    /// The reactions on a message now: each person's latest, unless they took it back.
    pub fn reactions(&self, to: &Ref) -> Result<Vec<Reaction>> {
        let mut stmt = self.0.prepare_cached(
            "SELECT author, text FROM messages r
             WHERE kind = 'reaction' AND ref_author = ?1 AND ref_seq = ?2
               AND seq = (SELECT MAX(seq) FROM messages
                          WHERE kind = 'reaction' AND author = r.author
                            AND ref_author = ?1 AND ref_seq = ?2)
               AND text != ''
             ORDER BY author",
        )?;
        let rows = stmt.query_map(params![to.author, to.seq as i64], |r| {
            Ok(Reaction { author: r.get(0)?, emoji: r.get(1)? })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
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

    /// The `hlc` for an entry I write at `now`: later than every entry stored here, and
    /// every one erased.
    pub fn next_hlc(&self, now: i64) -> Result<i64> {
        Ok(now.max(self.clock()?.saturating_add(1)))
    }

    /// The highest `hlc` stored here or erased.
    fn clock(&self) -> Result<i64> {
        let stored: i64 =
            self.0.query_row("SELECT COALESCE(MAX(hlc), 0) FROM messages", [], |r| r.get(0))?;
        let erased = self.get_str(CLEARED_CLOCK)?.and_then(|v| v.parse().ok()).unwrap_or(0);
        Ok(stored.max(erased))
    }

    /// Erases the history, except `keep_author`'s entries after `keep_after` (mine that
    /// haven't been delivered yet). The space is zeroed rather than just freed. Returns
    /// how many entries were erased.
    pub fn clear(&self, keep_author: &str, keep_after: Option<u64>) -> Result<usize> {
        let keep_after = keep_after.map(|n| n as i64);
        let erase = "?2 IS NULL OR NOT (author = ?1 AND seq > ?2)";
        self.0.execute_batch("BEGIN")?;
        let result = (|| -> Result<usize> {
            self.0.execute(
                &format!("DELETE FROM search WHERE {erase}"),
                params![keep_author, keep_after],
            )?;
            self.set(CLEARED_CLOCK, self.clock()?.to_string())?;
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
        self.0.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        result
    }

    /// Stores an entry. Returns false if it was already stored.
    pub fn insert(&self, e: &Entry) -> Result<bool> {
        let (to, vanish, file) = match &e.kind {
            Kind::Message { reply_to, vanish, file } => (reply_to.as_ref(), *vanish, file.as_ref()),
            Kind::Reaction { to } | Kind::Edit { of: to } | Kind::Delete { of: to } => {
                (Some(to), None, None)
            }
            Kind::Inert { to, .. } => (to.as_ref(), None, None),
        };
        let changed = self.0.execute(
            "INSERT OR IGNORE INTO messages
                 (author, seq, ts, hlc, text, kind, ref_author, ref_seq, vanish, file, file_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                e.author,
                e.seq as i64,
                e.ts,
                e.hlc,
                e.text,
                e.kind.tag(),
                to.map(|r| &r.author),
                to.map(|r| r.seq as i64),
                vanish.map(|v| v as i64),
                file.map(serde_json::to_string).transpose()?,
                file.map(|f| &f.hash),
            ],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        match &e.kind {
            Kind::Message { .. } => self.index(&Ref { author: e.author.clone(), seq: e.seq })?,
            // Only the author's own edits and deletes get here; anyone else's are inert.
            Kind::Edit { of } | Kind::Delete { of } => {
                self.0.execute(
                    "DELETE FROM search WHERE author = ?1 AND seq = ?2",
                    params![of.author, of.seq as i64],
                )?;
                self.index(of)?;
            }
            _ => {}
        }
        Ok(true)
    }

    /// Adds a message's current text, and its file's name, to the search index, unless
    /// it was deleted.
    fn index(&self, at: &Ref) -> Result<()> {
        let Some((original, _)) = self.original(at)? else { return Ok(()) };
        let current = self.current(at, original)?;
        if !current.deleted && !self.vanished(at)? {
            let text = match self.file_of(at)? {
                Some(file) if current.text.is_empty() => file.name,
                Some(file) => format!("{}\n{}", current.text, file.name),
                None => current.text,
            };
            self.0
                .prepare_cached("INSERT INTO search (text, author, seq) VALUES (?1, ?2, ?3)")?
                .execute(params![text, at.author, at.seq as i64])?;
        }
        Ok(())
    }

    /// A message's file, if it has one and is stored here.
    fn file_of(&self, at: &Ref) -> Result<Option<FileInfo>> {
        let json: Option<Option<String>> = self
            .0
            .prepare_cached(
                "SELECT file FROM messages WHERE author = ?1 AND seq = ?2 AND kind = 'message'",
            )?
            .query_row(params![at.author, at.seq as i64], |r| r.get(0))
            .optional()?;
        Ok(json.flatten().and_then(|j| serde_json::from_str(&j).ok()))
    }

    /// A message's text as first sent, and its place in display order, if it's stored here.
    fn original(&self, at: &Ref) -> Result<Option<(String, i64)>> {
        Ok(self
            .0
            .prepare_cached(
                "SELECT text, hlc FROM messages WHERE author = ?1 AND seq = ?2 AND kind = 'message'",
            )?
            .query_row(params![at.author, at.seq as i64], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
    }

    /// Stores many entries in one transaction (for tests and seeding).
    pub fn insert_many(&self, entries: &[Entry]) -> Result<()> {
        self.0.execute_batch("BEGIN")?;
        let result = entries.iter().try_for_each(|e| self.insert(e).map(drop));
        self.0.execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
        result
    }

    /// Entries by `author` with a sequence number above `after`, oldest first.
    pub fn since(&self, author: &str, after: u64) -> Result<Vec<Entry>> {
        self.entries("WHERE author = ?1 AND seq > ?2 ORDER BY seq", params![author, after as i64])
    }

    /// Adds the latest edit, the quoted message and the reactions.
    fn show(&self, e: Entry) -> Result<Message> {
        let Kind::Message { reply_to, vanish, file } = e.kind else { bail!("not a message") };
        let at = Ref { author: e.author, seq: e.seq };
        let current = self.current(&at, e.text)?;
        let expires = self.expires(&at)?;
        let vanished = expires.is_some_and(|t| t <= self.1.now_ms());
        let (author, seq, ts, hlc) = (at.author.clone(), at.seq, e.ts, e.hlc);
        if current.deleted || vanished {
            let (text, reply_to, reactions) = (String::new(), None, Vec::new());
            return Ok(Message {
                author,
                seq,
                ts,
                hlc,
                text,
                edited: false,
                deleted: true,
                file: None,
                vanish,
                expires,
                vanished,
                reply_to,
                reactions,
            });
        }
        let reply_to = match reply_to {
            Some(to) => {
                let stored = self.original(&to)?;
                let hlc = stored.as_ref().map(|(_, hlc)| *hlc);
                let quoted = stored.map(|(text, _)| self.current(&to, text)).transpose()?;
                let vanished = self.vanished(&to)?;
                let deleted = vanished || quoted.as_ref().is_some_and(|q| q.deleted);
                let text = quoted.filter(|_| !deleted).map(|q| q.text);
                let file = if deleted { None } else { self.file_of(&to)? };
                let (author, seq) = (to.author, to.seq);
                Some(Quote { author, seq, hlc, text, deleted, vanished, file })
            }
            None => None,
        };
        let file = match file {
            Some(info) => {
                let (state, openable) = (self.file_state(&info.hash)?, may_open(&info.name));
                Some(Attachment { info, state, openable })
            }
            None => None,
        };
        let reactions = self.reactions(&at)?;
        let (text, edited) = (current.text, current.edited);
        Ok(Message {
            author,
            seq,
            ts,
            hlc,
            text,
            edited,
            deleted: false,
            file,
            vanish,
            expires,
            vanished: false,
            reply_to,
            reactions,
        })
    }

    /// A message's text now, given its original: its author's latest edit, or nothing if
    /// they deleted it. Edits and deletes by anyone else don't count.
    fn current(&self, at: &Ref, original: String) -> Result<Current> {
        let latest = self
            .0
            .prepare_cached(
                "SELECT kind, text FROM messages
                 WHERE ref_author = ?1 AND ref_seq = ?2 AND author = ?1
                   AND kind IN ('edit', 'delete')
                 ORDER BY kind = 'delete' DESC, seq DESC LIMIT 1",
            )?
            .query_row(params![at.author, at.seq as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .optional()?;
        Ok(match latest {
            Some((kind, _)) if kind == DELETE => {
                Current { text: String::new(), edited: false, deleted: true }
            }
            Some((_, text)) => Current { text, edited: true, deleted: false },
            None => Current { text: original, edited: false, deleted: false },
        })
    }

    /// Blanks the text and file details of a deleted message, and the text of its edits,
    /// zeroing the space. Only the message's author can delete it, so `of.author` is also
    /// whose edits these are. The file itself is removed once no message has it (see
    /// `unused_files`).
    pub fn scrub(&self, of: &Ref) -> Result<()> {
        self.0.execute(
            "DELETE FROM search WHERE author = ?1 AND seq = ?2",
            params![of.author, of.seq as i64],
        )?;
        self.0.execute(
            "UPDATE messages SET text = '', file = NULL, file_hash = NULL
             WHERE author = ?1 AND (
                 (kind = 'message' AND seq = ?2)
                 OR (kind = 'edit' AND ref_author = ?1 AND ref_seq = ?2))",
            params![of.author, of.seq as i64],
        )?;
        self.0.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        Ok(())
    }

    /// Entries matching `filter`, the SQL after `FROM messages`.
    fn entries(&self, filter: &str, p: impl rusqlite::Params) -> Result<Vec<Entry>> {
        let mut stmt = self.0.prepare_cached(&format!(
            "SELECT author, seq, ts, hlc, text, kind, ref_author, ref_seq, vanish, file
             FROM messages {filter}"
        ))?;
        let rows = stmt.query_map(p, |r| {
            let author: String = r.get(0)?;
            let to = match (r.get::<_, Option<String>>(6)?, r.get::<_, Option<i64>>(7)?) {
                (Some(author), Some(seq)) => Some(Ref { author, seq: seq as u64 }),
                _ => None,
            };
            let tag: String = r.get(5)?;
            let kind = match (tag.as_str(), to) {
                (MESSAGE, reply_to) => Kind::Message {
                    reply_to,
                    vanish: r.get::<_, Option<i64>>(8)?.map(|v| v as u64),
                    file: r
                        .get::<_, Option<String>>(9)?
                        .and_then(|json| serde_json::from_str(&json).ok()),
                },
                (REACTION, Some(to)) => Kind::Reaction { to },
                (EDIT | DELETE, Some(of)) => Kind::change(&author, &tag, of),
                // A kind this build doesn't know, left by a newer one.
                (_, to) => Kind::Inert { tag, to },
            };
            Ok(Entry {
                author,
                seq: r.get::<_, i64>(1)? as u64,
                ts: r.get(2)?,
                hlc: r.get(3)?,
                text: r.get(4)?,
                kind,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(author: &str, seq: u64) -> Entry {
        Entry::message(author, seq, seq as i64, format!("{author}{seq}"))
    }

    fn reply(author: &str, seq: u64, to: (&str, u64)) -> Entry {
        let reply_to = Some(Ref { author: to.0.into(), seq: to.1 });
        Entry { kind: Kind::Message { reply_to, vanish: None, file: None }, ..msg(author, seq) }
    }

    fn react(author: &str, seq: u64, emoji: &str, to: (&str, u64)) -> Entry {
        let to = Ref { author: to.0.into(), seq: to.1 };
        Entry { text: emoji.into(), kind: Kind::Reaction { to }, ..msg(author, seq) }
    }

    fn edit(author: &str, seq: u64, text: &str, of: (&str, u64)) -> Entry {
        let of = Ref { author: of.0.into(), seq: of.1 };
        Entry { text: text.into(), kind: Kind::Edit { of }, ..msg(author, seq) }
    }

    fn delete(author: &str, seq: u64, of: (&str, u64)) -> Entry {
        let of = Ref { author: of.0.into(), seq: of.1 };
        Entry { text: String::new(), kind: Kind::Delete { of }, ..msg(author, seq) }
    }

    fn texts(db: &Db) -> Vec<String> {
        db.all().unwrap().into_iter().map(|m| m.text).collect()
    }

    #[test]
    fn clear_keeps_numbering_and_undelivered_messages() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        db.insert_many(&[msg("me", 1), msg("me", 2), msg("me", 3), msg("peer", 1), msg("peer", 2)])
            .unwrap();

        // The peer has my first two; the third is still waiting.
        assert_eq!(db.clear("me", Some(2)).unwrap(), 4);
        assert_eq!(texts(&db), ["me3"]);
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
        let page = db.page(None, 10).unwrap().messages;
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].text, "peer3");
    }

    #[test]
    fn replies_quote_and_reactions_replace() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        db.insert_many(&[
            msg("me", 1),
            reply("peer", 1, ("me", 1)),
            react("peer", 2, "👍", ("me", 1)),
            react("me", 2, "❤️", ("me", 1)),
            react("peer", 3, "😂", ("me", 1)), // replaces their 👍
            react("me", 3, "", ("me", 1)),     // takes mine back
        ])
        .unwrap();

        let messages = db.all().unwrap();
        assert_eq!(messages.len(), 2, "reactions aren't messages");
        assert_eq!(messages[0].reactions, [Reaction { author: "peer".into(), emoji: "😂".into() }]);
        let quote = messages[1].reply_to.as_ref().unwrap();
        assert_eq!((quote.text.as_deref(), quote.hlc), (Some("me1"), Some(1)));
        assert_eq!(db.page(None, 10).unwrap().messages, messages);

        // Reactions sync and count like messages.
        assert_eq!(db.since("peer", 1).unwrap().len(), 2);
        assert_eq!(db.max_seq("me").unwrap(), 3);

        // A reply to a message that isn't here any more.
        db.clear("me", None).unwrap();
        db.insert(&reply("peer", 4, ("me", 1))).unwrap();
        let gone = db.all().unwrap()[0].reply_to.clone().unwrap();
        assert_eq!((gone.text, gone.hlc), (None, None), "nowhere to jump to");
    }

    #[test]
    fn migrates_a_database_from_before_replies() {
        let path =
            std::env::temp_dir().join(format!("tightbeam-migrate-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(
                "CREATE TABLE messages (author TEXT NOT NULL, seq INTEGER NOT NULL,
                     ts INTEGER NOT NULL, text TEXT NOT NULL, PRIMARY KEY (author, seq));
                 INSERT INTO messages VALUES ('peer', 1, 1, 'from 0.3');",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(texts(&db), ["from 0.3"]);
        assert_eq!(db.all().unwrap()[0].hlc, 1, "ordered by ts, as before");
        assert_eq!(db.search("0.3", 10).unwrap().len(), 1, "old messages are searchable");
        db.insert(&reply("me", 1, ("peer", 1))).unwrap();
        let all = db.all().unwrap();
        let mine = all.iter().find(|m| m.author == "me").unwrap();
        assert_eq!(mine.reply_to.as_ref().unwrap().text.as_deref(), Some("from 0.3"));
        drop(db);
        assert_eq!(
            texts(&Db::open(&path).unwrap()).len(),
            2,
            "opens again without migrating twice"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn every_kind_survives_the_log() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let at = |author: &str, seq| Ref { author: author.into(), seq };
        let file = FileInfo {
            hash: "a".repeat(64),
            name: "beach.jpg".into(),
            size: 1000,
            mime: "image/jpeg".into(),
            width: Some(4),
            height: Some(3),
            preview: None,
        };
        let full = Entry {
            hlc: 9,
            kind: Kind::Message { reply_to: Some(at("me", 1)), vanish: Some(60), file: Some(file) },
            ..msg("peer", 1)
        };
        let inert = Kind::Inert { tag: EDIT.into(), to: Some(at("me", 1)) };
        let entries = [
            full,
            react("peer", 2, "👍", ("me", 1)),
            edit("peer", 3, "fixed", ("peer", 1)),
            delete("peer", 4, ("peer", 1)),
            Entry { kind: inert.clone(), ..msg("peer", 5) },
        ];
        db.insert_many(&entries).unwrap();
        assert_eq!(db.since("peer", 0).unwrap(), entries);

        // Someone else's edit reads back as inert, however it was stored.
        db.insert(&edit("peer", 6, "not yours", ("me", 1))).unwrap();
        assert_eq!(db.since("peer", 5).unwrap()[0].kind, inert);

        // A kind this build doesn't know, left by a newer one.
        db.0.execute(
            "INSERT INTO messages (author, seq, ts, hlc, text, kind, ref_author, ref_seq)
             VALUES ('me', 1, 1, 1, 'x', 'pin', 'peer', 1)",
            [],
        )
        .unwrap();
        let pin = Kind::Inert { tag: "pin".into(), to: Some(at("peer", 1)) };
        assert_eq!(db.since("me", 0).unwrap()[0].kind, pin);
        assert!(db.all().unwrap().iter().all(|m| m.author == "peer"), "never shown");
    }

    #[test]
    fn people_from_before_0_8_are_kept() {
        let path = std::env::temp_dir().join(format!(
            "tightbeam-people-{}.db",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        {
            // A database from 0.7, paired with Bob.
            let db = Db::open(&path).unwrap();
            db.set("peer_id", "bob-id").unwrap();
            db.set("peer_name", "Bob").unwrap();
            db.0.execute_batch("DROP TABLE people; PRAGMA user_version = 5;").unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.people().unwrap(), [("bob-id".to_string(), "Bob".to_string())]);
        db.set_person("bob-id", "Robert").unwrap();
        db.set_person("carol-id", "Carol").unwrap();
        let names: Vec<_> = db.people().unwrap().into_iter().map(|(_, name)| name).collect();
        assert_eq!(names, ["Robert", "Carol"]);
        drop(db);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn edits_and_deletes() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        db.insert_many(&[
            msg("me", 1),
            msg("me", 2),
            reply("peer", 1, ("me", 1)),
            reply("peer", 2, ("me", 2)),
            edit("me", 3, "first, fixed", ("me", 1)),
            edit("me", 4, "first, fixed again", ("me", 1)),
            edit("peer", 3, "not yours", ("me", 2)), // only the author can edit
            react("peer", 4, "👍", ("me", 2)),
        ])
        .unwrap();
        let find = |db: &Db, author: &str, seq: u64| {
            db.all().unwrap().into_iter().find(|m| m.author == author && m.seq == seq).unwrap()
        };

        let first = find(&db, "me", 1);
        assert_eq!((first.text.as_str(), first.edited), ("first, fixed again", true));
        assert_eq!(
            find(&db, "peer", 1).reply_to.unwrap().text.as_deref(),
            Some("first, fixed again")
        );
        let second = find(&db, "me", 2);
        assert_eq!((second.text.as_str(), second.edited), ("me2", false));
        assert_eq!(db.all().unwrap().len(), 4, "edits aren't messages");

        // A delete by someone else does nothing.
        db.insert(&delete("peer", 5, ("me", 1))).unwrap();
        assert!(!find(&db, "me", 1).deleted);

        // Deleting: a placeholder, and the quote says so.
        db.insert(&delete("me", 5, ("me", 1))).unwrap();
        db.scrub(&Ref { author: "me".into(), seq: 1 }).unwrap();
        let first = find(&db, "me", 1);
        assert!(first.deleted && first.text.is_empty() && first.reactions.is_empty());
        let quote = find(&db, "peer", 1).reply_to.unwrap();
        assert!(quote.deleted && quote.text.is_none());
        // An edit after the delete doesn't bring it back.
        db.insert(&edit("me", 6, "back?", ("me", 1))).unwrap();
        assert!(find(&db, "me", 1).deleted);

        // The deleted text and its edits are blank in storage, so they sync blank too.
        let stored: Vec<_> = db.since("me", 0).unwrap().into_iter().map(|e| e.text).collect();
        assert_eq!(stored, ["", "me2", "", "", "", "back?"]);
    }

    #[test]
    fn orders_by_the_clock_not_the_wall_time() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        // The peer's clock is ten seconds ahead of mine.
        db.insert(&Entry::message("peer", 1, 11_000, "question")).unwrap();
        let now = 1_000;
        let hlc = db.next_hlc(now).unwrap();
        assert_eq!(hlc, 11_001);
        db.insert(&Entry { hlc, ..Entry::message("me", 1, now, "answer") }).unwrap();
        assert_eq!(texts(&db), ["question", "answer"]);
        assert_eq!(db.all().unwrap()[1].ts, now, "shown at my time");
        let before = db.page(Some(&Cursor { hlc, author: "me".into(), seq: 1 }), 10).unwrap();
        assert_eq!(before.messages.len(), 1, "paging goes by the clock too");

        // An ordinary clock just uses the time.
        assert_eq!(db.next_hlc(20_000).unwrap(), 20_000);

        // Erasing the history doesn't wind the clock back.
        db.clear("me", None).unwrap();
        assert_eq!(db.next_hlc(now).unwrap(), 11_002);
    }

    #[test]
    fn search() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let say =
            |author: &str, seq: u64, text: &str| Entry::message(author, seq, seq as i64, text);
        db.insert_many(&[
            say("me", 1, "Lunch at the café?"),
            say("peer", 1, "lunchtime works"),
            say("me", 2, "great, see you there"),
            say("peer", 2, "bring the \"book\" (the blue one)"),
        ])
        .unwrap();
        let found = |q: &str| -> Vec<String> {
            db.search(q, 10).unwrap().into_iter().map(|h| h.message.text).collect()
        };

        assert_eq!(found("lunch"), ["lunchtime works", "Lunch at the café?"], "newest first");
        assert_eq!(found("CAFE"), ["Lunch at the café?"], "ignores case and accents");
        assert_eq!(found("lunch cafe"), ["Lunch at the café?"], "every word must match");
        assert_eq!(found("\"book\" (blue"), [r#"bring the "book" (the blue one)"#]);
        assert!(found("* OR -").is_empty(), "search syntax is just text");
        let hit = &db.search("great", 10).unwrap()[0];
        assert_eq!(hit.snippet, "\u{2}great\u{3}, see you there");

        // Edits replace the text; deletes remove it.
        db.insert(&edit("me", 3, "dinner at the café?", ("me", 1))).unwrap();
        assert_eq!(found("cafe"), ["dinner at the café?"]);
        assert_eq!(found("lunch"), ["lunchtime works"]);
        db.insert(&delete("peer", 3, ("me", 2))).unwrap(); // not theirs to delete
        assert_eq!(found("great").len(), 1);
        db.insert(&delete("me", 4, ("me", 2))).unwrap();
        assert!(found("great").is_empty());

        // Clearing empties the index, except what's kept.
        db.clear("me", Some(3)).unwrap();
        assert!(found("cafe").is_empty() && found("lunch").is_empty());
    }

    #[test]
    fn range_fills_the_gap_to_a_message() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        db.insert_many(&(1..=5).map(|i| msg("me", i)).collect::<Vec<_>>()).unwrap();
        let at = |seq: u64| Cursor { hlc: seq as i64, author: "me".into(), seq };
        let texts: Vec<_> = db.range(&at(2), &at(5)).unwrap().into_iter().map(|m| m.text).collect();
        assert_eq!(texts, ["me2", "me3", "me4"]);
    }

    #[test]
    fn vanishing_messages() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let vanishing = |author: &str, seq: u64, secs: u64| Entry {
            kind: Kind::Message { reply_to: None, vanish: Some(secs), file: None },
            ..Entry::message(author, seq, seq as i64, "secret plans")
        };
        db.insert_many(&[
            vanishing("peer", 1, 60),
            reply("me", 1, ("peer", 1)),
            vanishing("peer", 2, 60),
        ])
        .unwrap();
        let find = |seq: u64| {
            db.all().unwrap().into_iter().find(|m| m.author == "peer" && m.seq == seq).unwrap()
        };
        assert_eq!(
            db.since("peer", 0).unwrap()[0].kind,
            Kind::Message { reply_to: None, vanish: Some(60), file: None }
        );

        // Nothing happens until it's seen.
        assert!(db.expire(i64::MAX).unwrap().is_empty());
        assert_eq!(find(1).expires, None);

        // Seen at t=1000: it vanishes a minute later. The unseen one doesn't start.
        let started = db.start_vanish("peer", 1, 1_000).unwrap();
        assert_eq!(started, [Ref { author: "peer".into(), seq: 1 }]);
        assert!(db.start_vanish("peer", 1, 5_000).unwrap().is_empty(), "starts once");
        assert_eq!(find(1).expires, Some(61_000));
        assert_eq!(find(2).expires, None);
        assert!(db.expire(60_999).unwrap().is_empty());

        // Time's up (`expire` gets the time; `show` uses the real clock, long past 61s).
        assert_eq!(db.expire(61_000).unwrap().len(), 1);
        let gone = find(1);
        assert!(gone.vanished && gone.deleted && gone.text.is_empty());
        let quote = db.all().unwrap().into_iter().find(|m| m.author == "me").unwrap();
        let quote = quote.reply_to.unwrap();
        assert!(quote.vanished && quote.deleted && quote.text.is_none());
        assert_eq!(db.search("secret", 10).unwrap().len(), 1, "only the unseen one");
        assert_eq!(db.since("peer", 0).unwrap()[0].text, "", "erased in storage");
        assert!(db.expire(i64::MAX).unwrap().iter().all(|at| at.seq == 2), "erased once");

        // An edit that arrives after it vanished isn't indexed.
        db.insert(&edit("peer", 3, "secret again", ("peer", 1))).unwrap();
        assert!(find(1).vanished);
        assert!(db.search("again", 10).unwrap().is_empty());
    }

    #[test]
    fn files() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let photo = |hash: &str| FileInfo {
            hash: hash.repeat(64),
            name: "beach.jpg".into(),
            size: 1000,
            mime: "image/jpeg".into(),
            width: Some(4),
            height: Some(3),
            preview: None,
        };
        let with_file = |author: &str, seq: u64, text: &str, file: FileInfo| Entry {
            kind: Kind::Message { reply_to: None, vanish: None, file: Some(file) },
            ..Entry::message(author, seq, seq as i64, text)
        };
        db.insert_many(&[
            with_file("peer", 1, "", photo("a")),
            with_file("peer", 2, "the view", photo("b")),
            with_file("me", 1, "same again", photo("a")),
            reply("me", 2, ("peer", 2)),
        ])
        .unwrap();
        let find = |author: &str, seq: u64| {
            db.all().unwrap().into_iter().find(|m| m.author == author && m.seq == seq).unwrap()
        };
        let a = "a".repeat(64);

        // It syncs with the message, and shows as on its way.
        assert_eq!(db.since("peer", 0).unwrap()[0].kind, with_file("x", 1, "", photo("a")).kind);
        let shown = find("peer", 1).file.unwrap();
        assert_eq!((shown.info, shown.state), (photo("a"), FileState::Pending));
        assert_eq!(find("me", 2).reply_to.unwrap().file, Some(photo("b")), "replies show it");

        // Each file is wanted once, newest message first.
        let wanted: Vec<_> = db.wanted_files().unwrap().into_iter().map(|f| f.hash).collect();
        assert_eq!(wanted, ["b".repeat(64), a.clone()]);
        db.set_file(&a, "beach.jpg", FileState::Done).unwrap();
        assert_eq!(find("peer", 1).file.unwrap().state, FileState::Done);
        assert_eq!(db.wanted_files().unwrap().len(), 1);
        let stored = db.stored_file(&a).unwrap().unwrap();
        assert_eq!((stored.name.as_str(), stored.mime.as_str()), ("beach.jpg", "image/jpeg"));
        assert_eq!(db.with_file(&a).unwrap().len(), 2);

        // Found by name, or by caption and name together.
        assert_eq!(db.search("beach", 10).unwrap().len(), 3);
        assert_eq!(db.search("view beach", 10).unwrap().len(), 1);

        // Deleting one message leaves the file for the other.
        db.insert(&delete("peer", 3, ("peer", 1))).unwrap();
        db.scrub(&Ref { author: "peer".into(), seq: 1 }).unwrap();
        assert!(find("peer", 1).file.is_none());
        assert_eq!(db.since("peer", 0).unwrap()[0].kind, msg("x", 1).kind, "blanked in storage");
        assert!(db.file_in_use(&a).unwrap() && db.unused_files().unwrap().is_empty());
        db.insert(&delete("me", 3, ("me", 1))).unwrap();
        db.scrub(&Ref { author: "me".into(), seq: 1 }).unwrap();
        assert!(!db.file_in_use(&a).unwrap());
        assert!(db.stored_file(&a).unwrap().is_none(), "never served once unused");
        assert_eq!(db.unused_files().unwrap(), [a.as_str()]);
        db.forget_file(&a).unwrap();

        // Clearing leaves nothing wanted.
        db.clear("me", None).unwrap();
        assert!(db.wanted_files().unwrap().is_empty());
    }
}
