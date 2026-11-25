//! Log entries: what each kind is, how it goes over the wire, and what a peer's entry
//! must pass to be stored (admission).
//!
//! Every author keeps a log of numbered entries: messages, and reactions, edits and
//! deletes that refer to an earlier message by `(author, seq)`. A reply also refers to
//! the message it answers.
//!
//! The limits here hold for both sides. My own entries are refused with an error when
//! they break one (see `Core`); a peer's are repaired instead, since the entry has to
//! be stored either way to keep their log without gaps.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

/// The longest text an entry carries, in characters: a message, a caption or an edit.
pub(crate) const MAX_TEXT_CHARS: usize = 4000;
/// A reaction's emoji: room for one made of several code points, like a family or a flag.
pub(crate) const MAX_EMOJI_CHARS: usize = 16;
/// How long a vanishing message can last once seen: 5 seconds to 7 days.
pub(crate) const VANISH_SECS: RangeInclusive<u64> = 5..=7 * 24 * 60 * 60;
/// The largest file that can be sent: 1 GB.
pub(crate) const MAX_FILE_BYTES: u64 = 1 << 30;
const MAX_FILE_NAME_CHARS: usize = 200;
/// Room for a tiny JPEG preview as a `data:` URL.
const MAX_PREVIEW_CHARS: usize = 8 * 1024;

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
    /// Kept in the author's log, but never applied, shown or sent: an edit or delete of
    /// someone else's message, or a kind this build doesn't know (left by a newer one).
    /// `tag` and `to` are what it was stored as.
    Inert { tag: String, to: Option<Ref> },
}

/// Names of the kinds, as the log stores them.
pub(crate) const MESSAGE: &str = "message";
pub(crate) const REACTION: &str = "reaction";
pub(crate) const EDIT: &str = "edit";
pub(crate) const DELETE: &str = "delete";

impl Kind {
    /// The kind's name in the log.
    pub(crate) fn tag(&self) -> &str {
        match self {
            Kind::Message { .. } => MESSAGE,
            Kind::Reaction { .. } => REACTION,
            Kind::Edit { .. } => EDIT,
            Kind::Delete { .. } => DELETE,
            Kind::Inert { tag, .. } => tag,
        }
    }

    /// An edit or delete by `author`: it counts only on their own message.
    pub(crate) fn change(author: &str, tag: &str, of: Ref) -> Kind {
        match tag {
            EDIT if of.author == author => Kind::Edit { of },
            DELETE if of.author == author => Kind::Delete { of },
            _ => Kind::Inert { tag: tag.to_string(), to: Some(of) },
        }
    }

    /// The longest text an entry of this kind carries.
    pub(crate) fn max_chars(&self) -> usize {
        match self {
            Kind::Reaction { .. } => MAX_EMOJI_CHARS,
            _ => MAX_TEXT_CHARS,
        }
    }
}

impl Entry {
    pub fn message(author: impl Into<String>, seq: u64, ts: i64, text: impl Into<String>) -> Self {
        let (author, text) = (author.into(), text.into());
        let kind = Kind::Message { reply_to: None, vanish: None, file: None };
        Self { author, seq, ts, hlc: ts, text, kind }
    }

    /// How this entry goes over the wire, in a `msg` frame. An inert entry isn't sent:
    /// the peer's `have` is the highest entry it holds, so a gap in what it gets is fine.
    pub(crate) fn into_wire(self) -> Option<Wire> {
        let (mut reply_to, mut react_to, mut edit_of, mut delete_of) = (None, None, None, None);
        let (mut vanish, mut file) = (None, None);
        match self.kind {
            Kind::Message { reply_to: to, vanish: v, file: f } => {
                (reply_to, vanish, file) = (to, v, f.map(Box::new))
            }
            Kind::Reaction { to } => react_to = Some(to),
            Kind::Edit { of } => edit_of = Some(of),
            Kind::Delete { of } => delete_of = Some(of),
            Kind::Inert { .. } => return None,
        }
        let (seq, ts, hlc, text) = (self.seq, self.ts, Some(self.hlc), self.text);
        Some(Wire { seq, ts, hlc, text, reply_to, react_to, edit_of, delete_of, vanish, file })
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

impl Wire {
    /// Admission: the entry `author` sent, as it's stored here, where `me` is this
    /// computer's id.
    ///
    /// Only one of `delete_of`, `edit_of` and `react_to` counts, in that order. A reply
    /// to anyone but the two of us becomes a plain message. An edit or delete of anyone's
    /// message but the sender's own is inert. A reaction to a stranger's message is kept
    /// but never shown, since no such message is stored here. Text is cut to the kind's
    /// limit, a lifetime is brought into range, and a file that's too big or badly
    /// described is left off.
    pub(crate) fn admit(self, author: &str, me: &str) -> Entry {
        let kind = match (self.delete_of, self.edit_of, self.react_to) {
            (Some(of), ..) => Kind::change(author, DELETE, of),
            (None, Some(of), _) => Kind::change(author, EDIT, of),
            (None, None, Some(to)) => Kind::Reaction { to },
            (None, None, None) => Kind::Message {
                reply_to: self.reply_to.filter(|to| to.author == me || to.author == author),
                vanish: self.vanish.map(|v| v.clamp(*VANISH_SECS.start(), *VANISH_SECS.end())),
                file: self.file.and_then(|f| checked_file(*f)),
            },
        };
        Entry {
            author: author.to_string(),
            seq: self.seq,
            ts: self.ts,
            hlc: self.hlc.unwrap_or(self.ts),
            text: clean(&self.text, kind.max_chars()),
            kind,
        }
    }
}

/// Text without surrounding space, cut to `max_chars` characters.
pub(crate) fn clean(s: &str, max_chars: usize) -> String {
    s.trim().chars().take(max_chars).collect()
}

/// A BLAKE3 hash as files are named by: 64 lowercase hex digits.
pub(crate) fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A file's description, tidied, or `None` if it can't be accepted: no valid hash, empty,
/// or over the size limit. Image details that don't make sense are dropped.
pub(crate) fn checked_file(file: FileInfo) -> Option<FileInfo> {
    if !is_hash(&file.hash) || file.size == 0 || file.size > MAX_FILE_BYTES {
        return None;
    }
    let mut name = clean(&file.name, MAX_FILE_NAME_CHARS);
    if name.is_empty() {
        name = "file".into();
    }
    let mime = file.mime.trim().to_ascii_lowercase();
    let mime_ok = mime.len() <= 100
        && mime.contains('/')
        && mime.chars().all(|c| c.is_ascii_alphanumeric() || "/.+-".contains(c));
    let mime = if mime_ok { mime } else { "application/octet-stream".into() };
    let dims = match (file.width, file.height) {
        (Some(w), Some(h)) if (1..=100_000).contains(&w) && (1..=100_000).contains(&h) => {
            Some((w, h))
        }
        _ => None,
    };
    let preview = file.preview.filter(|p| {
        p.len() <= MAX_PREVIEW_CHARS
            && ["data:image/jpeg;base64,", "data:image/png;base64,", "data:image/webp;base64,"]
                .iter()
                .any(|prefix| p.starts_with(prefix))
            && p.split_once(',').is_some_and(|(_, data)| {
                data.chars().all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c))
            })
    });
    Some(FileInfo {
        hash: file.hash,
        name,
        size: file.size,
        mime,
        width: dims.map(|d| d.0),
        height: dims.map(|d| d.1),
        preview: preview.filter(|_| dims.is_some()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "me";
    const PEER: &str = "peer";

    fn wire(json: &str) -> Wire {
        serde_json::from_str(json).unwrap()
    }

    fn at(author: &str, seq: u64) -> Ref {
        Ref { author: author.into(), seq }
    }

    fn photo() -> FileInfo {
        FileInfo {
            hash: "ab".repeat(32),
            name: "a.png".into(),
            size: 5,
            mime: "image/png".into(),
            width: Some(4),
            height: Some(3),
            preview: None,
        }
    }

    #[test]
    fn every_kind_survives_the_wire() {
        let entries = [
            Entry {
                hlc: 9,
                kind: Kind::Message {
                    reply_to: Some(at(ME, 1)),
                    vanish: Some(60),
                    file: Some(photo()),
                },
                ..Entry::message(PEER, 2, 5, "caption")
            },
            Entry { kind: Kind::Reaction { to: at(ME, 1) }, ..Entry::message(PEER, 3, 5, "👍") },
            Entry { kind: Kind::Edit { of: at(PEER, 2) }, ..Entry::message(PEER, 4, 5, "new") },
            Entry { kind: Kind::Delete { of: at(PEER, 2) }, ..Entry::message(PEER, 5, 5, "") },
        ];
        for entry in entries {
            let json = serde_json::to_string(&entry.clone().into_wire().unwrap()).unwrap();
            assert_eq!(wire(&json).admit(PEER, ME), entry, "{json}");
        }
    }

    #[test]
    fn changes_to_someone_elses_message_are_inert() {
        let edit = wire(r#"{"seq":1,"ts":1,"text":"x","edit_of":{"author":"me","seq":1}}"#);
        let inert = Kind::Inert { tag: EDIT.into(), to: Some(at(ME, 1)) };
        assert_eq!(edit.admit(PEER, ME).kind, inert);
        let delete = wire(r#"{"seq":1,"ts":1,"text":"","delete_of":{"author":"me","seq":1}}"#);
        let inert = Kind::Inert { tag: DELETE.into(), to: Some(at(ME, 1)) };
        assert_eq!(delete.admit(PEER, ME).kind, inert);

        // Never sent, whatever it was.
        let entry = Entry { kind: inert, ..Entry::message(ME, 1, 1, "") };
        assert!(entry.into_wire().is_none());
        let unknown = Kind::Inert { tag: "pin".into(), to: None };
        assert!(Entry { kind: unknown, ..Entry::message(ME, 2, 1, "") }.into_wire().is_none());
    }

    #[test]
    fn only_one_target_counts() {
        let all = r#"{"seq":1,"ts":1,"text":"x","react_to":{"author":"me","seq":1},
            "edit_of":{"author":"peer","seq":1},"delete_of":{"author":"peer","seq":2}}"#;
        assert_eq!(wire(all).admit(PEER, ME).kind, Kind::Delete { of: at(PEER, 2) });
        let two = r#"{"seq":1,"ts":1,"text":"x","react_to":{"author":"me","seq":1},
            "edit_of":{"author":"peer","seq":1}}"#;
        assert_eq!(wire(two).admit(PEER, ME).kind, Kind::Edit { of: at(PEER, 1) });
        let reply_and_reaction = r#"{"seq":1,"ts":1,"text":"x",
            "reply_to":{"author":"me","seq":3},"react_to":{"author":"me","seq":1}}"#;
        let kind = wire(reply_and_reaction).admit(PEER, ME).kind;
        assert_eq!(kind, Kind::Reaction { to: at(ME, 1) });
    }

    #[test]
    fn admission_repairs_what_it_can() {
        // Before 0.5 there's no `hlc`: the author's clock stands in.
        let old = wire(r#"{"seq":1,"ts":7,"text":"  hi  "}"#).admit(PEER, ME);
        assert_eq!(old, Entry::message(PEER, 1, 7, "hi"));

        let long = "x".repeat(MAX_TEXT_CHARS + 10);
        let json = format!(r#"{{"seq":1,"ts":1,"text":"{long}"}}"#);
        assert_eq!(wire(&json).admit(PEER, ME).text.chars().count(), MAX_TEXT_CHARS);
        let json =
            format!(r#"{{"seq":1,"ts":1,"text":"{long}","react_to":{{"author":"me","seq":1}}}}"#);
        assert_eq!(wire(&json).admit(PEER, ME).text.chars().count(), MAX_EMOJI_CHARS);

        let reply = |author: &str| {
            let json = format!(
                r#"{{"seq":1,"ts":1,"text":"x","reply_to":{{"author":"{author}","seq":1}}}}"#
            );
            match wire(&json).admit(PEER, ME).kind {
                Kind::Message { reply_to, .. } => reply_to,
                kind => panic!("{kind:?}"),
            }
        };
        assert_eq!(reply(ME), Some(at(ME, 1)));
        assert_eq!(reply(PEER), Some(at(PEER, 1)));
        assert_eq!(reply("stranger"), None);

        let vanish = |secs: u64| {
            let json = format!(r#"{{"seq":1,"ts":1,"text":"x","vanish":{secs}}}"#);
            match wire(&json).admit(PEER, ME).kind {
                Kind::Message { vanish, .. } => vanish,
                kind => panic!("{kind:?}"),
            }
        };
        assert_eq!(vanish(1), Some(*VANISH_SECS.start()));
        assert_eq!(vanish(60), Some(60));
        assert_eq!(vanish(u64::MAX), Some(*VANISH_SECS.end()));

        let bad_file = r#"{"seq":1,"ts":1,"text":"x",
            "file":{"hash":"../x","name":"a","size":1,"mime":"image/png"}}"#;
        let kind = wire(bad_file).admit(PEER, ME).kind;
        assert_eq!(kind, Kind::Message { reply_to: None, vanish: None, file: None });
    }

    #[test]
    fn file_details_are_checked() {
        let good = FileInfo {
            hash: "0f".repeat(32),
            name: "  pic.png ".into(),
            size: 10,
            mime: "Image/PNG".into(),
            width: Some(4),
            height: Some(3),
            preview: Some("data:image/jpeg;base64,AAAA".into()),
        };
        let checked = checked_file(good.clone()).unwrap();
        assert_eq!((checked.name.as_str(), checked.mime.as_str()), ("pic.png", "image/png"));
        assert_eq!(checked.preview, good.preview);

        let bad_hash = FileInfo { hash: "../x".into(), ..good.clone() };
        assert!(checked_file(bad_hash).is_none());
        assert!(checked_file(FileInfo { hash: "0F".repeat(32), ..good.clone() }).is_none());
        assert!(checked_file(FileInfo { size: MAX_FILE_BYTES + 1, ..good.clone() }).is_none());
        assert!(checked_file(FileInfo { size: 0, ..good.clone() }).is_none());

        let odd = checked_file(FileInfo {
            mime: "text/html; charset=utf-8".into(),
            width: Some(0),
            preview: Some("data:image/svg+xml;base64,AAAA".into()),
            ..good.clone()
        })
        .unwrap();
        assert_eq!(odd.mime, "application/octet-stream");
        assert_eq!((odd.width, odd.height, odd.preview), (None, None, None));
        let script = Some("data:image/jpeg;base64,<script>".into());
        assert!(checked_file(FileInfo { preview: script, ..good }).unwrap().preview.is_none());
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
