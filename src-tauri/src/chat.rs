//! The chat engine: identity, local discovery, pairing, and syncing messages with the
//! one paired peer. Knows nothing about Tauri; it reports changes through `Emit`.
//!
//! Wire protocol (ALPN `tightbeam/1`): one bidirectional QUIC stream per connection,
//! carrying length-prefixed JSON `Frame`s. A connection starts with either
//! `PairRequest` (unpaired) or `Hello` (paired). After both sides say `Hello`, each
//! sends the log entries the other is missing (messages and reactions), then live
//! entries, acks, seen receipts, typing and nudges.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use std::{borrow::Cow, net::IpAddr};

use iroh::{
    Endpoint, EndpointId, SecretKey, TransportAddr,
    address_lookup::AddrFilter,
    endpoint::{Connection, Incoming, PortmapperConfig, RecvStream, SendStream, presets},
    endpoint_info::UserData,
};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};

use crate::db::{Cursor, Db, Entry, Kind, Message, Page, Reaction, Ref};

pub const ALPN: &[u8] = b"tightbeam/1";
/// mDNS service name, so we only see other Tightbeam apps (`_tightbeam._udp.local`).
const SERVICE: &str = "tightbeam";
const MAX_FRAME: usize = 64 * 1024;
const MAX_TEXT_CHARS: usize = 4000;
const MAX_NAME_CHARS: usize = 40;
/// Room for an emoji made of several code points, like a family or a flag.
const MAX_EMOJI_CHARS: usize = 16;
const PAIR_PROMPT_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REDIAL_EVERY: Duration = Duration::from_secs(3);
/// At most one nudge this often, each way.
const NUDGE_EVERY: Duration = Duration::from_secs(3);

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
    },
    /// An entry in the sender's log. Plain, it's a message.
    Msg {
        seq: u64,
        ts: i64,
        text: String,
        /// The message this one answers. Added in 0.4; older peers show a plain message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<Ref>,
        /// Makes this a reaction to that message: `text` is the emoji, or "" to take
        /// the sender's reaction back. Added in 0.4; older peers show the emoji as a
        /// message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        react_to: Option<Ref>,
    },
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
    /// A frame type from a newer version. Never sent.
    #[serde(other, skip_serializing)]
    Unknown,
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
    pub pairing: Option<PairPrompt>,
    pub notice: Option<String>,
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
    /// The reactions on a message changed. `reactions` is the whole new set.
    Reactions {
        to: Ref,
        reactions: Vec<Reaction>,
    },
    /// The peer is typing.
    Typing,
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
    out: mpsc::UnboundedSender<Frame>,
    conn: Connection,
}

struct State {
    name: String,
    peer: Option<(EndpointId, String)>,
    peer_has: u64,
    peer_seen: u64,
    /// Highest of the peer's sequence numbers I've seen; what our `Seen` frames say.
    seen: u64,
    nearby: BTreeMap<String, String>,
    pairing: Option<Pairing>,
    session: Option<Session>,
    next_generation: u64,
    notice: Option<String>,
    nudge_sent: Option<Instant>,
    nudge_received: Option<Instant>,
}

pub struct Core {
    endpoint: Endpoint,
    me: EndpointId,
    db: Mutex<Db>,
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
    /// (Windows refuses to send from a LAN address to that same address, so same-machine
    /// tests over the LAN address are flaky.)
    pub async fn start_with(
        data_dir: PathBuf,
        emit: Emit,
        loopback_only: bool,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("creating {}", data_dir.display()))?;
        let db = Db::open(&data_dir.join("tightbeam.db"))?;

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
        let (peer_has, peer_seen, seen) =
            (counter("peer_has")?, counter("peer_seen")?, counter("seen")?);

        // LAN only for now: no relay servers, no public DNS, just mDNS. No UPnP either:
        // we never ask the router to open a public port, and we only ever advertise
        // private addresses.
        let mut builder = Endpoint::builder(presets::Minimal)
            .secret_key(secret)
            .alpns(vec![ALPN.to_vec()])
            .portmapper_config(PortmapperConfig::Disabled)
            .addr_filter(AddrFilter::new(move |addrs| {
                Cow::Owned(addrs.iter().filter(|a| is_lan(a, loopback_only)).cloned().collect())
            }));
        if loopback_only {
            builder = builder.clear_ip_transports().bind_addr("127.0.0.1:0")?;
        }
        let endpoint = builder.bind().await?;
        let mdns = MdnsAddressLookup::builder().service_name(SERVICE).build(endpoint.id())?;
        endpoint.address_lookup()?.add(mdns.clone());

        let core = Arc::new(Self {
            me: endpoint.id(),
            endpoint,
            db: Mutex::new(db),
            state: Mutex::new(State {
                name,
                peer,
                peer_has,
                peer_seen,
                seen,
                nearby: BTreeMap::new(),
                pairing: None,
                session: None,
                next_generation: 0,
                notice: None,
                nudge_sent: None,
                nudge_received: None,
            }),
            emit,
            tasks: Mutex::new(Vec::new()),
            rt: tokio::runtime::Handle::current(),
        });
        core.advertise_name();

        *core.tasks.lock().unwrap() = vec![
            tokio::spawn(core.clone().accept_loop()).abort_handle(),
            tokio::spawn(core.clone().discovery_loop(mdns)).abort_handle(),
            tokio::spawn(core.clone().dial_loop()).abort_handle(),
        ];
        Ok(core)
    }

    /// Stop background work and close connections cleanly so the peer notices right away.
    pub async fn shutdown(&self) {
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
        self.endpoint.close().await;
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
            pairing: st.pairing.as_ref().map(|p| p.prompt.clone()),
            notice: st.notice.clone(),
        }
    }

    /// The whole history, oldest first. The UI uses `page` instead.
    pub fn messages(&self) -> Result<Vec<Message>> {
        self.db().all()
    }

    pub fn page(&self, before: Option<&Cursor>, limit: usize) -> Result<Page> {
        self.db().page(before, limit)
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
            let _ = out.send(Frame::Hello { name, have: have.unwrap_or(0) });
        }
        self.changed();
        Ok(())
    }

    /// `reply_to`: the message this one answers.
    pub fn send_message(&self, text: &str, reply_to: Option<Ref>) -> Result<Message> {
        let text = clean(text, MAX_TEXT_CHARS);
        ensure!(!text.is_empty(), "message can't be empty");
        let peer = self.peer_id().context("not paired with anyone yet")?;
        if let Some(to) = &reply_to {
            ensure!(self.is_ours(to, &peer.to_string()), "can't reply to that");
        }
        let entry = self.append(text, Kind::Message { reply_to })?;
        let at = Ref { author: entry.author.clone(), seq: entry.seq };
        let msg = self.db().message(&at)?.context("message vanished")?;
        (self.emit)(Event::Message(msg.clone()));
        self.send_live(entry);
        Ok(msg)
    }

    /// Sets my reaction to a message, replacing any earlier one. `emoji` "" takes it back.
    pub fn react(&self, to: Ref, emoji: &str) -> Result<()> {
        let emoji = clean(emoji, MAX_EMOJI_CHARS);
        let peer = self.peer_id().context("not paired with anyone yet")?;
        ensure!(self.is_ours(&to, &peer.to_string()), "can't react to that");
        let entry = self.append(emoji, Kind::Reaction { to: to.clone() })?;
        let reactions = self.db().reactions(&to)?;
        (self.emit)(Event::Reactions { to, reactions });
        self.send_live(entry);
        Ok(())
    }

    /// I'm typing. The UI calls this every few seconds while I type.
    pub fn typing(&self) {
        if let Some(s) = &self.state().session {
            let _ = s.out.send(Frame::Typing);
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
        Ok(())
    }

    /// Erases the conversation on this computer; the peer keeps their copy. My messages
    /// they haven't received yet stay, so they still get delivered. Sequence numbers
    /// carry on as before, so sync never asks for the erased messages again.
    pub fn clear_history(&self) -> Result<()> {
        let keep_after = self.peer_id().map(|_| self.state().peer_has);
        self.db().clear(&self.id(), keep_after)?;
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
                !st.nudge_sent.is_some_and(|t| t.elapsed() < NUDGE_EVERY),
                "You just nudged. Give it a few seconds."
            );
            st.nudge_sent = Some(Instant::now());
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
        }
        let session = {
            let mut st = self.state();
            st.peer = None;
            st.peer_has = 0;
            st.peer_seen = 0;
            st.seen = 0;
            st.session.take()
        };
        if let Some(s) = session {
            s.conn.close(0u32.into(), b"unpaired");
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
        while let Some(incoming) = self.endpoint.accept().await {
            let core = self.clone();
            tokio::spawn(async move {
                if let Err(e) = core.handle_incoming(incoming).await {
                    eprintln!("incoming connection: {e:#}");
                }
            });
        }
    }

    async fn discovery_loop(self: Arc<Self>, mdns: MdnsAddressLookup) {
        let mut events = mdns.subscribe().await;
        while let Some(event) = events.next().await {
            {
                let mut st = self.state();
                match event {
                    DiscoveryEvent::Discovered { endpoint_info, .. } => {
                        let id = endpoint_info.endpoint_id;
                        if id == self.me {
                            continue;
                        }
                        let name = endpoint_info
                            .data
                            .user_data()
                            .map(|u| u.to_string())
                            .unwrap_or_default();
                        st.nearby.insert(id.to_string(), name);
                    }
                    DiscoveryEvent::Expired { endpoint_id } => {
                        st.nearby.remove(&endpoint_id.to_string());
                    }
                    _ => continue,
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
            let conn = match timeout(CONNECT_TIMEOUT, self.endpoint.connect(peer, ALPN)).await {
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

    async fn handle_incoming(self: Arc<Self>, incoming: Incoming) -> Result<()> {
        let conn = incoming.await?;
        let remote = conn.remote_id();
        let (send, mut recv) = conn.accept_bi().await?;
        match read_frame(&mut recv).await? {
            Frame::PairRequest { name } => self.answer_pair_request(conn, send, recv, name).await,
            Frame::Hello { name, have } => {
                if self.peer_id() != Some(remote) {
                    conn.close(1u32.into(), b"not paired");
                    return Ok(());
                }
                self.run_session(conn, send, recv, Some((name, have))).await
            }
            other => bail!("unexpected first frame: {other:?}"),
        }
    }

    async fn answer_pair_request(
        self: Arc<Self>,
        conn: Connection,
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
        let conn = timeout(CONNECT_TIMEOUT, self.endpoint.connect(remote, ALPN))
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
        conn: Connection,
        mut send: SendStream,
        mut recv: RecvStream,
        their_hello: Option<(String, u64)>,
    ) -> Result<()> {
        let peer = self.peer_id().context("not paired")?;
        ensure!(conn.remote_id() == peer, "connection is not from our peer");
        let peer_str = peer.to_string();

        let have = self.db().max_seq(&peer_str)?;
        write_frame(&mut send, &Frame::Hello { name: self.name(), have }).await?;
        let (their_name, their_have) = match their_hello {
            Some(hello) => hello,
            None => match read_frame(&mut recv).await? {
                Frame::Hello { name, have } => (name, have),
                other => bail!("expected hello, got {other:?}"),
            },
        };
        self.update_peer_name(&their_name)?;
        self.set_peer_has(their_have, true)?;

        // Register the session before reading the backlog, so a message sent in between
        // goes out live. Worst case it's sent twice, and the receiver ignores duplicates.
        let (out, mut rx) = mpsc::unbounded_channel();
        let generation = {
            let mut st = self.state();
            st.next_generation += 1;
            let generation = st.next_generation;
            let old =
                st.session.replace(Session { generation, out: out.clone(), conn: conn.clone() });
            if let Some(old) = old {
                old.conn.close(0u32.into(), b"replaced");
            }
            generation
        };
        for entry in self.db().since(&self.id(), their_have)? {
            let _ = out.send(to_frame(entry));
        }
        // Covers anything seen while we were apart.
        let _ = out.send(Frame::Seen { up_to: self.state().seen });
        self.changed();

        let writer = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                if write_frame(&mut send, &frame).await.is_err() {
                    break;
                }
            }
        });
        let result = self.read_loop(&mut recv, &peer_str, &out).await;
        writer.abort();
        conn.close(0u32.into(), b"bye");
        {
            let mut st = self.state();
            if st.session.as_ref().is_some_and(|s| s.generation == generation) {
                st.session = None;
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
                Frame::Msg { seq, ts, text, reply_to, react_to } => {
                    // Refs to anyone but the two of us are dropped: a reply becomes a
                    // plain message, a reaction is stored but never shown.
                    let kind = match react_to {
                        Some(to) => Kind::Reaction { to },
                        None => {
                            Kind::Message { reply_to: reply_to.filter(|to| self.is_ours(to, peer)) }
                        }
                    };
                    let limit = if matches!(kind, Kind::Reaction { .. }) {
                        MAX_EMOJI_CHARS
                    } else {
                        MAX_TEXT_CHARS
                    };
                    let entry = Entry {
                        author: peer.to_string(),
                        seq,
                        ts,
                        text: clean(&text, limit),
                        kind,
                    };
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
        let name = self.name();
        let data = if name.is_empty() { None } else { UserData::try_from(name).ok() };
        self.endpoint.set_user_data_for_address_lookup(data);
    }

    fn save_peer(&self, id: EndpointId, name: &str) -> Result<()> {
        {
            let db = self.db();
            db.set("peer_id", id.to_string())?;
            db.set("peer_name", name)?;
        }
        self.state().peer = Some((id, name.to_string()));
        self.changed();
        Ok(())
    }

    fn update_peer_name(&self, name: &str) -> Result<()> {
        let name = clean(name, MAX_NAME_CHARS);
        let mut st = self.state();
        if let Some((_, current)) = &mut st.peer
            && *current != name
        {
            *current = name.clone();
            drop(st);
            self.db().set("peer_name", &name)?;
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
        Ok(())
    }

    /// Adds an entry to my log.
    fn append(&self, text: String, kind: Kind) -> Result<Entry> {
        let db = self.db();
        let entry =
            Entry { author: self.id(), seq: db.max_seq(&self.id())? + 1, ts: now_ms(), text, kind };
        db.insert(&entry)?;
        Ok(entry)
    }

    /// Sends one of my entries now, if connected. Otherwise it goes with the catch-up.
    fn send_live(&self, entry: Entry) {
        if let Some(s) = &self.state().session {
            let _ = s.out.send(to_frame(entry));
        }
    }

    /// Tells the UI about a new entry from the peer.
    fn announce(&self, entry: Entry) -> Result<()> {
        match entry.kind {
            Kind::Message { .. } => {
                let at = Ref { author: entry.author, seq: entry.seq };
                if let Some(msg) = self.db().message(&at)? {
                    (self.emit)(Event::Message(msg));
                }
            }
            Kind::Reaction { to } => {
                let reactions = self.db().reactions(&to)?;
                (self.emit)(Event::Reactions { to, reactions });
            }
        }
        Ok(())
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
            st.nudge_received = Some(Instant::now());
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
fn is_lan(addr: &TransportAddr, allow_loopback: bool) -> bool {
    match addr {
        TransportAddr::Ip(sock) => match sock.ip() {
            IpAddr::V4(ip) if ip.is_loopback() => allow_loopback,
            IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
            IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local(),
        },
        _ => false,
    }
}

fn to_frame(entry: Entry) -> Frame {
    let (reply_to, react_to) = match entry.kind {
        Kind::Message { reply_to } => (reply_to, None),
        Kind::Reaction { to } => (None, Some(to)),
    };
    Frame::Msg { seq: entry.seq, ts: entry.ts, text: entry.text, reply_to, react_to }
}

fn clean(s: &str, max_chars: usize) -> String {
    s.trim().chars().take(max_chars).collect()
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

async fn write_frame(send: &mut SendStream, frame: &Frame) -> Result<()> {
    let body = serde_json::to_vec(frame)?;
    send.write_all(&(body.len() as u32).to_be_bytes()).await?;
    send.write_all(&body).await?;
    Ok(())
}

async fn read_frame(recv: &mut RecvStream) -> Result<Frame> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    ensure!(len <= MAX_FRAME, "frame too large ({len} bytes)");
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Frame {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn unknown_frame_types_parse_as_unknown() {
        assert!(matches!(parse(r#"{"t":"away"}"#), Frame::Unknown));
        assert!(matches!(
            parse(r#"{"t":"reaction","target":{"author":"x","seq":3},"emoji":"+1"}"#),
            Frame::Unknown
        ));
    }

    #[test]
    fn extra_fields_on_known_frames_are_ignored() {
        let frame =
            parse(r#"{"t":"msg","seq":7,"ts":1,"text":"hi","edit_of":{"author":"x","seq":2}}"#);
        assert!(matches!(frame, Frame::Msg { seq: 7, ref text, .. } if text == "hi"));
        let frame =
            parse(r#"{"t":"msg","seq":7,"ts":1,"text":"hi","reply_to":{"author":"x","seq":2}}"#);
        assert!(matches!(frame, Frame::Msg { reply_to: Some(Ref { seq: 2, .. }), .. }));
    }

    #[test]
    fn known_frames_keep_their_wire_format() {
        let json = serde_json::to_string(&Frame::Ack { have: 4 }).unwrap();
        assert_eq!(json, r#"{"t":"ack","have":4}"#);
        let json = serde_json::to_string(&Frame::Seen { up_to: 4 }).unwrap();
        assert_eq!(json, r#"{"t":"seen","up_to":4}"#);
        let json = serde_json::to_string(&Frame::Nudge).unwrap();
        assert_eq!(json, r#"{"t":"nudge"}"#);
        // A plain message has no reply or reaction fields, exactly as before 0.4.
        let plain = to_frame(Entry::message("me", 7, 1, "hi"));
        let json = serde_json::to_string(&plain).unwrap();
        assert_eq!(json, r#"{"t":"msg","seq":7,"ts":1,"text":"hi"}"#);
        let to = Ref { author: "x".into(), seq: 2 };
        let reaction = Entry {
            text: "👍".into(),
            kind: Kind::Reaction { to },
            ..Entry::message("me", 8, 1, "")
        };
        let json = serde_json::to_string(&to_frame(reaction)).unwrap();
        assert_eq!(
            json,
            r#"{"t":"msg","seq":8,"ts":1,"text":"👍","react_to":{"author":"x","seq":2}}"#
        );
    }
}
