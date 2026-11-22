//! The chat engine: identity, local discovery, pairing, and syncing messages with the
//! one paired peer. Knows nothing about Tauri; it reports changes through `Emit`.
//!
//! Wire protocol (ALPN `tightbeam/1`): one bidirectional QUIC stream per connection,
//! carrying length-prefixed JSON `Frame`s. A connection starts with either
//! `PairRequest` (unpaired) or `Hello` (paired). After both sides say `Hello`, each
//! sends the messages the other is missing, then live messages and acks.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
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

use crate::db::{Db, Message};

pub const ALPN: &[u8] = b"tightbeam/1";
/// mDNS service name, so we only see other Tightbeam apps (`_tightbeam._udp.local`).
const SERVICE: &str = "tightbeam";
const MAX_FRAME: usize = 64 * 1024;
const MAX_TEXT_CHARS: usize = 4000;
const MAX_NAME_CHARS: usize = 40;
const PAIR_PROMPT_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REDIAL_EVERY: Duration = Duration::from_secs(3);

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Frame {
    PairRequest { name: String },
    PairAccept { name: String },
    PairReject,
    /// `have` = the highest sequence number I hold from you.
    Hello { name: String, have: u64 },
    Msg { seq: u64, ts: i64, text: String },
    Ack { have: u64 },
}

/// Everything the UI needs except the message list.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub me: Person,
    pub peer: Option<Person>,
    pub connected: bool,
    /// Highest of my sequence numbers the peer has confirmed. Mine at or below are delivered.
    pub peer_has: u64,
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
    nearby: BTreeMap<String, String>,
    pairing: Option<Pairing>,
    session: Option<Session>,
    next_generation: u64,
    notice: Option<String>,
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
    pub async fn start_with(data_dir: PathBuf, emit: Emit, loopback_only: bool) -> Result<Arc<Self>> {
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
        let peer_has = db.get_str("peer_has")?.and_then(|s| s.parse().ok()).unwrap_or(0);

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
        let mdns = MdnsAddressLookup::builder()
            .service_name(SERVICE)
            .build(endpoint.id())?;
        endpoint.address_lookup()?.add(mdns.clone());

        let core = Arc::new(Self {
            me: endpoint.id(),
            endpoint,
            db: Mutex::new(db),
            state: Mutex::new(State {
                name,
                peer,
                peer_has,
                nearby: BTreeMap::new(),
                pairing: None,
                session: None,
                next_generation: 0,
                notice: None,
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
            peer: st.peer.as_ref().map(|(id, name)| Person { id: id.to_string(), name: name.clone() }),
            connected: st.session.is_some(),
            peer_has: st.peer_has,
            nearby: st
                .nearby
                .iter()
                .map(|(id, name)| Person { id: id.clone(), name: name.clone() })
                .collect(),
            pairing: st.pairing.as_ref().map(|p| p.prompt.clone()),
            notice: st.notice.clone(),
        }
    }

    pub fn messages(&self) -> Result<Vec<Message>> {
        self.db().all()
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

    pub fn send_message(&self, text: &str) -> Result<Message> {
        let text = clean(text, MAX_TEXT_CHARS);
        ensure!(!text.is_empty(), "message can't be empty");
        ensure!(self.peer_id().is_some(), "not paired with anyone yet");
        let msg = {
            let db = self.db();
            let msg = Message {
                author: self.id(),
                seq: db.max_seq(&self.id())? + 1,
                ts: now_ms(),
                text,
            };
            db.insert(&msg)?;
            msg
        };
        (self.emit)(Event::Message(msg.clone()));
        if let Some(s) = &self.state().session {
            let _ = s.out.send(Frame::Msg { seq: msg.seq, ts: msg.ts, text: msg.text.clone() });
        }
        Ok(msg)
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
        }
        let session = {
            let mut st = self.state();
            st.peer = None;
            st.peer_has = 0;
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
                    eprintln!("[{}] dial failed after {:?}: {e:#}", self.me.fmt_short(), started.elapsed());
                    continue;
                }
                Err(_) => {
                    eprintln!("[{}] dial timed out after {:?}", self.me.fmt_short(), started.elapsed());
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
            let old = st.session.replace(Session { generation, out: out.clone(), conn: conn.clone() });
            if let Some(old) = old {
                old.conn.close(0u32.into(), b"replaced");
            }
            generation
        };
        for m in self.db().since(&self.id(), their_have)? {
            let _ = out.send(Frame::Msg { seq: m.seq, ts: m.ts, text: m.text });
        }
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
                Frame::Msg { seq, ts, text } => {
                    let msg = Message { author: peer.to_string(), seq, ts, text: clean(&text, MAX_TEXT_CHARS) };
                    let (is_new, have) = {
                        let db = self.db();
                        (db.insert(&msg)?, db.max_seq(peer)?)
                    };
                    if is_new {
                        (self.emit)(Event::Message(msg));
                    }
                    let _ = out.send(Frame::Ack { have });
                }
                Frame::Ack { have } => self.set_peer_has(have, false)?,
                Frame::Hello { name, .. } => self.update_peer_name(&name)?,
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
        let new = {
            let mut st = self.state();
            let new = if exact { have } else { st.peer_has.max(have) };
            if new == st.peer_has {
                return Ok(());
            }
            st.peer_has = new;
            new
        };
        self.db().set("peer_has", new.to_string())?;
        self.changed();
        Ok(())
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
