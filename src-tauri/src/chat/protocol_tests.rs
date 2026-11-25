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
async fn until(what: &str, ok: impl FnMut() -> bool) {
    within(60, what, ok).await
}

/// Waits until `ok`, for at most `secs` seconds of test time.
async fn within(secs: u64, what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
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

// ---- Pairing -------------------------------------------------------------------------

fn prompts(peer: &Peer) -> usize {
    peer.count(|e| matches!(e, Event::Status(s) if s.pairing.is_some()))
}

#[tokio::test(start_paused = true)]
async fn pairing_both_screens_show_one_code() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    alice.pair_with(&bob.id()).unwrap();
    until("bob is asked", || bob.status().pairing.is_some()).await;
    let (asked, asking) = (bob.status().pairing.unwrap(), alice.status().pairing.unwrap());
    assert_eq!(asked.code, asking.code);
    assert!(asked.incoming && !asking.incoming);
    assert_eq!((asked.name.as_str(), asked.id), ("Alice", alice.id()));
}

#[tokio::test(start_paused = true)]
async fn pairing_declined() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    alice.pair_with(&bob.id()).unwrap();
    until("bob is asked", || bob.status().pairing.is_some()).await;
    bob.answer_pair(false);
    until("alice hears no", || alice.status().notice.is_some()).await;
    assert_eq!(alice.status().notice.unwrap(), "They declined the pairing request.");
    let (a, b) = (alice.status(), bob.status());
    assert!(a.peer.is_none() && b.peer.is_none() && a.pairing.is_none() && b.pairing.is_none());
}

#[tokio::test(start_paused = true)]
async fn pairing_prompt_times_out() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    alice.pair_with(&bob.id()).unwrap();
    until("bob is asked", || bob.status().pairing.is_some()).await;
    // Nobody answers for two minutes.
    within(130, "the prompt goes", || bob.status().pairing.is_none()).await;
    until("alice hears no", || alice.status().notice.is_some()).await;
    assert!(alice.status().peer.is_none() && bob.status().peer.is_none());
}

#[tokio::test(start_paused = true)]
async fn pairing_cancelled_by_the_asker() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    alice.pair_with(&bob.id()).unwrap();
    until("bob is asked", || bob.status().pairing.is_some()).await;
    alice.answer_pair(false);
    assert!(alice.status().pairing.is_none());
    // Well before the prompt would time out on its own.
    within(5, "bob's prompt goes too", || bob.status().pairing.is_none()).await;
    assert!(bob.status().peer.is_none());
}

#[tokio::test(start_paused = true)]
async fn pairing_again_with_a_trusted_key_needs_no_answer() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    alice.unpair().unwrap();
    until("disconnected", || !bob.status().connected).await;
    assert!(bob.status().peer.is_some(), "bob still trusts alice");

    let asked = prompts(&bob);
    alice.pair_with(&bob.id()).unwrap();
    until("connected again", || alice.status().connected && bob.status().connected).await;
    assert_eq!(prompts(&bob), asked, "bob wasn't asked");
    assert_eq!(alice.status().peer.unwrap().name, "Bob");
}

#[tokio::test(start_paused = true)]
async fn pairing_with_someone_already_paired_is_refused() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    let carol = world.start("carol", "Carol").await;
    let asked = prompts(&bob);
    carol.pair_with(&bob.id()).unwrap();
    until("carol hears no", || carol.status().notice.is_some()).await;
    assert_eq!(prompts(&bob), asked, "bob wasn't asked");
    assert_eq!(bob.status().peer.unwrap().name, "Alice");
    assert!(bob.status().connected, "and stayed connected to alice");
}

// ---- Sessions ------------------------------------------------------------------------

/// The side that dials (the smaller id), then the other.
fn dialer_first<'a>(a: &'a Peer, b: &'a Peer) -> (&'a Peer, &'a Peer) {
    if a.me.as_bytes() < b.me.as_bytes() { (a, b) } else { (b, a) }
}

#[tokio::test(start_paused = true)]
async fn a_new_session_replaces_a_half_dead_one() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    let (dialer, other) = dialer_first(&alice, &bob);
    dialer.set_away("lunch");
    let lunch = Some(Away { note: Some("lunch".into()) });
    until("the other sees it", || other.status().peer_away == lunch).await;

    // The dialer loses its connection; the other side doesn't notice, until the dialer
    // connects again and the new session replaces the old one.
    world.lan.drop_out(dialer.me);
    until("the dialer notices", || !dialer.status().connected).await;
    assert!(other.status().connected, "still thinks it's connected");
    until("the dialer is back", || dialer.status().connected).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Ending the old session didn't end the new one.
    assert!(other.status().connected);
    assert_eq!(other.status().peer_away, lunch, "said again on the new session");
    other.send_message("still there?", None, None).unwrap();
    until("it arrives", || dialer.texts().contains(&"still there?".into())).await;
}
