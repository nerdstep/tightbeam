//! The chat engine: identity, local discovery, pairing, and syncing messages with the
//! one paired peer. Knows nothing about Tauri; it reports changes through `Emit`.
//!
//! Wire protocol (ALPN `tightbeam/1`): one bidirectional QUIC stream per connection,
//! carrying length-prefixed JSON `Frame`s. A connection starts with either
//! `PairRequest` (unpaired) or `Hello` (paired). After both sides say `Hello`, each
//! sends the log entries the other is missing (messages and reactions), then live
//! entries, acks, seen receipts, typing and nudges.
//!
//! Files travel on streams of their own, so a big one never holds up messages. A `Msg`
//! carries the file's hash; once it arrives, the receiver opens a stream to the sender,
//! asks for the file from the first byte it lacks (`FileRequest`), and gets a
//! `FileReply` followed by the bytes. It checks the whole file against the hash before
//! keeping it. A transfer that breaks off resumes there next session.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{EndpointId, SecretKey};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{Notify, mpsc, oneshot},
    time::timeout,
};

use crate::{
    clock::Clock,
    db::{Cursor, Db, FileState, Hit, Message, Page},
    entry::{
        Entry, FileInfo, Kind, MAX_EMOJI_CHARS, MAX_FILE_BYTES, MAX_TEXT_CHARS, Ref, VANISH_SECS,
        Wire, checked_file, clean, is_hash,
    },
    net::{Accepting, Conn, Discoveries, Discovery, Network, RecvStream, SendStream},
};

const MAX_FRAME: usize = 64 * 1024;
const MAX_NAME_CHARS: usize = 40;
const PAIR_PROMPT_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REDIAL_EVERY: Duration = Duration::from_secs(3);
/// At most one nudge this often, each way.
const NUDGE_EVERY: Duration = Duration::from_secs(3);
/// This build's version, sent in `Hello`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Search shows at most this many matches, the newest.
pub const SEARCH_LIMIT: usize = 100;
/// An away note is a few words, like "lunch" or "back at 3".
const MAX_NOTE_CHARS: usize = 100;
/// What this build can do beyond 0.6, said in `Hello`.
const FEATURES: &[&str] = &[FILES];
/// Feature: sends and receives files.
const FILES: &str = "files";
/// How long to wait for the start of a file before trying another.
const FILE_REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// Transfer progress goes to the UI at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);

/// Compatibility rules, since the two peers often run different builds:
/// - New frame types are fine: older peers read them as `Unknown` and skip them.
/// - New fields on existing frames must be `#[serde(default)]`; older peers ignore them.
/// - Never change the meaning or type of an existing field. Add a new one instead.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Frame {
    PairRequest {
        name: String,
    },
    PairAccept {
        name: String,
    },
    PairReject,
    /// `have` = the highest sequence number I hold from you.
    Hello {
        name: String,
        have: u64,
        /// The sender's app version. Added in 0.6: a peer that sends it also handles
        /// vanishing messages and `Dnd`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        /// What the sender's build can do, such as `files`. Added in 0.7.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        features: Vec<String>,
    },
    /// An entry in the sender's log (see `entry::Wire`).
    Msg(Wire),
    Ack {
        have: u64,
    },
    /// `up_to` = the highest of your sequence numbers I've seen. Sent at the start of
    /// each session and whenever it grows. Added in 0.3; older peers skip it.
    Seen {
        up_to: u64,
    },
    /// Get the other person's attention. Live only, never stored. Added in 0.3; older
    /// peers skip it.
    Nudge,
    /// The sender is typing. Live only, repeated every few seconds while they type; the
    /// receiver lets it lapse after a few seconds without one. Added in 0.4.
    Typing,
    /// The sender is away: idle for a while, or by `/away`, with an optional note. Sent
    /// when it starts and at the start of each session; lasts until `Back` or the
    /// session ends. Added in 0.5.
    Away {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// The sender is no longer away. Added in 0.5.
    Back,
    /// The sender turned do not disturb on until `until` (ms, their clock), or off with
    /// `None`. Sent on change and at the start of each session. Added in 0.6.
    Dnd {
        #[serde(default)]
        until: Option<i64>,
    },
    /// A frame type from a newer version. Never sent.
    #[serde(other, skip_serializing)]
    Unknown,
}

/// Opens a file stream: send me this file, starting at byte `from`.
#[derive(Debug, Serialize, Deserialize)]
struct FileRequest {
    hash: String,
    from: u64,
}

/// The answer on a file stream. With `found`, the bytes from `from` to the end follow.
#[derive(Debug, Serialize, Deserialize)]
struct FileReply {
    found: bool,
}

/// What the UI says about a file it sends. The bytes come separately.
#[derive(Debug, Deserialize)]
pub struct NewFile {
    pub name: String,
    pub mime: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub preview: Option<String>,
    /// The caption.
    #[serde(default)]
    pub text: String,
    pub reply_to: Option<Ref>,
}

/// Everything the UI needs except the message list.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub me: Person,
    pub peer: Option<Person>,
    pub connected: bool,
    /// Highest of my sequence numbers the peer has confirmed. Mine at or below are delivered.
    pub peer_has: u64,
    /// Highest of my sequence numbers the peer has seen. Mine at or below are seen.
    pub peer_seen: u64,
    pub nearby: Vec<Person>,
    /// Everyone I've been paired with, by their last known name, to label their messages
    /// (the peer's, and anyone's from before).
    pub people: Vec<Person>,
    pub pairing: Option<PairPrompt>,
    pub notice: Option<String>,
    /// Set when I went away with `/away`. Away from being idle isn't shown: by the time I
    /// could see it, I'm back.
    pub away: Option<Away>,
    /// The peer is away. Only while connected.
    pub peer_away: Option<Away>,
    /// Do not disturb is on until then (ms). `None` once it's past.
    pub dnd_until: Option<i64>,
    /// The peer's do not disturb, by their clock. Only while connected.
    pub peer_dnd_until: Option<i64>,
}

/// For `/status`: how the connection is doing, and which versions are talking.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionInfo {
    pub my_version: String,
    /// `None` for peers before 0.6, or when we haven't been connected since.
    pub peer_version: Option<String>,
    /// When this session started (ms). The rest are also only while connected.
    pub since: Option<i64>,
    /// The peer's address on the path in use.
    pub address: Option<String>,
    pub rtt_ms: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Away {
    /// What they said with `/away`, if anything.
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Person {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairPrompt {
    pub id: String,
    pub name: String,
    /// Six digits both screens show; matching codes mean nobody is in the middle.
    pub code: String,
    /// True when the other person asked us.
    pub incoming: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Event {
    Status(Status),
    Message(Message),
    /// The peer nudged us.
    Nudge,
    /// A message changed: its reactions, an edit, or a delete.
    Updated(Message),
    /// The peer is typing.
    Typing,
    /// A file is on its way, to or from here: `have` of `size` bytes so far. The last one
    /// for a transfer has `have == size`.
    Transfer {
        hash: String,
        have: u64,
        size: u64,
    },
}

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

struct Pairing {
    prompt: PairPrompt,
    /// Incoming request: where the user's yes/no goes.
    decide: Option<oneshot::Sender<bool>>,
    /// Outgoing request: lets the user cancel.
    task: Option<tokio::task::AbortHandle>,
}

struct Session {
    generation: u64,
    since: i64,
    out: mpsc::UnboundedSender<Frame>,
    conn: Conn,
}

struct State {
    name: String,
    peer: Option<(EndpointId, String)>,
    peer_has: u64,
    peer_seen: u64,
    /// Highest of the peer's sequence numbers I've seen; what our `Seen` frames say.
    seen: u64,
    /// My highest sequence number when I paired with this peer: they get only what I
    /// write after it (see `shares`). 0 for pairings from before 0.8.
    shared_after: u64,
    nearby: BTreeMap<String, String>,
    /// See `Status::people`: id to name.
    people: BTreeMap<String, String>,
    pairing: Option<Pairing>,
    session: Option<Session>,
    next_generation: u64,
    notice: Option<String>,
    /// tokio's clock, so tests can move past the limit.
    nudge_sent: Option<tokio::time::Instant>,
    nudge_received: Option<tokio::time::Instant>,
    away: Option<Away>,
    /// `away` came from `/away`, so it lasts until `/back` or a message, not until input.
    away_by_hand: bool,
    peer_away: Option<Away>,
    /// From their last `Hello`, kept between sessions.
    peer_version: Option<String>,
    peer_features: Vec<String>,
    dnd_until: Option<i64>,
    peer_dnd_until: Option<i64>,
}

pub struct Core {
    net: Network,
    me: EndpointId,
    db: Mutex<Db>,
    /// Wall-clock time, shared with `db`.
    clock: Clock,
    /// Where files are kept: `<hash>/<name>`, plus `<hash>.part` while one arrives.
    files_dir: PathBuf,
    /// Wakes the session's downloader when a message with a file arrives.
    files_wanted: Notify,
    state: Mutex<State>,
    emit: Emit,
    /// Discovery, accept and dial loops; stopped on shutdown so nothing keeps
    /// advertising a closed endpoint.
    tasks: Mutex<Vec<tokio::task::AbortHandle>>,
    /// The runtime we started on. UI actions arrive on Tauri's main thread, which isn't
    /// inside it, so background work is always spawned through this handle.
    rt: tokio::runtime::Handle,
}

impl Core {
    pub async fn start(data_dir: PathBuf, emit: Emit) -> Result<Arc<Self>> {
        Self::start_with(data_dir, emit, false).await
    }

    /// `loopback_only` binds to 127.0.0.1, for running two copies on one machine in tests.
    pub async fn start_with(
        data_dir: PathBuf,
        emit: Emit,
        loopback_only: bool,
    ) -> Result<Arc<Self>> {
        let network = |secret| Network::iroh(secret, loopback_only);
        Self::start_on(data_dir, emit, Clock::system(), network).await
    }

    /// Starts on the network `network` makes from this computer's secret key.
    pub(crate) async fn start_on<F: Future<Output = Result<Network>>>(
        data_dir: PathBuf,
        emit: Emit,
        clock: Clock,
        network: impl FnOnce(SecretKey) -> F,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("creating {}", data_dir.display()))?;
        let db = Db::open_with(&data_dir.join("tightbeam.db"), clock.clone())?;

        let secret = match db.get("secret_key")? {
            Some(bytes) => {
                let bytes: [u8; 32] = bytes.try_into().ok().context("corrupt secret key")?;
                SecretKey::from_bytes(&bytes)
            }
            None => {
                let key = SecretKey::generate();
                db.set("secret_key", key.to_bytes())?;
                key
            }
        };
        let name = db.get_str("name")?.unwrap_or_default();
        let peer = match (db.get_str("peer_id")?, db.get_str("peer_name")?) {
            (Some(id), name) => Some((id.parse()?, name.unwrap_or_default())),
            _ => None,
        };
        let counter =
            |key| -> Result<u64> { Ok(db.get_str(key)?.and_then(|s| s.parse().ok()).unwrap_or(0)) };
        let (peer_has, peer_seen, seen, shared_after) = (
            counter("peer_has")?,
            counter("peer_seen")?,
            counter("seen")?,
            counter("shared_after")?,
        );
        let peer_version = db.get_str("peer_version")?;
        let peer_features = features_from(db.get_str("peer_features")?.as_deref());
        let dnd_until = db.get_str("dnd_until")?.and_then(|s| s.parse().ok());
        let people = db.people()?.into_iter().collect();

        let net = network(secret).await?;
        let discoveries = net.discoveries().await;

        let core = Arc::new(Self {
            me: net.id(),
            net,
            db: Mutex::new(db),
            clock,
            files_dir: data_dir.join("files"),
            files_wanted: Notify::new(),
            state: Mutex::new(State {
                name,
                peer,
                peer_has,
                peer_seen,
                seen,
                shared_after,
                nearby: BTreeMap::new(),
                people,
                pairing: None,
                session: None,
                next_generation: 0,
                notice: None,
                nudge_sent: None,
                nudge_received: None,
                away: None,
                away_by_hand: false,
                peer_away: None,
                peer_version,
                peer_features,
                dnd_until,
                peer_dnd_until: None,
            }),
            emit,
            tasks: Mutex::new(Vec::new()),
            rt: tokio::runtime::Handle::current(),
        });
        core.advertise_name();
        core.prune_files(); // anything left over from a delete while closing

        *core.tasks.lock().unwrap() = vec![
            tokio::spawn(core.clone().accept_loop()).abort_handle(),
            tokio::spawn(core.clone().discovery_loop(discoveries)).abort_handle(),
            tokio::spawn(core.clone().dial_loop()).abort_handle(),
            tokio::spawn(core.clone().vanish_loop()).abort_handle(),
        ];
        Ok(core)
    }

    /// Stop background work and close connections cleanly so the peer notices right away.
    pub async fn shutdown(&self) {
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
        self.net.close().await;
    }

    // ---- Queries -------------------------------------------------------------------

    pub fn id(&self) -> String {
        self.me.to_string()
    }

    pub fn status(&self) -> Status {
        let st = self.state();
        Status {
            me: Person { id: self.id(), name: st.name.clone() },
            peer: st
                .peer
                .as_ref()
                .map(|(id, name)| Person { id: id.to_string(), name: name.clone() }),
            connected: st.session.is_some(),
            peer_has: st.peer_has,
            peer_seen: st.peer_seen,
            nearby: st
                .nearby
                .iter()
                .map(|(id, name)| Person { id: id.clone(), name: name.clone() })
                .collect(),
            people: st
                .people
                .iter()
                .map(|(id, name)| Person { id: id.clone(), name: name.clone() })
                .collect(),
            pairing: st.pairing.as_ref().map(|p| p.prompt.clone()),
            notice: st.notice.clone(),
            away: st.away.clone().filter(|_| st.away_by_hand),
            peer_away: st.peer_away.clone(),
            dnd_until: st.dnd_until.filter(|&t| t > self.clock.now_ms()),
            peer_dnd_until: st.peer_dnd_until,
        }
    }

    /// For `/status`.
    pub fn connection_info(&self) -> ConnectionInfo {
        let st = self.state();
        let (mut since, mut address, mut rtt_ms) = (None, None, None);
        if let Some(session) = &st.session {
            since = Some(session.since);
            if let Some(route) = session.conn.route() {
                (address, rtt_ms) = (Some(route.address), Some(route.rtt_ms));
            }
        }
        ConnectionInfo {
            my_version: VERSION.into(),
            peer_version: st.peer_version.clone(),
            since,
            address,
            rtt_ms,
        }
    }

    /// Whether my do not disturb is on: no notifications, and nudges are refused.
    pub fn dnd(&self) -> bool {
        self.state().dnd_until.is_some_and(|t| t > self.clock.now_ms())
    }

    /// The whole history, oldest first. The UI uses `page` instead.
    pub fn messages(&self) -> Result<Vec<Message>> {
        self.db().all()
    }

    pub fn page(&self, before: Option<&Cursor>, limit: usize) -> Result<Page> {
        self.db().page(before, limit)
    }

    /// Messages from `from` up to `before`, in display order.
    pub fn range(&self, from: &Cursor, before: &Cursor) -> Result<Vec<Message>> {
        self.db().range(from, before)
    }

    /// Messages matching `query`, newest first, at most `SEARCH_LIMIT`.
    pub fn search(&self, query: &str) -> Result<Vec<Hit>> {
        self.db().search(query, SEARCH_LIMIT)
    }

    // ---- Actions from the UI -------------------------------------------------------

    pub fn set_name(&self, name: &str) -> Result<()> {
        let name = clean(name, MAX_NAME_CHARS);
        ensure!(!name.is_empty(), "name can't be empty");
        self.db().set("name", &name)?;
        let session = {
            let mut st = self.state();
            st.name = name.clone();
            st.session.as_ref().map(|s| s.out.clone())
        };
        self.advertise_name();
        if let Some(out) = session {
            let have = self.peer_id().map(|p| self.db().max_seq(&p.to_string())).transpose()?;
            let _ = out.send(hello(name, have.unwrap_or(0)));
        }
        self.changed();
        Ok(())
    }

    /// `reply_to`: the message this one answers.
    /// `vanish`: seconds the message lasts once they've seen it.
    pub fn send_message(
        &self,
        text: &str,
        reply_to: Option<Ref>,
        vanish: Option<u64>,
    ) -> Result<Message> {
        let text = clean(text, MAX_TEXT_CHARS);
        ensure!(!text.is_empty(), "message can't be empty");
        let peer = self.peer_id().context("not paired with anyone yet")?;
        if let Some(to) = &reply_to {
            ensure!(self.is_ours(to, &peer.to_string()), "can't reply to that");
        }
        if let Some(secs) = vanish {
            ensure!(VANISH_SECS.contains(&secs), "A vanishing message lasts 5 seconds to 7 days.");
            let name = title_name(self.peer_name());
            ensure!(
                self.state().peer_version.is_some(),
                "Vanishing messages need {name} on Tightbeam 0.6 or later. If they've \
                 updated, try again once you've been connected."
            );
        }
        self.post(text, Kind::Message { reply_to, vanish, file: None })
    }

    /// Sends a file, with `new.text` as its caption. Blocking: it hashes and stores the
    /// file, so call it off the UI thread.
    pub fn send_file(&self, bytes: &[u8], new: NewFile) -> Result<Message> {
        let peer = self.peer_id().context("not paired with anyone yet")?;
        ensure!(!bytes.is_empty(), "That file is empty.");
        ensure!(bytes.len() as u64 <= MAX_FILE_BYTES, "Files can be up to 1 GB.");
        {
            let st = self.state();
            ensure!(
                st.peer_features.iter().any(|f| f == FILES),
                "Sending pictures needs {} on Tightbeam 0.7 or later. If they've updated, \
                 try again once you've been connected.",
                title_name(st.peer.as_ref().map(|(_, n)| n.clone()))
            );
        }
        if let Some(to) = &new.reply_to {
            ensure!(self.is_ours(to, &peer.to_string()), "can't reply to that");
        }
        let hash = blake3::hash(bytes).to_hex().to_string();
        let info = checked_file(FileInfo {
            hash: hash.clone(),
            name: new.name,
            size: bytes.len() as u64,
            mime: new.mime,
            width: new.width,
            height: new.height,
            preview: new.preview,
        })
        .context("That file can't be sent.")?;
        if self.db().file_state(&hash)? != FileState::Done {
            let name = safe_name(&info.name);
            let dir = self.files_dir.join(&hash);
            std::fs::create_dir_all(&dir)?;
            let part = self.files_dir.join(format!("{hash}.part"));
            std::fs::write(&part, bytes)?;
            std::fs::rename(&part, dir.join(&name))?;
            self.db().set_file(&hash, &name, FileState::Done)?;
        }
        let text = clean(&new.text, MAX_TEXT_CHARS);
        self.post(text, Kind::Message { reply_to: new.reply_to, vanish: None, file: Some(info) })
    }

    /// A file that's here and attached to a message: where it is, and its type.
    ///
    /// Someone may move, delete or edit files in the data folder. One that's gone, or
    /// whose size changed, counts as not here: it goes back to waiting, and the
    /// downloader gets it again from the peer.
    pub fn file(&self, hash: &str) -> Result<Option<(PathBuf, String)>> {
        if !is_hash(hash) {
            return Ok(None);
        }
        let Some(stored) = self.db().stored_file(hash)? else { return Ok(None) };
        let path = self.files_dir.join(hash).join(&stored.name);
        let intact = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == stored.size);
        if !intact {
            self.lost_file(hash)?;
            return Ok(None);
        }
        Ok(Some((path, stored.mime)))
    }

    /// A file went missing or changed outside the app: wait for it again, from the peer.
    /// If they don't have it either, it ends up gone.
    fn lost_file(&self, hash: &str) -> Result<()> {
        self.db().forget_file(hash)?;
        self.file_changed(hash)?;
        self.files_wanted.notify_one();
        Ok(())
    }

    /// Adds a message of mine to the log, shows it, and sends it.
    fn post(&self, text: String, kind: Kind) -> Result<Message> {
        let entry = self.append(text, kind)?;
        let at = Ref { author: entry.author.clone(), seq: entry.seq };
        let msg = self.db().message(&at)?.context("message vanished")?;
        (self.emit)(Event::Message(msg.clone()));
        self.send_live(entry);
        self.back(); // writing a message means I'm here
        Ok(msg)
    }

    /// Sets my reaction to a message, replacing any earlier one. `emoji` "" takes it back.
    pub fn react(&self, to: Ref, emoji: &str) -> Result<()> {
        let emoji = clean(emoji, MAX_EMOJI_CHARS);
        let peer = self.peer_id().context("not paired with anyone yet")?;
        ensure!(self.is_ours(&to, &peer.to_string()), "can't react to that");
        let entry = self.append(emoji, Kind::Reaction { to: to.clone() })?;
        self.updated(&to)?;
        self.send_live(entry);
        Ok(())
    }

    /// Replaces the text of one of my messages: a file's caption can be empty.
    pub fn edit(&self, of: Ref, text: &str) -> Result<()> {
        let text = clean(text, MAX_TEXT_CHARS);
        let current = self.my_message(&of)?;
        ensure!(!text.is_empty() || current.file.is_some(), "message can't be empty");
        if text == current.text {
            return Ok(());
        }
        let entry = self.append(text, Kind::Edit { of: of.clone() })?;
        self.updated(&of)?;
        self.send_live(entry);
        Ok(())
    }

    /// Deletes one of my messages, here and for the peer. Its text is blanked on both
    /// sides, not just hidden.
    pub fn delete(&self, of: Ref) -> Result<()> {
        self.my_message(&of)?;
        let entry = self.append(String::new(), Kind::Delete { of: of.clone() })?;
        self.db().scrub(&of)?;
        self.updated(&of)?;
        self.send_live(entry);
        self.prune_files();
        Ok(())
    }

    /// `/dnd`: no notifications until `until` (ms), and the peer sees it. `None` ends it.
    pub fn set_dnd(&self, until: Option<i64>) -> Result<()> {
        let until = until.filter(|&t| t > self.clock.now_ms());
        match until {
            Some(t) => self.db().set("dnd_until", t.to_string())?,
            None => self.db().delete("dnd_until")?,
        }
        self.state().dnd_until = until;
        self.send_frame(Frame::Dnd { until });
        self.changed();
        Ok(())
    }

    /// I'm typing. The UI calls this every few seconds while I type.
    pub fn typing(&self) {
        self.send_frame(Frame::Typing);
    }

    /// `/away`: away until `back`, or until I send a message. `note` says why, if anything.
    pub fn set_away(&self, note: &str) {
        let note = clean(&note.split_whitespace().collect::<Vec<_>>().join(" "), MAX_NOTE_CHARS);
        self.go_away(Away { note: Some(note).filter(|n| !n.is_empty()) }, true);
    }

    /// No longer away, whether that was from `/away` or from being idle.
    pub fn back(&self) {
        let was = {
            let mut st = self.state();
            st.away_by_hand = false;
            st.away.take()
        };
        if was.is_some() {
            self.send_frame(Frame::Back);
            self.changed();
        }
    }

    /// Whether the computer has gone unused for a while. Being idle makes me away, and
    /// input brings me back, except while I'm away by `/away`.
    pub fn set_idle(&self, idle: bool) {
        let (away, by_hand) = {
            let st = self.state();
            (st.away.is_some(), st.away_by_hand)
        };
        if idle && !away {
            self.go_away(Away::default(), false);
        } else if !idle && away && !by_hand {
            self.back();
        }
    }

    /// The person is looking at the chat: everything received so far counts as seen.
    /// Cheap when nothing changed, so call it freely.
    pub fn mark_seen(&self) -> Result<()> {
        let Some(peer) = self.peer_id() else { return Ok(()) };
        let up_to = self.db().max_seq(&peer.to_string())?;
        let out = {
            let mut st = self.state();
            if up_to <= st.seen {
                return Ok(());
            }
            st.seen = up_to;
            st.session.as_ref().map(|s| s.out.clone())
        };
        self.db().set("seen", up_to.to_string())?;
        if let Some(out) = out {
            let _ = out.send(Frame::Seen { up_to });
        }
        self.start_vanish(&peer.to_string(), up_to)
    }

    /// Erases the conversation on this computer; the peer keeps their copy. My messages
    /// they haven't received yet stay, so they still get delivered. Sequence numbers
    /// carry on as before, so sync never asks for the erased messages again.
    pub fn clear_history(&self) -> Result<()> {
        let keep_after = self.peer_id().map(|_| self.state().peer_has);
        self.db().clear(&self.id(), keep_after)?;
        self.prune_files();
        Ok(())
    }

    /// Shakes the peer's window and plays a sound there. Only while connected.
    pub fn nudge(&self) -> Result<()> {
        let out = {
            let mut st = self.state();
            let out = st
                .session
                .as_ref()
                .map(|s| s.out.clone())
                .context("Not connected, so the nudge can't reach them.")?;
            ensure!(
                !st.peer_dnd_until.is_some_and(|t| t > self.clock.now_ms()),
                "{} has do not disturb on, so nudges are paused.",
                title_name(st.peer.as_ref().map(|(_, n)| n.clone()))
            );
            ensure!(
                !st.nudge_sent.is_some_and(|t| t.elapsed() < NUDGE_EVERY),
                "You just nudged. Give it a few seconds."
            );
            st.nudge_sent = Some(tokio::time::Instant::now());
            out
        };
        let _ = out.send(Frame::Nudge);
        Ok(())
    }

    /// Ask a nearby person to pair. Returns immediately; progress shows up in `status`.
    pub fn pair_with(self: &Arc<Self>, id: &str) -> Result<()> {
        let remote: EndpointId = id.parse().context("bad id")?;
        {
            let mut st = self.state();
            ensure!(!st.name.is_empty(), "set your name first");
            ensure!(st.peer.is_none(), "already paired");
            ensure!(st.pairing.is_none(), "already pairing");
            let core = self.clone();
            let task = self.rt.spawn(async move {
                if let Err(e) = core.request_pairing(remote).await {
                    core.set_notice(format!("Pairing failed: {e:#}"));
                }
                core.clear_pairing(remote);
            });
            st.pairing = Some(Pairing {
                prompt: PairPrompt {
                    id: remote.to_string(),
                    name: st.nearby.get(id).cloned().unwrap_or_default(),
                    code: pair_code(self.me, remote),
                    incoming: false,
                },
                decide: None,
                task: Some(task.abort_handle()),
            });
            st.notice = None;
        }
        self.changed();
        Ok(())
    }

    /// Yes/no on an incoming request; `false` also cancels an outgoing one.
    pub fn answer_pair(&self, accept: bool) {
        {
            let mut st = self.state();
            let Some(p) = st.pairing.as_mut() else { return };
            if let Some(decide) = p.decide.take() {
                let _ = decide.send(accept);
            } else if accept {
                return; // can't "accept" our own outgoing request
            } else if let Some(task) = p.task.take() {
                task.abort();
            }
            st.pairing = None;
        }
        self.changed();
    }

    /// Forget the peer. History stays in the database.
    pub fn unpair(&self) -> Result<()> {
        {
            let db = self.db();
            db.delete("peer_id")?;
            db.delete("peer_name")?;
            db.delete("peer_has")?;
            db.delete("peer_seen")?;
            db.delete("seen")?;
            db.delete("peer_version")?;
            db.delete("peer_features")?;
            db.delete("shared_after")?;
        }
        let session = {
            let mut st = self.state();
            st.peer = None;
            st.peer_has = 0;
            st.peer_seen = 0;
            st.seen = 0;
            st.shared_after = 0;
            st.peer_version = None;
            st.peer_features.clear();
            st.session.take()
        };
        if let Some(s) = session {
            s.conn.close(0, b"unpaired");
        }
        self.changed();
        Ok(())
    }

    pub fn dismiss_notice(&self) {
        self.state().notice = None;
        self.changed();
    }

    /// An app preference stored alongside the chat data (e.g. notifications on/off).
    pub fn pref(&self, key: &str) -> Result<Option<String>> {
        self.db().get_str(&format!("pref_{key}"))
    }

    pub fn set_pref(&self, key: &str, value: &str) -> Result<()> {
        self.db().set(&format!("pref_{key}"), value)
    }

    /// The peer's display name, if paired.
    pub fn peer_name(&self) -> Option<String> {
        self.state().peer.as_ref().map(|(_, name)| name.clone())
    }

    // ---- Background tasks ----------------------------------------------------------

    async fn accept_loop(self: Arc<Self>) {
        while let Some(accepting) = self.net.accept().await {
            let core = self.clone();
            tokio::spawn(async move {
                if let Err(e) = core.handle_incoming(accepting).await {
                    eprintln!("incoming connection: {e:#}");
                }
            });
        }
    }

    async fn discovery_loop(self: Arc<Self>, mut discoveries: Discoveries) {
        while let Some(event) = discoveries.next().await {
            {
                let mut st = self.state();
                match event {
                    Discovery::Found { id, .. } if id == self.me => continue,
                    Discovery::Found { id, name } => {
                        st.nearby.insert(id.to_string(), name);
                    }
                    Discovery::Lost(id) => {
                        st.nearby.remove(&id.to_string());
                    }
                }
            }
            self.changed();
        }
    }

    /// Once paired, keep a connection up. Only the side with the smaller id dials, so the
    /// two apps never race to open two connections.
    async fn dial_loop(self: Arc<Self>) {
        loop {
            tokio::time::sleep(REDIAL_EVERY).await;
            let target = {
                let st = self.state();
                match &st.peer {
                    Some((peer, _))
                        if st.session.is_none()
                            && st.pairing.is_none()
                            && self.me.as_bytes() < peer.as_bytes() =>
                    {
                        Some(*peer)
                    }
                    _ => None,
                }
            };
            let Some(peer) = target else { continue };
            let started = std::time::Instant::now();
            let conn = match timeout(CONNECT_TIMEOUT, self.net.connect(peer)).await {
                Ok(Ok(conn)) => conn,
                Ok(Err(e)) => {
                    eprintln!(
                        "[{}] dial failed after {:?}: {e:#}",
                        self.me.fmt_short(),
                        started.elapsed()
                    );
                    continue;
                }
                Err(_) => {
                    eprintln!(
                        "[{}] dial timed out after {:?}",
                        self.me.fmt_short(),
                        started.elapsed()
                    );
                    continue;
                }
            };
            let result = async {
                let (send, recv) = conn.open_bi().await?;
                self.run_session(conn, send, recv, None).await
            }
            .await;
            if let Err(e) = result {
                eprintln!("session ended: {e:#}");
            }
        }
    }

    // ---- Connection handling -------------------------------------------------------

    async fn handle_incoming(self: Arc<Self>, accepting: Accepting) -> Result<()> {
        let conn = accepting.connected().await?;
        let remote = conn.remote_id();
        let (send, mut recv) = conn.accept_bi().await?;
        match read_frame(&mut recv).await? {
            Frame::PairRequest { name } => self.answer_pair_request(conn, send, recv, name).await,
            Frame::Hello { name, have, version, features } => {
                if self.peer_id() != Some(remote) {
                    conn.close(1, b"not paired");
                    return Ok(());
                }
                let theirs = TheirHello { name, have, version, features };
                self.run_session(conn, send, recv, Some(theirs)).await
            }
            other => bail!("unexpected first frame: {other:?}"),
        }
    }

    async fn answer_pair_request(
        self: Arc<Self>,
        conn: Conn,
        mut send: SendStream,
        recv: RecvStream,
        their_name: String,
    ) -> Result<()> {
        let remote = conn.remote_id();
        let their_name = clean(&their_name, MAX_NAME_CHARS);

        enum Decision {
            Accept,
            Reject,
            Ask(oneshot::Receiver<bool>),
        }
        let decision = {
            let mut st = self.state();
            match &st.peer {
                // They reinstalled or reset; we already trust this key.
                Some((peer, _)) if *peer == remote => Decision::Accept,
                Some(_) => Decision::Reject,
                None if st.pairing.is_some() => Decision::Reject,
                None => {
                    let (tx, rx) = oneshot::channel();
                    st.pairing = Some(Pairing {
                        prompt: PairPrompt {
                            id: remote.to_string(),
                            name: their_name.clone(),
                            code: pair_code(self.me, remote),
                            incoming: true,
                        },
                        decide: Some(tx),
                        task: None,
                    });
                    Decision::Ask(rx)
                }
            }
        };
        let accepted = match decision {
            Decision::Accept => true,
            Decision::Reject => false,
            Decision::Ask(rx) => {
                self.changed();
                let accepted = tokio::select! {
                    answer = rx => answer.unwrap_or(false),
                    _ = conn.closed() => false,
                    _ = tokio::time::sleep(PAIR_PROMPT_TIMEOUT) => false,
                };
                self.clear_pairing(remote);
                accepted
            }
        };

        if !accepted {
            write_frame(&mut send, &Frame::PairReject).await?;
            send.finish()?;
            let _ = timeout(Duration::from_secs(2), conn.closed()).await;
            return Ok(());
        }
        self.save_peer(remote, &their_name)?;
        write_frame(&mut send, &Frame::PairAccept { name: self.name() }).await?;
        self.run_session(conn, send, recv, None).await
    }

    async fn request_pairing(self: &Arc<Self>, remote: EndpointId) -> Result<()> {
        let conn = timeout(CONNECT_TIMEOUT, self.net.connect(remote))
            .await
            .context("timed out connecting")??;
        let (mut send, mut recv) = conn.open_bi().await?;
        write_frame(&mut send, &Frame::PairRequest { name: self.name() }).await?;
        let reply = timeout(PAIR_PROMPT_TIMEOUT + Duration::from_secs(10), read_frame(&mut recv))
            .await
            .context("no answer")??;
        match reply {
            Frame::PairAccept { name } => {
                self.save_peer(remote, &name)?;
                self.clear_pairing(remote);
                self.run_session(conn, send, recv, None).await
            }
            Frame::PairReject => {
                self.set_notice("They declined the pairing request.".into());
                Ok(())
            }
            other => bail!("unexpected reply: {other:?}"),
        }
    }

    /// Runs one connected session with the paired peer until the connection drops.
    async fn run_session(
        self: &Arc<Self>,
        conn: Conn,
        mut send: SendStream,
        mut recv: RecvStream,
        their_hello: Option<TheirHello>,
    ) -> Result<()> {
        let peer = self.peer_id().context("not paired")?;
        ensure!(conn.remote_id() == peer, "connection is not from our peer");
        let peer_str = peer.to_string();

        let have = self.db().max_seq(&peer_str)?;
        write_frame(&mut send, &hello(self.name(), have)).await?;
        let theirs = match their_hello {
            Some(hello) => hello,
            None => match read_frame(&mut recv).await? {
                Frame::Hello { name, have, version, features } => {
                    TheirHello { name, have, version, features }
                }
                other => bail!("expected hello, got {other:?}"),
            },
        };
        self.update_peer_name(&theirs.name)?;
        self.set_peer_build(theirs.version, theirs.features)?;
        let their_have = theirs.have;
        self.set_peer_has(their_have, true)?;

        // Register the session before reading the backlog, so a message sent in between
        // goes out live. Worst case it's sent twice, and the receiver ignores duplicates.
        let (out, mut rx) = mpsc::unbounded_channel();
        let generation = {
            let mut st = self.state();
            st.next_generation += 1;
            let generation = st.next_generation;
            let session = Session {
                generation,
                since: self.clock.now_ms(),
                out: out.clone(),
                conn: conn.clone(),
            };
            let old = st.session.replace(session);
            // They say so again if they still are.
            st.peer_away = None;
            st.peer_dnd_until = None;
            if let Some(old) = old {
                old.conn.close(0, b"replaced");
            }
            generation
        };
        let shared_after = self.state().shared_after;
        let mine = self.db().since(&self.id(), their_have.max(shared_after))?;
        let shared = mine.into_iter().filter(|e| shares(e, shared_after, &peer_str));
        for wire in shared.filter_map(Entry::into_wire) {
            let _ = out.send(Frame::Msg(wire));
        }
        // Covers anything seen while we were apart.
        let _ = out.send(Frame::Seen { up_to: self.state().seen });
        if let Some(Away { note }) = self.state().away.clone() {
            let _ = out.send(Frame::Away { note });
        }
        if let Some(until) = self.state().dnd_until.filter(|&t| t > self.clock.now_ms()) {
            let _ = out.send(Frame::Dnd { until: Some(until) });
        }
        self.changed();

        let writer = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                if write_frame(&mut send, &frame).await.is_err() {
                    break;
                }
            }
        });
        // Files go on streams of their own: the ones they ask me for, and the ones I want.
        let serving = tokio::spawn(self.clone().serve_files(conn.clone()));
        let fetching = tokio::spawn(self.clone().fetch_files(conn.clone()));
        // Stopped however this ends, including when the session itself is dropped (the
        // dialer's, at shutdown), so none of them outlives it.
        let tasks = [writer, serving, fetching].map(|task| AbortOnDrop(task.abort_handle()));
        let result = self.read_loop(&mut recv, &peer_str, &out).await;
        drop(tasks);
        conn.close(0, b"bye");
        {
            let mut st = self.state();
            if st.session.as_ref().is_some_and(|s| s.generation == generation) {
                st.session = None;
                st.peer_away = None;
                st.peer_dnd_until = None;
            }
        }
        self.changed();
        result
    }

    async fn read_loop(
        &self,
        recv: &mut RecvStream,
        peer: &str,
        out: &mpsc::UnboundedSender<Frame>,
    ) -> Result<()> {
        loop {
            match read_frame(recv).await? {
                Frame::Msg(wire) => {
                    let entry = wire.admit(peer, &self.id());
                    let (is_new, have) = {
                        let db = self.db();
                        (db.insert(&entry)?, db.max_seq(peer)?)
                    };
                    if is_new {
                        self.announce(entry)?;
                    }
                    let _ = out.send(Frame::Ack { have });
                }
                Frame::Typing => (self.emit)(Event::Typing),
                Frame::Ack { have } => self.set_peer_has(have, false)?,
                Frame::Seen { up_to } => self.set_peer_seen(up_to)?,
                Frame::Nudge => self.nudged(),
                Frame::Away { note } => {
                    let note = note.map(|n| clean(&n, MAX_NOTE_CHARS)).filter(|n| !n.is_empty());
                    self.set_peer_away(Some(Away { note }));
                }
                Frame::Back => self.set_peer_away(None),
                Frame::Dnd { until } => {
                    self.state().peer_dnd_until = until;
                    self.changed();
                }
                Frame::Hello { name, .. } => self.update_peer_name(&name)?,
                // From a newer version of the app: skip it rather than drop the session.
                Frame::Unknown => {}
                other => bail!("unexpected frame mid-session: {other:?}"),
            }
        }
    }

    // ---- Helpers -------------------------------------------------------------------

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    fn db(&self) -> MutexGuard<'_, Db> {
        self.db.lock().unwrap()
    }

    fn changed(&self) {
        (self.emit)(Event::Status(self.status()));
    }

    fn name(&self) -> String {
        self.state().name.clone()
    }

    fn peer_id(&self) -> Option<EndpointId> {
        self.state().peer.as_ref().map(|(id, _)| *id)
    }

    fn advertise_name(&self) {
        self.net.advertise(&self.name());
    }

    /// Pairs with `id`. Someone new gets only what I write from now on; pairing again
    /// with the same key carries on as before.
    fn save_peer(&self, id: EndpointId, name: &str) -> Result<()> {
        let known = self.peer_id() == Some(id);
        let shared_after = {
            let db = self.db();
            db.set("peer_id", id.to_string())?;
            db.set("peer_name", name)?;
            db.set_person(&id.to_string(), name)?;
            let shared_after = if known { None } else { Some(db.max_seq(&self.id())?) };
            if let Some(after) = shared_after {
                db.set("shared_after", after.to_string())?;
            }
            shared_after
        };
        {
            let mut st = self.state();
            st.peer = Some((id, name.to_string()));
            st.people.insert(id.to_string(), name.to_string());
            if let Some(after) = shared_after {
                st.shared_after = after;
            }
        }
        self.changed();
        Ok(())
    }

    fn update_peer_name(&self, name: &str) -> Result<()> {
        let name = clean(name, MAX_NAME_CHARS);
        let mut st = self.state();
        if let Some((id, current)) = &mut st.peer
            && *current != name
        {
            *current = name.clone();
            let id = id.to_string();
            st.people.insert(id.clone(), name.clone());
            drop(st);
            {
                let db = self.db();
                db.set("peer_name", &name)?;
                db.set_person(&id, &name)?;
            }
            self.changed();
        }
        Ok(())
    }

    /// `exact` on hello (they may have lost history); otherwise only ever increases.
    fn set_peer_has(&self, have: u64, exact: bool) -> Result<()> {
        let (new, seen) = {
            let mut st = self.state();
            let new = if exact { have } else { st.peer_has.max(have) };
            // They can't have seen what they no longer have.
            let seen = st.peer_seen.min(new);
            if new == st.peer_has && seen == st.peer_seen {
                return Ok(());
            }
            st.peer_has = new;
            st.peer_seen = seen;
            (new, seen)
        };
        {
            let db = self.db();
            db.set("peer_has", new.to_string())?;
            db.set("peer_seen", seen.to_string())?;
        }
        self.changed();
        Ok(())
    }

    /// Only ever increases; `set_peer_has` lowers it if they lost history.
    fn set_peer_seen(&self, up_to: u64) -> Result<()> {
        {
            let mut st = self.state();
            if up_to <= st.peer_seen {
                return Ok(());
            }
            st.peer_seen = up_to;
        }
        self.db().set("peer_seen", up_to.to_string())?;
        self.changed();
        self.start_vanish(&self.id(), up_to)
    }

    /// `author`'s vanishing messages up to `up_to` have been seen: start their clocks.
    fn start_vanish(&self, author: &str, up_to: u64) -> Result<()> {
        let started = self.db().start_vanish(author, up_to, self.clock.now_ms())?;
        for at in started {
            self.updated(&at)?;
        }
        Ok(())
    }

    /// Adds an entry to my log.
    fn append(&self, text: String, kind: Kind) -> Result<Entry> {
        let db = self.db();
        let (seq, ts) = (db.max_seq(&self.id())? + 1, self.clock.now_ms());
        let entry = Entry { author: self.id(), seq, ts, hlc: db.next_hlc(ts)?, text, kind };
        db.insert(&entry)?;
        Ok(entry)
    }

    /// Records the peer's version and features from their `Hello`.
    fn set_peer_build(&self, version: Option<String>, features: Vec<String>) -> Result<()> {
        let features: Vec<String> = features
            .into_iter()
            .filter(|f| FEATURES.contains(&f.as_str()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        {
            let st = self.state();
            if st.peer_version == version && st.peer_features == features {
                return Ok(());
            }
        }
        {
            let db = self.db();
            match &version {
                Some(v) => db.set("peer_version", v)?,
                None => db.delete("peer_version")?, // they went back to an older build
            }
            db.set("peer_features", features.join(","))?;
        }
        let mut st = self.state();
        st.peer_version = version;
        st.peer_features = features;
        Ok(())
    }

    // ---- Files ---------------------------------------------------------------------

    /// Answers the peer's file requests for this session.
    async fn serve_files(self: Arc<Self>, conn: Conn) {
        while let Ok((send, recv)) = conn.accept_bi().await {
            let core = self.clone();
            tokio::spawn(async move {
                if let Err(e) = core.serve_file(send, recv).await {
                    eprintln!("sending a file: {e:#}");
                }
            });
        }
    }

    /// Sends one file, from where they asked. Only files that are here, still attached
    /// to a message, and unchanged: one edited in place is treated as lost, never sent as
    /// the original.
    async fn serve_file(&self, mut send: SendStream, mut recv: RecvStream) -> Result<()> {
        let request: FileRequest = read_json(&mut recv).await?;
        let found = match self.file(&request.hash)? {
            Some((path, _)) => {
                let check = path.clone();
                let hash = tokio::task::spawn_blocking(move || hash_file(&check)).await??;
                if hash == request.hash {
                    Some(path)
                } else {
                    self.lost_file(&request.hash)?;
                    None
                }
            }
            None => None,
        };
        let Some(path) = found else {
            write_json(&mut send, &FileReply { found: false }).await?;
            send.finish()?;
            return Ok(());
        };
        let mut file = tokio::fs::File::open(&path).await?;
        let size = file.metadata().await?.len();
        let from = request.from.min(size);
        file.seek(std::io::SeekFrom::Start(from)).await?;
        write_json(&mut send, &FileReply { found: true }).await?;
        let mut progress = Progress::new(&request.hash, from, size, &self.emit);
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            send.write_all(&buf[..n]).await?;
            progress.add(n as u64);
        }
        send.finish()?;
        // Wait for them to have it all, so the progress ends when theirs does.
        let _ = send.stopped().await;
        drop(progress);
        Ok(())
    }

    /// Downloads the files this computer still needs, one at a time, newest first, for
    /// as long as the session lasts.
    async fn fetch_files(self: Arc<Self>, conn: Conn) {
        // Ones that failed this session: tried again next time.
        let mut failed = HashSet::new();
        loop {
            let wanted = self.db().wanted_files();
            let next = match wanted {
                Ok(wanted) => wanted.into_iter().find(|f| !failed.contains(&f.hash)),
                Err(e) => {
                    eprintln!("finding files to fetch: {e:#}");
                    None
                }
            };
            let Some(file) = next else {
                self.files_wanted.notified().await;
                continue;
            };
            if let Err(e) = self.fetch_file(&conn, &file).await {
                eprintln!("fetching a file: {e:#}");
                failed.insert(file.hash);
            }
        }
    }

    async fn fetch_file(&self, conn: &Conn, file: &FileInfo) -> Result<()> {
        tokio::fs::create_dir_all(&self.files_dir).await?;
        let part_path = self.files_dir.join(format!("{}.part", file.hash));
        let mut part = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&part_path)
            .await?;
        let mut have = part.metadata().await?.len();
        if have > file.size {
            have = 0;
        }
        part.set_len(have).await?;
        part.seek(std::io::SeekFrom::Start(have)).await?;

        let (mut send, mut recv) = conn.open_bi().await?;
        write_json(&mut send, &FileRequest { hash: file.hash.clone(), from: have }).await?;
        send.finish()?;
        let reply: FileReply = timeout(FILE_REPLY_TIMEOUT, read_json(&mut recv))
            .await
            .context("no answer to a file request")??;
        if !reply.found {
            drop(part);
            let _ = tokio::fs::remove_file(&part_path).await;
            self.db().set_file(&file.hash, "", FileState::Gone)?;
            return self.file_changed(&file.hash);
        }
        let mut progress = Progress::new(&file.hash, have, file.size, &self.emit);
        let mut buf = vec![0u8; 256 * 1024];
        while let Some(n) = recv.read(&mut buf).await? {
            have += n as u64;
            ensure!(have <= file.size, "the file is bigger than they said");
            part.write_all(&buf[..n]).await?;
            progress.add(n as u64);
        }
        part.flush().await?;
        drop(part);
        ensure!(have == file.size, "the file ended early");

        let path = part_path.clone();
        let hash = tokio::task::spawn_blocking(move || hash_file(&path)).await??;
        if hash != file.hash {
            let _ = tokio::fs::remove_file(&part_path).await;
            bail!("{} didn't match its hash", file.name);
        }
        // Deleted while it was on its way: don't keep it.
        if !self.db().file_in_use(&file.hash)? {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Ok(());
        }
        let name = safe_name(&file.name);
        let dir = self.files_dir.join(&file.hash);
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::rename(&part_path, dir.join(&name)).await?;
        self.db().set_file(&file.hash, &name, FileState::Done)?;
        drop(progress);
        self.file_changed(&file.hash)
    }

    /// Tells the UI about each message with this file.
    fn file_changed(&self, hash: &str) -> Result<()> {
        let refs = self.db().with_file(hash)?;
        for at in refs {
            self.updated(&at)?;
        }
        Ok(())
    }

    /// Removes files no message has any more, and partial downloads of them.
    fn prune_files(&self) {
        let result = (|| -> Result<()> {
            let unused = self.db().unused_files()?;
            for hash in unused {
                let dir = self.files_dir.join(&hash);
                if dir.exists() {
                    std::fs::remove_dir_all(&dir)?;
                }
                self.db().forget_file(&hash)?;
            }
            let Ok(entries) = std::fs::read_dir(&self.files_dir) else { return Ok(()) };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Some(hash) = name.strip_suffix(".part")
                    && !self.db().file_in_use(hash)?
                {
                    // Fails while it's still being written; the download checks again.
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("removing unused files: {e:#}");
        }
    }

    /// Erases vanishing messages as their time runs out.
    async fn vanish_loop(self: Arc<Self>) {
        let mut ticks = tokio::time::interval(Duration::from_secs(1));
        loop {
            ticks.tick().await;
            let gone = self.db().expire(self.clock.now_ms());
            match gone {
                Ok(gone) => {
                    for at in &gone {
                        let _ = self.updated(at);
                    }
                    if !gone.is_empty() {
                        self.prune_files();
                    }
                }
                Err(e) => eprintln!("erasing vanished messages: {e:#}"),
            }
        }
    }

    /// Sends a live-only frame, if connected.
    fn send_frame(&self, frame: Frame) {
        if let Some(s) = &self.state().session {
            let _ = s.out.send(frame);
        }
    }

    fn go_away(&self, away: Away, by_hand: bool) {
        {
            let mut st = self.state();
            if st.away.as_ref() == Some(&away) && st.away_by_hand == by_hand {
                return;
            }
            st.away = Some(away.clone());
            st.away_by_hand = by_hand;
        }
        self.send_frame(Frame::Away { note: away.note });
        self.changed();
    }

    fn set_peer_away(&self, away: Option<Away>) {
        let changed = {
            let mut st = self.state();
            let changed = st.peer_away != away;
            st.peer_away = away;
            changed
        };
        if changed {
            self.changed();
        }
    }

    /// Sends one of my entries now, if connected. Otherwise it goes with the catch-up.
    fn send_live(&self, entry: Entry) {
        let st = self.state();
        let (Some(s), Some((peer, _))) = (&st.session, &st.peer) else { return };
        if shares(&entry, st.shared_after, &peer.to_string())
            && let Some(wire) = entry.into_wire()
        {
            let _ = s.out.send(Frame::Msg(wire));
        }
    }

    /// Tells the UI about a new entry from the peer.
    fn announce(&self, entry: Entry) -> Result<()> {
        match entry.kind {
            Kind::Message { file, .. } => {
                let at = Ref { author: entry.author, seq: entry.seq };
                if let Some(msg) = self.db().message(&at)? {
                    (self.emit)(Event::Message(msg));
                }
                if file.is_some() {
                    self.files_wanted.notify_one();
                }
            }
            Kind::Reaction { to } => self.updated(&to)?,
            Kind::Edit { of } | Kind::Delete { of } => {
                // Blank it if it's deleted. Also catches an edit sent after the delete.
                if self.db().message(&of)?.is_some_and(|m| m.deleted) {
                    self.db().scrub(&of)?;
                    self.prune_files();
                }
                self.updated(&of)?;
            }
            Kind::Inert { .. } => {} // nothing to show
        }
        Ok(())
    }

    /// Tells the UI a message changed, if it's stored here.
    fn updated(&self, at: &Ref) -> Result<()> {
        if let Some(msg) = self.db().message(at)? {
            (self.emit)(Event::Updated(msg));
        }
        Ok(())
    }

    /// One of my messages, still there to edit or delete.
    fn my_message(&self, at: &Ref) -> Result<Message> {
        ensure!(at.author == self.id(), "you can only change your own messages");
        let msg = self.db().message(at)?.context("that message isn't here")?;
        ensure!(!msg.deleted, "that message was deleted");
        Ok(msg)
    }

    /// Whether `to` is one of our two logs, mine or the peer's.
    fn is_ours(&self, to: &Ref, peer: &str) -> bool {
        to.author == self.id() || to.author == peer
    }

    /// Passes a nudge on, unless one just came in.
    fn nudged(&self) {
        {
            let mut st = self.state();
            if st.nudge_received.is_some_and(|t| t.elapsed() < NUDGE_EVERY) {
                return;
            }
            st.nudge_received = Some(tokio::time::Instant::now());
        }
        (self.emit)(Event::Nudge);
    }

    fn set_notice(&self, notice: String) {
        self.state().notice = Some(notice);
        self.changed();
    }

    fn clear_pairing(&self, remote: EndpointId) {
        let cleared = {
            let mut st = self.state();
            let matches = st.pairing.as_ref().is_some_and(|p| p.prompt.id == remote.to_string());
            if matches {
                st.pairing = None;
            }
            matches
        };
        if cleared {
            self.changed();
        }
    }
}

/// Same six digits on both machines, derived from both public keys.
fn pair_code(a: EndpointId, b: EndpointId) -> String {
    let (lo, hi) = if a.as_bytes() < b.as_bytes() { (a, b) } else { (b, a) };
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"tightbeam pairing code v1");
    hasher.update(lo.as_bytes());
    hasher.update(hi.as_bytes());
    let bytes = hasher.finalize();
    let n = u32::from_le_bytes(bytes.as_bytes()[..4].try_into().unwrap()) % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

/// Private-network addresses only (192.168.x, 10.x, 172.16-31.x, link-local, IPv6 ULA).
/// My `Hello`: who I am, the last of their entries I have, and what this build does.
fn hello(name: String, have: u64) -> Frame {
    let features = FEATURES.iter().map(|f| f.to_string()).collect();
    Frame::Hello { name, have, version: Some(VERSION.into()), features }
}

/// The peer's `Hello`, at the start of a session.
struct TheirHello {
    name: String,
    have: u64,
    version: Option<String>,
    features: Vec<String>,
}

/// Features as stored in settings: comma separated.
fn features_from(stored: Option<&str>) -> Vec<String> {
    let stored = stored.unwrap_or_default();
    stored.split(',').filter(|f| !f.is_empty()).map(String::from).collect()
}

fn hash_file(path: &Path) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(std::fs::File::open(path)?)?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// A file name that's safe to store on Windows and macOS, keeping its extension.
fn safe_name(name: &str) -> String {
    const MAX: usize = 100;
    let name: String = name
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    let name = name.trim().trim_start_matches('.').trim_end_matches(['.', ' ']);
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.chars().count() <= 10 => (stem, ext),
        _ => (name, ""),
    };
    let room = MAX - ext.chars().count() - 1;
    let mut stem: String = stem.chars().take(room).collect::<String>().trim_end().to_string();
    if stem.is_empty() {
        stem = "file".into();
    }
    // Names Windows reserves for devices, with or without an extension.
    let device = stem.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&device.as_str())
        || (device.len() == 4
            && (device.starts_with("COM") || device.starts_with("LPT"))
            && device.as_bytes()[3].is_ascii_digit());
    if reserved {
        stem.insert(0, '_');
    }
    if ext.is_empty() { stem } else { format!("{stem}.{ext}") }
}

/// Reports a transfer's progress to the UI, a few times a second. Dropping it says the
/// transfer is over, done or not, so a broken one doesn't look stuck.
struct Progress {
    hash: String,
    have: u64,
    size: u64,
    last: Instant,
    emit: Emit,
}

impl Progress {
    fn new(hash: &str, have: u64, size: u64, emit: &Emit) -> Self {
        let (hash, emit) = (hash.to_string(), emit.clone());
        Self { hash, have, size, last: Instant::now(), emit }
    }

    fn add(&mut self, n: u64) {
        self.have += n;
        if self.last.elapsed() >= PROGRESS_EVERY && self.have < self.size {
            self.last = Instant::now();
            self.send(self.have);
        }
    }

    fn send(&self, have: u64) {
        (self.emit)(Event::Transfer { hash: self.hash.clone(), have, size: self.size });
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.send(self.size);
    }
}

/// Whether one of my entries goes to `peer`: only what I wrote after pairing with them
/// (`shared_after`), and only about messages they have, theirs or mine since then. A
/// reply always goes: its text is new, and a quote they don't have shows as missing.
fn shares(entry: &Entry, shared_after: u64, peer: &str) -> bool {
    let about = match &entry.kind {
        Kind::Reaction { to } | Kind::Edit { of: to } | Kind::Delete { of: to } => Some(to),
        Kind::Message { .. } | Kind::Inert { .. } => None,
    };
    entry.seq > shared_after
        && about.is_none_or(|to| {
            to.author == peer || (to.author == entry.author && to.seq > shared_after)
        })
}

/// Aborts a task when dropped.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The peer's name for messages, or a stand-in before they've named themselves.
fn title_name(name: Option<String>) -> String {
    name.filter(|n| !n.is_empty()).unwrap_or_else(|| "the other person".into())
}

async fn write_frame(send: &mut SendStream, frame: &Frame) -> Result<()> {
    write_json(send, frame).await
}

async fn read_frame(recv: &mut RecvStream) -> Result<Frame> {
    read_json(recv).await
}

/// Length-prefixed JSON, as frames are sent.
async fn write_json(send: &mut SendStream, value: &impl Serialize) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    send.write_all(&(body.len() as u32).to_be_bytes()).await?;
    send.write_all(&body).await?;
    Ok(())
}

async fn read_json<T: serde::de::DeserializeOwned>(recv: &mut RecvStream) -> Result<T> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    ensure!(len <= MAX_FRAME, "frame too large ({len} bytes)");
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod protocol_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Frame {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn unknown_frame_types_parse_as_unknown() {
        assert!(matches!(parse(r#"{"t":"pin"}"#), Frame::Unknown));
        assert!(matches!(
            parse(r#"{"t":"reaction","target":{"author":"x","seq":3},"emoji":"+1"}"#),
            Frame::Unknown
        ));
    }

    #[test]
    fn extra_fields_on_known_frames_are_ignored() {
        let frame =
            parse(r#"{"t":"msg","seq":7,"ts":1,"text":"hi","pinned_by":{"author":"x","seq":2}}"#);
        assert!(
            matches!(frame, Frame::Msg(Wire { seq: 7, hlc: None, ref text, .. }) if text == "hi")
        );
        let frame =
            parse(r#"{"t":"msg","seq":7,"ts":1,"text":"hi","reply_to":{"author":"x","seq":2}}"#);
        assert!(matches!(frame, Frame::Msg(Wire { reply_to: Some(Ref { seq: 2, .. }), .. })));
    }

    #[test]
    fn known_frames_keep_their_wire_format() {
        let json = serde_json::to_string(&Frame::Ack { have: 4 }).unwrap();
        assert_eq!(json, r#"{"t":"ack","have":4}"#);
        let json = serde_json::to_string(&Frame::Seen { up_to: 4 }).unwrap();
        assert_eq!(json, r#"{"t":"seen","up_to":4}"#);
        let json = serde_json::to_string(&Frame::Nudge).unwrap();
        assert_eq!(json, r#"{"t":"nudge"}"#);
        let json = serde_json::to_string(&Frame::Away { note: None }).unwrap();
        assert_eq!(json, r#"{"t":"away"}"#);
        let json = serde_json::to_string(&Frame::Away { note: Some("lunch".into()) }).unwrap();
        assert_eq!(json, r#"{"t":"away","note":"lunch"}"#);
        let json = serde_json::to_string(&Frame::Back).unwrap();
        assert_eq!(json, r#"{"t":"back"}"#);
        let json = serde_json::to_string(&Frame::Dnd { until: Some(5) }).unwrap();
        assert_eq!(json, r#"{"t":"dnd","until":5}"#);
        let hello = Frame::Hello {
            name: "a".into(),
            have: 1,
            version: Some("0.7.0".into()),
            features: vec!["files".into()],
        };
        let json = serde_json::to_string(&hello).unwrap();
        assert_eq!(
            json,
            r#"{"t":"hello","name":"a","have":1,"version":"0.7.0","features":["files"]}"#
        );
        // A Hello from before 0.6 has no version, and from before 0.7 no features.
        let old = parse(r#"{"t":"hello","name":"a","have":1}"#);
        assert!(
            matches!(old, Frame::Hello { version: None, ref features, .. } if features.is_empty())
        );
        let vanishing = Entry {
            kind: Kind::Message { reply_to: None, vanish: Some(60), file: None },
            ..Entry::message("me", 9, 1, "psst")
        };
        let json = serde_json::to_string(&Frame::Msg(vanishing.into_wire().unwrap())).unwrap();
        assert_eq!(json, r#"{"t":"msg","seq":9,"ts":1,"hlc":1,"text":"psst","vanish":60}"#);
        let file = FileInfo {
            hash: "ab".repeat(32),
            name: "a.png".into(),
            size: 5,
            mime: "image/png".into(),
            width: None,
            height: None,
            preview: None,
        };
        let photo = Entry {
            kind: Kind::Message { reply_to: None, vanish: None, file: Some(file) },
            ..Entry::message("me", 10, 1, "")
        };
        let json = serde_json::to_string(&Frame::Msg(photo.into_wire().unwrap())).unwrap();
        let hash = "ab".repeat(32);
        assert_eq!(
            json,
            format!(
                r#"{{"t":"msg","seq":10,"ts":1,"hlc":1,"text":"","file":{{"hash":"{hash}","name":"a.png","size":5,"mime":"image/png"}}}}"#
            )
        );
        // A plain message has the fields from before 0.4, plus `hlc` from 0.5.
        let plain = Frame::Msg(Entry::message("me", 7, 1, "hi").into_wire().unwrap());
        let json = serde_json::to_string(&plain).unwrap();
        assert_eq!(json, r#"{"t":"msg","seq":7,"ts":1,"hlc":1,"text":"hi"}"#);
        let to = Ref { author: "x".into(), seq: 2 };
        let reaction = Entry {
            text: "👍".into(),
            kind: Kind::Reaction { to },
            ..Entry::message("me", 8, 1, "")
        };
        let json = serde_json::to_string(&Frame::Msg(reaction.into_wire().unwrap())).unwrap();
        assert_eq!(
            json,
            r#"{"t":"msg","seq":8,"ts":1,"hlc":1,"text":"👍","react_to":{"author":"x","seq":2}}"#
        );
    }

    #[test]
    fn file_names_are_safe_to_store() {
        assert_eq!(safe_name("beach.jpg"), "beach.jpg");
        assert_eq!(safe_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_name(r"C:\Windows\a:b?.png"), "C__Windows_a_b_.png");
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name("trailing. . "), "trailing");
        assert_eq!(safe_name("con.txt"), "_con.txt");
        assert_eq!(safe_name("COM1"), "_COM1");
        assert_eq!(safe_name("company.txt"), "company.txt");
        assert_eq!(safe_name("..."), "file");
        assert_eq!(safe_name(""), "file");
        let long = format!("{}.jpeg", "x".repeat(300));
        let safe = safe_name(&long);
        assert_eq!(safe.chars().count(), 100);
        assert!(safe.ends_with(".jpeg"));
    }
}
