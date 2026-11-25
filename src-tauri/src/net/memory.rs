//! An in-memory network for tests. Every node that joins a `Lan` discovers the others
//! by name, connects to them instantly, and streams bytes through channels.
//!
//! Like QUIC, a connection ends abruptly: reads and writes on its streams fail at once.
//! Closing it, or dropping every handle to one side of it, ends it for both sides. A node
//! that closes (its app shut down) leaves the LAN: the others see it go, and its
//! connections end. A node can also drop out (`Lan::drop_out`): its connections end on
//! its side only, while the other side notices nothing, the way a laptop that loses its
//! Wi-Fi leaves the other end waiting for a timeout.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{Result, anyhow, bail};
use iroh::{EndpointId, SecretKey};
use tokio::sync::{mpsc, oneshot, watch};

use super::{Accepting, Discoveries, Discovery, Network};

#[derive(Clone, Default)]
pub(crate) struct Lan(Arc<Mutex<Nodes>>);

#[derive(Default)]
struct Nodes(HashMap<EndpointId, Joined>);

struct Joined {
    name: String,
    incoming: mpsc::UnboundedSender<Conn>,
    watchers: Vec<mpsc::UnboundedSender<Discovery>>,
    /// Every connection this node is part of, and which side of it is this node's.
    links: Vec<(Arc<Link>, usize)>,
}

impl Lan {
    /// A node with this key joins, in place of any earlier node with the same key.
    pub fn join(&self, secret: SecretKey) -> Network {
        let id = secret.public();
        let (incoming, accept) = mpsc::unbounded_channel();
        let mut nodes = self.0.lock().unwrap();
        if let Some(old) = nodes.0.remove(&id) {
            old.leave();
        }
        nodes.tell_others(id, || Discovery::Found { id, name: String::new() });
        let joined = Joined { name: String::new(), incoming, watchers: vec![], links: vec![] };
        nodes.0.insert(id, joined);
        Network::Memory(Node { id, lan: self.clone(), accept: tokio::sync::Mutex::new(accept) })
    }

    /// `id` loses its connections without closing them: they end on its side only.
    pub fn drop_out(&self, id: EndpointId) {
        if let Some(node) = self.0.lock().unwrap().0.get_mut(&id) {
            for (link, side) in node.links.drain(..) {
                link.cut[side].send_replace(true);
            }
        }
    }
}

impl Nodes {
    fn tell_others(&mut self, id: EndpointId, event: impl Fn() -> Discovery) {
        for (_, node) in self.0.iter_mut().filter(|(other, _)| **other != id) {
            node.watchers.retain(|w| w.send(event()).is_ok());
        }
    }
}

impl Joined {
    fn leave(self) {
        for (link, _) in self.links {
            link.closed.send_replace(true);
        }
    }
}

pub(crate) struct Node {
    id: EndpointId,
    lan: Lan,
    accept: tokio::sync::Mutex<mpsc::UnboundedReceiver<Conn>>,
}

impl Node {
    pub fn id(&self) -> EndpointId {
        self.id
    }

    pub fn advertise(&self, name: &str) {
        let mut nodes = self.lan.0.lock().unwrap();
        let Some(me) = nodes.0.get_mut(&self.id) else { return };
        me.name = name.to_string();
        let (id, name) = (self.id, name.to_string());
        nodes.tell_others(id, || Discovery::Found { id, name: name.clone() });
    }

    pub fn discoveries(&self) -> Discoveries {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut nodes = self.lan.0.lock().unwrap();
        for (id, node) in nodes.0.iter().filter(|(id, _)| **id != self.id) {
            let _ = tx.send(Discovery::Found { id: *id, name: node.name.clone() });
        }
        if let Some(me) = nodes.0.get_mut(&self.id) {
            me.watchers.push(tx);
        }
        Discoveries::Memory(rx)
    }

    pub fn connect(&self, peer: EndpointId) -> Result<Conn> {
        let mut nodes = self.lan.0.lock().unwrap();
        if !nodes.0.contains_key(&self.id) {
            bail!("this node has left the network");
        }
        let Some(them) = nodes.0.get_mut(&peer) else { bail!("{peer} isn't on the network") };
        let (mine, theirs) = Conn::pair(self.id, peer);
        let link = mine.link.clone();
        if them.incoming.send(theirs).is_err() {
            bail!("{peer} isn't accepting connections");
        }
        them.links.retain(|(l, _)| !l.is_closed());
        them.links.push((link.clone(), 1));
        let me = nodes.0.get_mut(&self.id).unwrap();
        me.links.retain(|(l, _)| !l.is_closed());
        me.links.push((link, 0));
        Ok(mine)
    }

    pub async fn accept(&self) -> Option<Accepting> {
        self.accept.lock().await.recv().await.map(Accepting::Memory)
    }

    /// Leaves the network: the others see this node go, and its connections end.
    pub fn close(&self) {
        let mut nodes = self.lan.0.lock().unwrap();
        if let Some(me) = nodes.0.remove(&self.id) {
            me.leave();
            let id = self.id;
            nodes.tell_others(id, || Discovery::Lost(id));
        }
        // Its `incoming` sender went with it, so `accept` now returns `None`.
    }
}

/// What both ends of one connection share.
struct Link {
    /// Ended for both sides.
    closed: watch::Sender<bool>,
    /// Ended for one side only (`Lan::drop_out`), by side.
    cut: [watch::Sender<bool>; 2],
}

impl Link {
    fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// Ends the connection for both sides, unless `side` has dropped out: then the other
    /// side never hears of it.
    fn close_from(&self, side: usize) {
        if !*self.cut[side].borrow() {
            self.closed.send_replace(true);
        }
    }

    fn is_down(&self, side: usize) -> bool {
        self.is_closed() || *self.cut[side].borrow()
    }

    /// Resolves once the connection has ended for `side`.
    async fn down(&self, side: usize) {
        let (mut closed, mut cut) = (self.closed.subscribe(), self.cut[side].subscribe());
        tokio::select! {
            _ = closed.wait_for(|c| *c) => {}
            _ = cut.wait_for(|c| *c) => {}
        }
    }
}

/// One side's handles; when the last is dropped, the connection closes, as in QUIC.
struct Handle(Arc<Link>, usize);

impl Drop for Handle {
    fn drop(&mut self) {
        self.0.close_from(self.1);
    }
}

/// Streams the other side opened, waiting to be accepted.
type Streams = mpsc::UnboundedReceiver<(Send, Recv)>;

#[derive(Clone)]
pub(crate) struct Conn {
    remote: EndpointId,
    link: Arc<Link>,
    side: usize,
    to_peer: mpsc::UnboundedSender<(Send, Recv)>,
    from_peer: Arc<tokio::sync::Mutex<Streams>>,
    _handle: Arc<Handle>,
}

impl Conn {
    /// Both ends of a new connection from `a` (side 0) to `b` (side 1).
    fn pair(a: EndpointId, b: EndpointId) -> (Conn, Conn) {
        let link = Arc::new(Link {
            closed: watch::Sender::new(false),
            cut: [watch::Sender::new(false), watch::Sender::new(false)],
        });
        let (a_to_b, b_from_a) = mpsc::unbounded_channel();
        let (b_to_a, a_from_b) = mpsc::unbounded_channel();
        let end = |remote, side, to_peer, from_peer| Conn {
            remote,
            link: link.clone(),
            side,
            to_peer,
            from_peer: Arc::new(tokio::sync::Mutex::new(from_peer)),
            _handle: Arc::new(Handle(link.clone(), side)),
        };
        (end(b, 0, a_to_b, a_from_b), end(a, 1, b_to_a, b_from_a))
    }

    pub fn remote_id(&self) -> EndpointId {
        self.remote
    }

    pub fn open_bi(&self) -> Result<(Send, Recv)> {
        if self.link.is_down(self.side) {
            bail!("connection lost");
        }
        let theirs = 1 - self.side;
        let (mine_out, theirs_in) = self.pipe(self.side, theirs);
        let (theirs_out, mine_in) = self.pipe(theirs, self.side);
        if self.to_peer.send((theirs_out, theirs_in)).is_err() {
            bail!("connection lost");
        }
        Ok((mine_out, mine_in))
    }

    /// One direction of a stream, from side `from` to side `to`.
    fn pipe(&self, from: usize, to: usize) -> (Send, Recv) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (reading, stopped) = oneshot::channel();
        let send = Send { tx: Some(tx), stopped, link: self.link.clone(), side: from };
        let recv =
            Recv { rx, pending: Vec::new(), _reading: reading, link: self.link.clone(), side: to };
        (send, recv)
    }

    pub async fn accept_bi(&self) -> Result<(Send, Recv)> {
        let mut from_peer = self.from_peer.lock().await;
        tokio::select! {
            biased;
            _ = self.link.down(self.side) => bail!("connection lost"),
            streams = from_peer.recv() => streams.ok_or_else(|| anyhow!("connection lost")),
        }
    }

    pub fn close(&self) {
        self.link.close_from(self.side);
    }

    pub async fn closed(&self) {
        self.link.down(self.side).await
    }
}

pub(crate) struct Send {
    /// `None` once finished.
    tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    /// Resolves (as an error) when the reading side is dropped.
    stopped: oneshot::Receiver<()>,
    link: Arc<Link>,
    side: usize,
}

impl Send {
    pub fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        if self.link.is_down(self.side) {
            bail!("connection lost");
        }
        let Some(tx) = &self.tx else { bail!("stream already finished") };
        if buf.is_empty() {
            return Ok(());
        }
        if tx.send(buf.to_vec()).is_err() {
            bail!("the other side stopped reading");
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        match self.tx.take() {
            Some(_) => Ok(()),
            None => bail!("stream already finished"),
        }
    }

    pub async fn stopped(&mut self) {
        tokio::select! {
            _ = &mut self.stopped => {}
            _ = self.link.down(self.side) => {}
        }
    }
}

pub(crate) struct Recv {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    /// Bytes received but not yet read.
    pending: Vec<u8>,
    /// Dropped with this side, which is what `Send::stopped` waits for.
    _reading: oneshot::Sender<()>,
    link: Arc<Link>,
    side: usize,
}

impl Recv {
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>> {
        if self.pending.is_empty() {
            tokio::select! {
                biased;
                _ = self.link.down(self.side) => bail!("connection lost"),
                chunk = self.rx.recv() => match chunk {
                    Some(chunk) => self.pending = chunk,
                    // A sender that dropped out can't be heard finishing: just silence.
                    None if *self.link.cut[1 - self.side].borrow() => {
                        self.link.down(self.side).await;
                        bail!("connection lost");
                    }
                    None => return Ok(None),
                },
            }
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(Some(n))
    }

    pub async fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        let mut filled = 0;
        while filled < buf.len() {
            match self.read(&mut buf[filled..]).await? {
                Some(n) => filled += n,
                None => bail!("stream ended early"),
            }
        }
        Ok(())
    }
}
