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

    /// How this entry goes over the wire, in a `msg` frame.
    pub(crate) fn into_wire(self) -> Wire {
        let (mut reply_to, mut react_to, mut edit_of, mut delete_of) = (None, None, None, None);
        let (mut vanish, mut file) = (None, None);
        match self.kind {
            Kind::Message { reply_to: to, vanish: v, file: f } => {
                (reply_to, vanish, file) = (to, v, f.map(Box::new))
            }
            Kind::Reaction { to } => react_to = Some(to),
            Kind::Edit { of } => edit_of = Some(of),
            Kind::Delete { of } => delete_of = Some(of),
        }
        let (seq, ts, hlc, text) = (self.seq, self.ts, Some(self.hlc), self.text);
        Wire { seq, ts, hlc, text, reply_to, react_to, edit_of, delete_of, vanish, file }
    }
}

/// An entry in the sender's log, as the body of a `msg` frame. Plain, it's a message.
///
/// The same rules as for frames: new fields must be `#[serde(default)]`, so older peers
/// ignore them, and an existing field never changes its meaning or type.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Wire {
    pub seq: u64,
    pub ts: i64,
    /// Display order (see `Entry::hlc`). Added in 0.5; without it, `ts` is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hlc: Option<i64>,
    pub text: String,
    /// The message this one answers. Added in 0.4; older peers show a plain message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Ref>,
    /// Makes this a reaction to that message: `text` is the emoji, or "" to take the
    /// sender's reaction back. Added in 0.4; older peers show the emoji as a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub react_to: Option<Ref>,
    /// Makes this new text for one of the sender's own messages. Added in 0.4; older peers
    /// show the new text as a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_of: Option<Ref>,
    /// Deletes one of the sender's own messages; `text` is "". Added in 0.4; older peers
    /// show an empty message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_of: Option<Ref>,
    /// A vanishing message: each side erases it this many seconds after the recipient
    /// has seen it. Added in 0.6, and only sent to peers that say their version in
    /// `Hello`: older ones would keep it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vanish: Option<u64>,
    /// A file, with `text` as its caption. Added in 0.7, and only sent to peers whose
    /// `Hello` lists the `files` feature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<Box<FileInfo>>,
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
