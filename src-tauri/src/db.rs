//! Local storage: settings (keys, names, pairing) and the log.
//!
//! The log holds every author's numbered entries: messages, and reactions, edits and
//! deletes that refer to an earlier message by `(author, seq)`. A reply also refers to
//! the message it answers. Entries are only ever added, with one exception: a delete
//! blanks the text of the message it deletes, and of that message's edits.

use std::path::Path;

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// An entry, by author and sequence number.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ref {
    pub author: String,
    pub seq: u64,
}

/// One entry in an author's log, as stored and synced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Endpoint id (public key) of whoever wrote it.
    pub author: String,
    /// Per-author sequence number, starting at 1 with no gaps.
    pub seq: u64,
    /// Milliseconds since the Unix epoch, from the author's clock.
    pub ts: i64,
    /// The message, the reaction's emoji ("" takes a reaction back), the edit's new text,
    /// or "" for a delete.
    pub text: String,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Message {
        reply_to: Option<Ref>,
    },
    /// Replaces the author's earlier reaction to `to`, if any.
    Reaction {
        to: Ref,
    },
    /// New text for one of the author's own messages. The latest edit wins.
    Edit {
        of: Ref,
    },
    /// Deletes one of the author's own messages.
    Delete {
        of: Ref,
    },
}

impl Entry {
    pub fn message(author: impl Into<String>, seq: u64, ts: i64, text: impl Into<String>) -> Self {
        let (author, text) = (author.into(), text.into());
        Self { author, seq, ts, text, kind: Kind::Message { reply_to: None } }
    }
}

/// A message as the transcript shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Message {
    pub author: String,
    pub seq: u64,
    pub ts: i64,
    /// The latest edit's text, if edited. Empty once deleted.
    pub text: String,
    pub edited: bool,
    /// Deleted by its author. Shown as a placeholder, without reply or reactions.
    pub deleted: bool,
    /// The message this one answers.
    pub reply_to: Option<Quote>,
    /// At most one per person, in author order.
    pub reactions: Vec<Reaction>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Quote {
    pub author: String,
    pub seq: u64,
    /// Its latest text. `None` when it isn't stored here (cleared, for example) or was
    /// deleted.
    pub text: Option<String>,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Reaction {
    pub author: String,
    pub emoji: String,
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

/// Values of the `kind` column.
const MESSAGE: &str = "message";
const REACTION: &str = "reaction";
const EDIT: &str = "edit";
const DELETE: &str = "delete";

/// What `Db::current` finds.
struct Current {
    text: String,
    edited: bool,
    deleted: bool,
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
        Ok(Self(conn))
    }

    /// Up to `limit` messages before `before` in display order (or the newest, if `None`),
    /// returned oldest first, plus whether anything older remains.
    pub fn page(&self, before: Option<&Cursor>, limit: usize) -> Result<Page> {
        let fetch = limit as i64 + 1; // one extra row tells us whether there's more
        let entries = match before {
            Some(c) => self.entries(
                "WHERE kind = 'message' AND (ts, author, seq) < (?1, ?2, ?3)
                 ORDER BY ts DESC, author DESC, seq DESC LIMIT ?4",
                params![c.ts, c.author, c.seq as i64, fetch],
            )?,
            None => self.entries(
                "WHERE kind = 'message' ORDER BY ts DESC, author DESC, seq DESC LIMIT ?1",
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
        let entries = self.entries("WHERE kind = 'message' ORDER BY ts, author, seq", [])?;
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

    /// Erases the history, except `keep_author`'s entries after `keep_after` (mine that
    /// haven't been delivered yet). The space is zeroed rather than just freed. Returns
    /// how many entries were erased.
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

    /// Stores an entry. Returns false if it was already stored.
    pub fn insert(&self, e: &Entry) -> Result<bool> {
        let (kind, to) = match &e.kind {
            Kind::Message { reply_to } => (MESSAGE, reply_to.as_ref()),
            Kind::Reaction { to } => (REACTION, Some(to)),
            Kind::Edit { of } => (EDIT, Some(of)),
            Kind::Delete { of } => (DELETE, Some(of)),
        };
        let changed = self.0.execute(
            "INSERT OR IGNORE INTO messages (author, seq, ts, text, kind, ref_author, ref_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                e.author,
                e.seq as i64,
                e.ts,
                e.text,
                kind,
                to.map(|r| &r.author),
                to.map(|r| r.seq as i64)
            ],
        )?;
        Ok(changed > 0)
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
        let Kind::Message { reply_to } = e.kind else { bail!("not a message") };
        let at = Ref { author: e.author, seq: e.seq };
        let current = self.current(&at, e.text)?;
        let (author, seq, ts) = (at.author.clone(), at.seq, e.ts);
        if current.deleted {
            let (text, reply_to, reactions) = (String::new(), None, Vec::new());
            return Ok(Message {
                author,
                seq,
                ts,
                text,
                edited: false,
                deleted: true,
                reply_to,
                reactions,
            });
        }
        let reply_to = match reply_to {
            Some(to) => {
                let original = self
                    .0
                    .prepare_cached(
                        "SELECT text FROM messages
                         WHERE author = ?1 AND seq = ?2 AND kind = 'message'",
                    )?
                    .query_row(params![to.author, to.seq as i64], |r| r.get::<_, String>(0))
                    .optional()?;
                let quoted = original.map(|text| self.current(&to, text)).transpose()?;
                let deleted = quoted.as_ref().is_some_and(|q| q.deleted);
                let text = quoted.filter(|q| !q.deleted).map(|q| q.text);
                Some(Quote { author: to.author, seq: to.seq, text, deleted })
            }
            None => None,
        };
        let reactions = self.reactions(&at)?;
        let (text, edited) = (current.text, current.edited);
        Ok(Message { author, seq, ts, text, edited, deleted: false, reply_to, reactions })
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

    /// Blanks the text of a deleted message and of its edits, zeroing the space. Only the
    /// message's author can delete it, so `of.author` is also whose edits these are.
    pub fn scrub(&self, of: &Ref) -> Result<()> {
        self.0.execute_batch("PRAGMA secure_delete = ON")?;
        let result = self.0.execute(
            "UPDATE messages SET text = ''
             WHERE author = ?1 AND (
                 (kind = 'message' AND seq = ?2)
                 OR (kind = 'edit' AND ref_author = ?1 AND ref_seq = ?2))",
            params![of.author, of.seq as i64],
        );
        self.0.execute_batch("PRAGMA secure_delete = OFF; PRAGMA wal_checkpoint(TRUNCATE);")?;
        result?;
        Ok(())
    }

    /// Entries matching `filter`, the SQL after `FROM messages`.
    fn entries(&self, filter: &str, p: impl rusqlite::Params) -> Result<Vec<Entry>> {
        let mut stmt = self.0.prepare_cached(&format!(
            "SELECT author, seq, ts, text, kind, ref_author, ref_seq FROM messages {filter}"
        ))?;
        let rows = stmt.query_map(p, |r| {
            let to = match (r.get::<_, Option<String>>(5)?, r.get::<_, Option<i64>>(6)?) {
                (Some(author), Some(seq)) => Some(Ref { author, seq: seq as u64 }),
                _ => None,
            };
            let kind = match (r.get::<_, String>(4)?.as_str(), to) {
                (REACTION, Some(to)) => Kind::Reaction { to },
                (EDIT, Some(of)) => Kind::Edit { of },
                (DELETE, Some(of)) => Kind::Delete { of },
                (_, reply_to) => Kind::Message { reply_to },
            };
            Ok(Entry {
                author: r.get(0)?,
                seq: r.get::<_, i64>(1)? as u64,
                ts: r.get(2)?,
                text: r.get(3)?,
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
        Entry { kind: Kind::Message { reply_to }, ..msg(author, seq) }
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
        assert_eq!(quote.text.as_deref(), Some("me1"));
        assert_eq!(db.page(None, 10).unwrap().messages, messages);

        // Reactions sync and count like messages.
        assert_eq!(db.since("peer", 1).unwrap().len(), 2);
        assert_eq!(db.max_seq("me").unwrap(), 3);

        // A reply to a message that isn't here any more.
        db.clear("me", None).unwrap();
        db.insert(&reply("peer", 4, ("me", 1))).unwrap();
        assert_eq!(db.all().unwrap()[0].reply_to.as_ref().unwrap().text, None);
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
}
