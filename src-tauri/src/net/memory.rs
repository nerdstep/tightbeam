//! An in-memory network for tests. Every node that joins a `Lan` discovers the others
//! by name, connects to them instantly, and streams bytes through channels.
//!
//! Like QUIC, closing a connection is abrupt: reads and writes on its streams fail at
//! once, on both sides. A node that closes (its app shut down) drops off the LAN: the
//! others see it go, and its connections end.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{Result, bail};
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
    /// Every connection this node is part of, to end when it leaves.
    conns: Vec<Arc<watch::Sender<bool>>>,
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
        let joined = Joined { name: String::new(), incoming, watchers: vec![], conns: vec![] };
        nodes.0.insert(id, joined);
        Network::Memory(Node { id, lan: self.clone(), accept: tokio::sync::Mutex::new(accept) })
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
        for conn in self.conns {
            conn.send_replace(true);
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
        let (mine, theirs, closed) = Conn::pair(self.id, peer);
        if them.incoming.send(theirs).is_err() {
            bail!("{peer} isn't accepting connections");
        }
        them.conns.push(closed.clone());
        nodes.0.get_mut(&self.id).unwrap().conns.push(closed);
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

/// Streams the other side opened, waiting to be accepted.
type Streams = mpsc::UnboundedReceiver<(Send, Recv)>;

#[derive(Clone)]
pub(crate) struct Conn {
    remote: EndpointId,
    to_peer: mpsc::UnboundedSender<(Send, Recv)>,
    from_peer: Arc<tokio::sync::Mutex<Streams>>,
    /// `true` once either side closes; shared by both sides and all their streams.
    closer: Arc<watch::Sender<bool>>,
}

impl Conn {
    /// Both ends of a new connection, and the switch that ends it.
    fn pair(a: EndpointId, b: EndpointId) -> (Conn, Conn, Arc<watch::Sender<bool>>) {
        let closer = Arc::new(watch::Sender::new(false));
        let (a_to_b, b_from_a) = mpsc::unbounded_channel();
        let (b_to_a, a_from_b) = mpsc::unbounded_channel();
        let end = |remote, to_peer, from_peer| Conn {
            remote,
            to_peer,
            from_peer: Arc::new(tokio::sync::Mutex::new(from_peer)),
            closer: closer.clone(),
        };
        (end(b, a_to_b, a_from_b), end(a, b_to_a, b_from_a), closer)
    }

    pub fn remote_id(&self) -> EndpointId {
        self.remote
    }

    fn is_closed(&self) -> bool {
        *self.closer.borrow()
    }

    pub fn open_bi(&self) -> Result<(Send, Recv)> {
        if self.is_closed() {
            bail!("connection lost");
        }
        let (mine_out, theirs_in) = pipe(&self.closer);
        let (theirs_out, mine_in) = pipe(&self.closer);
        if self.to_peer.send((theirs_out, theirs_in)).is_err() {
            bail!("connection lost");
        }
        Ok((mine_out, mine_in))
    }

    pub async fn accept_bi(&self) -> Result<(Send, Recv)> {
        let mut closed = self.closer.subscribe();
        let mut from_peer = self.from_peer.lock().await;
        tokio::select! {
            biased;
            _ = closed.wait_for(|c| *c) => bail!("connection lost"),
            streams = from_peer.recv() => streams.ok_or_else(|| anyhow::anyhow!("connection lost")),
        }
    }

    pub fn close(&self) {
        self.closer.send_replace(true);
    }

    pub async fn closed(&self) {
        let _ = self.closer.subscribe().wait_for(|c| *c).await;
    }
}

/// One direction of a stream.
fn pipe(closer: &Arc<watch::Sender<bool>>) -> (Send, Recv) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (read_tx, read_rx) = oneshot::channel();
    let send = Send { tx: Some(tx), stopped: read_rx, closed: closer.subscribe() };
    let recv = Recv { rx, pending: Vec::new(), _reading: read_tx, closed: closer.subscribe() };
    (send, recv)
}

pub(crate) struct Send {
    /// `None` once finished.
    tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    /// Resolves (as an error) when the reading side is dropped.
    stopped: oneshot::Receiver<()>,
    closed: watch::Receiver<bool>,
}

impl Send {
    pub fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        if *self.closed.borrow() {
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
            _ = self.closed.wait_for(|c| *c) => {}
        }
    }
}

pub(crate) struct Recv {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    /// Bytes received but not yet read.
    pending: Vec<u8>,
    /// Dropped with this side, which is what `Send::stopped` waits for.
    _reading: oneshot::Sender<()>,
    closed: watch::Receiver<bool>,
}

impl Recv {
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>> {
        if self.pending.is_empty() {
            tokio::select! {
                biased;
                _ = self.closed.wait_for(|c| *c) => bail!("connection lost"),
                chunk = self.rx.recv() => match chunk {
                    Some(chunk) => self.pending = chunk,
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
