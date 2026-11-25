//! Log entries: what each kind is.
//!
//! Every author keeps a log of numbered entries: messages, and reactions, edits and
//! deletes that refer to an earlier message by `(author, seq)`. A reply also refers to
//! the message it answers.

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
    /// Milliseconds since the Unix epoch, from the author's clock. The time shown.
    pub ts: i64,
    /// Display order: `ts`, or one past the highest `hlc` the author had stored, if
    /// that's later. Entries from peers before 0.5 have `hlc = ts`.
    pub hlc: i64,
    /// The message, the reaction's emoji ("" takes a reaction back), the edit's new text,
    /// or "" for a delete.
    pub text: String,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Message {
        reply_to: Option<Ref>,
        /// Seconds it lasts once seen, for a vanishing message.
        vanish: Option<u64>,
        file: Option<FileInfo>,
    },
    /// Replaces the author's earlier reaction to `to`, if any.
    Reaction { to: Ref },
    /// New text for one of the author's own messages. The latest edit wins.
    Edit { of: Ref },
    /// Deletes one of the author's own messages.
    Delete { of: Ref },
}

impl Entry {
    pub fn message(author: impl Into<String>, seq: u64, ts: i64, text: impl Into<String>) -> Self {
        let (author, text) = (author.into(), text.into());
        let kind = Kind::Message { reply_to: None, vanish: None, file: None };
        Self { author, seq, ts, hlc: ts, text, kind }
    }
}

/// A file attached to a message, as its sender describes it. Synced with the message;
/// the bytes follow separately.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileInfo {
    /// BLAKE3 of the contents: 64 lowercase hex digits.
    pub hash: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
    /// Set for an image the sender's app could display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// A tiny copy of the image as a `data:` URL, shown until the file arrives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}
