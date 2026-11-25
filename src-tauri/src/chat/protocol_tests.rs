//! The protocol between two engines, on the in-memory network with paused time: pairing,
//! sessions, catch-up, receipts, presence and files. Each test drives `Core` the way the
//! app does, through its public methods, and watches what it reports.
//!
//! `two_peers.rs` checks the same engine on the real network.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::net::Lan;

/// Where the test clock starts: November 2027.
const EPOCH_MS: i64 = 1_826_000_000_000;

/// Peers on one in-memory LAN, sharing one clock, each with a data folder of its own.
struct World {
    lan: Lan,
    clock: Clock,
    root: PathBuf,
}

impl World {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("tightbeam-protocol-{nanos}-{n}"));
        Self { lan: Lan::default(), clock: Clock::tokio(EPOCH_MS), root }
    }

    /// Starts (or restarts) the app in `folder`, named `name`.
    async fn start(&self, folder: &str, name: &str) -> Peer {
        let dir = self.root.join(folder);
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        let emit: Emit = Arc::new(move |event| seen.lock().unwrap().push(event));
        let lan = self.lan.clone();
        let network = |secret| async move { Ok(lan.join(secret)) };
        let core = Core::start_on(dir, emit, self.clock.clone(), network).await.unwrap();
        core.set_name(name).unwrap();
        Peer { core, events }
    }

    /// Pairs `a` and `b` (`a` asks, `b` accepts) and waits until they're connected.
    async fn pair(&self, a: &Peer, b: &Peer) {
        a.pair_with(&b.id()).unwrap();
        until("the pair request arrives", || b.status().pairing.is_some()).await;
        b.answer_pair(true);
        until("connected", || a.status().connected && b.status().connected).await;
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// One running app, and everything it has told the UI.
struct Peer {
    core: Arc<Core>,
    events: Arc<Mutex<Vec<Event>>>,
}

impl std::ops::Deref for Peer {
    type Target = Arc<Core>;
    fn deref(&self) -> &Arc<Core> {
        &self.core
    }
}

impl Peer {
    fn texts(&self) -> Vec<String> {
        self.messages().unwrap().into_iter().map(|m| m.text).collect()
    }

    fn count(&self, matches: impl Fn(&Event) -> bool) -> usize {
        self.events.lock().unwrap().iter().filter(|e| matches(e)).count()
    }

    async fn stop(self) {
        self.core.shutdown().await;
    }
}

/// Waits until `ok`, letting (paused) time pass. Fails after a minute of test time.
async fn until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while !ok() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(start_paused = true)]
async fn pairs_and_chats() {
    let world = World::new();
    let alice = world.start("alice", "Alice").await;
    let bob = world.start("bob", "Bob").await;

    let bob_id = bob.id();
    until("alice sees bob nearby", || {
        alice.status().nearby.iter().any(|p| p.id == bob_id && p.name == "Bob")
    })
    .await;
    world.pair(&alice, &bob).await;
    assert_eq!(alice.status().peer.unwrap().name, "Bob");
    assert_eq!(bob.status().peer.unwrap().name, "Alice");

    alice.send_message("hi bob", None, None).unwrap();
    until("bob gets it", || bob.texts() == ["hi bob"]).await;
    until("alice sees it delivered", || alice.status().peer_has == 1).await;
    bob.send_message("hi alice", None, None).unwrap();
    until("alice gets his", || alice.texts() == ["hi bob", "hi alice"]).await;
    assert_eq!(bob.count(|e| matches!(e, Event::Message(_))), 2, "his own and hers");

    alice.stop().await;
    until("bob sees her go", || !bob.status().connected).await;
}
