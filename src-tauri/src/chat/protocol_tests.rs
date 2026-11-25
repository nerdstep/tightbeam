//! The protocol between two engines, on the in-memory network with paused time: pairing,
//! sessions, catch-up, receipts, presence and files. Each test drives `Core` the way the
//! app does, through its public methods, and watches what it reports.
//!
//! `two_peers.rs` checks the same engine on the real network.

use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::*;
use crate::{db::FileState, net::Lan};

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

    fn find(&self, at: &Ref) -> Option<Message> {
        self.messages().unwrap().into_iter().find(|m| m.author == at.author && m.seq == at.seq)
    }

    fn reactions(&self, at: &Ref) -> Vec<String> {
        self.find(at).unwrap().reactions.into_iter().map(|r| r.emoji).collect()
    }

    fn count(&self, matches: impl Fn(&Event) -> bool) -> usize {
        self.events.lock().unwrap().iter().filter(|e| matches(e)).count()
    }

    /// Quits the app, and waits until its last task has let go of it (and its log).
    async fn stop(self) {
        self.core.shutdown().await;
        until("the app has stopped", || Arc::strong_count(&self.core) == 1).await;
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

// ---- Catch-up ------------------------------------------------------------------------

#[test]
fn only_entries_from_after_pairing_are_shared() {
    let (me, peer) = ("me", "carol");
    let at = |author: &str, seq| Ref { author: author.into(), seq };
    let entry = |seq, kind| Entry { kind, ..Entry::message(me, seq, 1, "x") };
    let after = 4;
    assert!(!shares(&Entry::message(me, 4, 1, "for bob"), after, peer));
    assert!(shares(&Entry::message(me, 5, 1, "for carol"), after, peer));
    // About her messages, or mine since: shared.
    assert!(shares(&entry(5, Kind::Reaction { to: at(peer, 1) }), after, peer));
    assert!(shares(&entry(6, Kind::Edit { of: at(me, 5) }), after, peer));
    // About mine from before, or bob's: kept here.
    assert!(!shares(&entry(6, Kind::Edit { of: at(me, 3) }), after, peer));
    assert!(!shares(&entry(6, Kind::Delete { of: at(me, 3) }), after, peer));
    assert!(!shares(&entry(6, Kind::Reaction { to: at("bob", 2) }), after, peer));
    // A reply's text is new, whatever it quotes.
    let reply = Kind::Message { reply_to: Some(at("bob", 2)), vanish: None, file: None };
    assert!(shares(&entry(6, reply), after, peer));
}

#[tokio::test(start_paused = true)]
async fn a_new_peer_gets_only_what_is_written_after_pairing() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    let for_bob = alice.send_message("a secret for bob", None, None).unwrap();
    let from_bob = bob.send_message("one for alice", None, None).unwrap();
    until("both arrive", || alice.texts().len() == 2 && bob.texts().len() == 2).await;
    alice.unpair().unwrap();
    bob.stop().await;

    let carol = world.start("carol", "Carol").await;
    world.pair(&alice, &carol).await;
    // Alice changes her old conversation: it stays on her computer.
    let old = Ref { author: alice.id(), seq: for_bob.seq };
    alice.edit(old.clone(), "still a secret").unwrap();
    let bobs = Ref { author: from_bob.author.clone(), seq: from_bob.seq };
    assert!(alice.react(bobs, "👍").is_err(), "bob's messages are no one's to react to now");
    alice.delete(old.clone()).unwrap();
    assert!(alice.find(&old).unwrap().deleted, "done here");

    let hi = alice.send_message("hi carol", None, None).unwrap();
    until("carol gets it", || carol.texts() == ["hi carol"]).await;
    let to_carol = Ref { author: alice.id(), seq: hi.seq };
    alice.edit(to_carol.clone(), "hi carol!").unwrap();
    until("and the edit", || carol.find(&to_carol).is_some_and(|m| m.edited)).await;

    let reached: Vec<_> = carol.db().since(&alice.id(), 0).unwrap();
    assert_eq!(reached.len(), 2, "the message and its edit, nothing from before: {reached:?}");
    until("delivered", || alice.status().peer_has == hi.seq + 1).await;
}

#[tokio::test(start_paused = true)]
async fn messages_keep_their_author_s_name_after_pairing_someone_new() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    let names = |peer: &Peer| -> Vec<(String, String)> {
        peer.status().people.into_iter().map(|p| (p.id, p.name)).collect()
    };
    assert_eq!(names(&alice), [(bob.id(), "Bob".to_string())]);
    bob.set_name("Robert").unwrap();
    until("alice hears his new name", || names(&alice) == [(bob.id(), "Robert".into())]).await;

    alice.unpair().unwrap();
    let carol = world.start("carol", "Carol").await;
    world.pair(&alice, &carol).await;
    let mut expected = vec![(bob.id(), "Robert".to_string()), (carol.id(), "Carol".to_string())];
    expected.sort();
    assert_eq!(names(&alice), expected, "bob's name stays for his messages");

    // And it lasts across a restart.
    alice.stop().await;
    let alice = world.start("alice", "Alice").await;
    assert_eq!(names(&alice), expected);
}

#[tokio::test(start_paused = true)]
async fn pairing_again_with_the_same_key_still_catches_up() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    alice.send_message("before", None, None).unwrap();
    until("bob gets it", || bob.texts() == ["before"]).await;
    bob.stop().await;
    until("alice notices", || !alice.status().connected).await;
    alice.send_message("while he was off", None, None).unwrap();

    // He asks again (say, from the pairing screen); she already trusts his key.
    let bob = world.start("bob", "Bob").await;
    until("reconnected", || alice.status().connected && bob.status().connected).await;
    until("he catches up", || bob.texts() == ["before", "while he was off"]).await;
    assert_eq!(alice.state().shared_after, 0, "the first pairing's floor stands");
}

#[tokio::test(start_paused = true)]
async fn catch_up_after_being_offline() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    alice.send_message("hi bob", None, None).unwrap();
    until("bob gets it", || bob.texts() == ["hi bob"]).await;
    bob.mark_seen().unwrap();
    until("alice sees he saw it", || alice.status().peer_seen == 1).await;

    // Bob is off; Alice writes twice and reacts to her own message.
    bob.stop().await;
    until("alice notices", || !alice.status().connected).await;
    alice.send_message("you there?", None, None).unwrap();
    alice.send_message("guess not", None, None).unwrap();
    let first = Ref { author: alice.id(), seq: 1 };
    alice.react(first.clone(), "👍").unwrap();
    assert_eq!(alice.status().peer_has, 1, "not delivered while he's off");

    let bob = world.start("bob", "Bob").await;
    until("reconnected", || alice.status().connected && bob.status().connected).await;
    until("alice sees it all delivered", || alice.status().peer_has == 4).await;
    assert_eq!(bob.texts(), ["hi bob", "you there?", "guess not"]);
    assert_eq!(bob.reactions(&first), ["👍"]);
    assert_eq!(alice.status().peer_seen, 1, "seen survives the restart, and doesn't grow");
    bob.mark_seen().unwrap();
    until("alice sees he saw it all", || alice.status().peer_seen == 4).await;
}

#[tokio::test(start_paused = true)]
async fn sessions_coming_and_going_never_duplicate() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    let (dialer, other) = dialer_first(&alice, &bob);
    for round in 0..3 {
        dialer.send_message(&format!("from the dialer {round}"), None, None).unwrap();
        other.send_message(&format!("from the other {round}"), None, None).unwrap();
        world.lan.drop_out(dialer.me);
        until("the dialer notices", || !dialer.status().connected).await;
        // Written while the dialer is off: it arrives with the catch-up.
        other.send_message(&format!("while it was off {round}"), None, None).unwrap();
        until("back", || dialer.status().connected).await;
    }
    until("all arrive", || dialer.texts().len() == 9 && other.texts().len() == 9).await;
    assert_eq!(dialer.texts(), other.texts());
    let arrivals = |peer: &Peer| peer.count(|e| matches!(e, Event::Message(_)));
    assert_eq!((arrivals(dialer), arrivals(other)), (9, 9), "each message shown once");
    until("all delivered", || dialer.status().peer_has == 3 && other.status().peer_has == 6).await;
}

/// The log's files in `folder`, the way a backup would copy them.
fn copy_log(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_peer_that_lost_history_gets_it_again() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    alice.send_message("one", None, None).unwrap();
    until("bob gets it", || bob.texts() == ["one"]).await;
    bob.mark_seen().unwrap();
    until("seen", || alice.status().peer_seen == 1).await;
    bob.stop().await;
    let (folder, backup) = (world.root.join("bob"), world.root.join("backup"));
    copy_log(&folder, &backup);

    let bob = world.start("bob", "Bob").await;
    alice.send_message("two", None, None).unwrap();
    alice.send_message("three", None, None).unwrap();
    until("bob gets them", || bob.texts().len() == 3).await;
    bob.mark_seen().unwrap();
    until("seen", || alice.status().peer_seen == 3).await;

    // Bob restores the backup from when he had only the first.
    bob.stop().await;
    std::fs::remove_dir_all(&folder).unwrap();
    copy_log(&backup, &folder);
    let bob = world.start("bob", "Bob").await;
    until("he has them all again", || bob.texts() == ["one", "two", "three"]).await;
    until("delivered again", || alice.status().peer_has == 3).await;
    assert_eq!(alice.status().peer_seen, 1, "he hasn't seen the ones he lost");
}

// ---- Vanishing messages --------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_vanishing_message_lasts_from_when_it_is_seen() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    let sent = alice.send_message("the code is 1234", None, Some(5)).unwrap();
    let at = Ref { author: alice.id(), seq: sent.seq };
    until("bob gets it", || bob.find(&at).is_some()).await;

    // Unseen, it stays, however long.
    tokio::time::sleep(Duration::from_secs(3600)).await;
    let waiting = bob.find(&at).unwrap();
    assert_eq!((waiting.text.as_str(), waiting.expires), ("the code is 1234", None));

    bob.mark_seen().unwrap();
    until("both clocks run", || {
        alice.find(&at).unwrap().expires.is_some() && bob.find(&at).unwrap().expires.is_some()
    })
    .await;
    within(7, "it vanishes on both sides", || {
        alice.find(&at).unwrap().vanished && bob.find(&at).unwrap().vanished
    })
    .await;
    assert!(alice.find(&at).unwrap().text.is_empty() && bob.find(&at).unwrap().text.is_empty());
}

// ---- Other builds --------------------------------------------------------------------

/// Another build of the app on the network, played by the test frame by frame.
struct Played {
    id: String,
    conn: Conn,
    send: SendStream,
    recv: RecvStream,
    _net: Network,
}

impl Played {
    /// Asks `peer` to pair (they accept), then says `hello`.
    async fn pair(world: &World, peer: &Peer, hello: Frame) -> Self {
        let net = world.lan.join(SecretKey::generate());
        let id = net.id().to_string();
        let conn = net.connect(peer.me).await.unwrap();
        let (mut send, recv) = conn.open_bi().await.unwrap();
        write_frame(&mut send, &Frame::PairRequest { name: "Old".into() }).await.unwrap();
        until("they're asked", || peer.status().pairing.is_some()).await;
        peer.answer_pair(true);
        let mut played = Self { id, conn, send, recv, _net: net };
        played.next("their answer", |f| matches!(f, Frame::PairAccept { .. })).await;
        played.next("their hello", |f| matches!(f, Frame::Hello { .. })).await;
        played.say(&hello).await;
        until("connected", || peer.status().connected).await;
        played
    }

    async fn say(&mut self, frame: &Frame) {
        write_frame(&mut self.send, frame).await.unwrap();
    }

    /// An entry in my log, as a `msg` frame.
    async fn write(&mut self, entry: Entry) {
        self.say(&Frame::Msg(entry.into_wire().unwrap())).await;
    }

    /// Reads frames until one matches.
    async fn next(&mut self, what: &str, wanted: impl Fn(&Frame) -> bool) -> Frame {
        let read = async {
            loop {
                let frame = read_frame(&mut self.recv).await.unwrap();
                if wanted(&frame) {
                    return frame;
                }
            }
        };
        timeout(Duration::from_secs(60), read).await.unwrap_or_else(|_| panic!("no {what}"))
    }

    fn at(&self, seq: u64) -> Ref {
        Ref { author: self.id.clone(), seq }
    }
}

fn hello_from(version: Option<&str>, features: &[&str]) -> Frame {
    let features = features.iter().map(|f| f.to_string()).collect();
    Frame::Hello { name: "Old".into(), have: 0, version: version.map(Into::into), features }
}

fn picture() -> NewFile {
    NewFile {
        name: "sunset.png".into(),
        mime: "image/png".into(),
        width: Some(1600),
        height: Some(900),
        preview: None,
        text: "look at this".into(),
        reply_to: None,
    }
}

#[tokio::test(start_paused = true)]
async fn a_peer_from_before_0_6_gets_neither_vanishing_messages_nor_files() {
    let world = World::new();
    let alice = world.start("alice", "Alice").await;
    let mut old = Played::pair(&world, &alice, hello_from(None, &[])).await;

    let refused = alice.send_message("psst", None, Some(60)).unwrap_err().to_string();
    assert!(refused.contains("0.6 or later"), "{refused}");
    let refused = alice.send_file(b"png", picture()).unwrap_err().to_string();
    assert!(refused.contains("0.7 or later"), "{refused}");
    alice.send_message("plain is fine", None, None).unwrap();
    let wanted = |f: &Frame| matches!(f, Frame::Msg(w) if w.text == "plain is fine");
    old.next("the message", wanted).await;
}

#[tokio::test(start_paused = true)]
async fn a_peer_on_0_6_gets_vanishing_messages_but_not_files() {
    let world = World::new();
    let alice = world.start("alice", "Alice").await;
    let mut old = Played::pair(&world, &alice, hello_from(Some("0.6.0"), &[])).await;

    alice.send_message("psst", None, Some(60)).unwrap();
    let vanishing = |f: &Frame| matches!(f, Frame::Msg(w) if w.vanish == Some(60));
    old.next("the vanishing message", vanishing).await;
    let refused = alice.send_file(b"png", picture()).unwrap_err().to_string();
    assert!(refused.contains("0.7 or later"), "{refused}");
}

#[tokio::test(start_paused = true)]
async fn frames_from_a_newer_build_are_skipped() {
    let world = World::new();
    let alice = world.start("alice", "Alice").await;
    let mut newer = Played::pair(&world, &alice, hello_from(Some("9.0.0"), &["files"])).await;

    let pin = serde_json::json!({"t": "pin", "to": {"author": "x", "seq": 1}});
    write_json(&mut newer.send, &pin).await.unwrap();
    newer.write(Entry::message(newer.id.clone(), 1, EPOCH_MS, "after the pin")).await;
    until("the message after it arrives", || alice.texts() == ["after the pin"]).await;
    assert!(alice.status().connected, "the session carried on");
}

// ---- Files ---------------------------------------------------------------------------

/// Three megabytes that aren't all the same.
fn photo_bytes() -> Vec<u8> {
    (0..3_000_000u32).map(|i| (i * 7 % 251) as u8).collect()
}

fn info_for(bytes: &[u8]) -> FileInfo {
    FileInfo {
        hash: blake3::hash(bytes).to_hex().to_string(),
        name: "sunset.png".into(),
        size: bytes.len() as u64,
        mime: "image/png".into(),
        width: None,
        height: None,
        preview: None,
    }
}

fn with_file(author: &str, seq: u64, file: FileInfo) -> Entry {
    let kind = Kind::Message { reply_to: None, vanish: None, file: Some(file) };
    Entry { kind, ..Entry::message(author, seq, EPOCH_MS, "") }
}

fn file_state(peer: &Peer, at: &Ref) -> Option<FileState> {
    peer.find(at).and_then(|m| m.file).map(|f| f.state)
}

/// Where `folder`'s app keeps its files.
fn files_in(world: &World, folder: &str) -> PathBuf {
    world.root.join(folder).join("files")
}

#[tokio::test(start_paused = true)]
async fn a_picture_arrives_after_its_message() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    let bytes = photo_bytes();
    let sent = alice.send_file(&bytes, picture()).unwrap();
    assert_eq!(sent.file.as_ref().unwrap().state, FileState::Done, "the sender has it");
    let at = Ref { author: alice.id(), seq: sent.seq };
    until("bob has it", || file_state(&bob, &at) == Some(FileState::Done)).await;
    let (path, mime) = bob.file(&sent.file.unwrap().info.hash).unwrap().unwrap();
    assert_eq!((std::fs::read(&path).unwrap(), mime.as_str()), (bytes, "image/png"));
    assert_eq!(path.file_name().unwrap(), "sunset.png");
    assert_eq!(bob.find(&at).unwrap().text, "look at this");
}

#[tokio::test(start_paused = true)]
async fn a_transfer_resumes_where_it_broke_off() {
    let world = World::new();
    let alice = world.start("alice", "Alice").await;
    let mut sender = Played::pair(&world, &alice, hello_from(Some("0.7.0"), &["files"])).await;
    let bytes = photo_bytes();
    let info = info_for(&bytes);
    // Alice already has the first megabyte, from a transfer that broke off.
    let files = files_in(&world, "alice");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join(format!("{}.part", info.hash)), &bytes[..1 << 20]).unwrap();

    let me = sender.id.clone();
    sender.write(with_file(&me, 1, info.clone())).await;
    let (mut send, mut recv) = sender.conn.accept_bi().await.unwrap();
    let request: FileRequest = read_json(&mut recv).await.unwrap();
    assert_eq!((request.hash.as_str(), request.from), (info.hash.as_str(), 1 << 20));
    write_json(&mut send, &FileReply { found: true }).await.unwrap();
    send.write_all(&bytes[1 << 20..]).await.unwrap();
    send.finish().unwrap();

    let at = sender.at(1);
    until("alice has it", || file_state(&alice, &at) == Some(FileState::Done)).await;
    let (path, _) = alice.file(&info.hash).unwrap().unwrap();
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[tokio::test(start_paused = true)]
async fn a_file_that_does_not_match_its_hash_is_not_kept() {
    let world = World::new();
    let alice = world.start("alice", "Alice").await;
    let mut sender = Played::pair(&world, &alice, hello_from(Some("0.7.0"), &["files"])).await;
    let bytes = photo_bytes();
    let info = info_for(&bytes);

    let me = sender.id.clone();
    sender.write(with_file(&me, 1, info.clone())).await;
    let (mut send, mut recv) = sender.conn.accept_bi().await.unwrap();
    let _: FileRequest = read_json(&mut recv).await.unwrap();
    write_json(&mut send, &FileReply { found: true }).await.unwrap();
    send.write_all(&vec![0; bytes.len()]).await.unwrap(); // the right size, the wrong bytes
    send.finish().unwrap();
    send.stopped().await; // alice has read it all and given up

    let at = sender.at(1);
    assert_eq!(file_state(&alice, &at), Some(FileState::Pending), "still to come");
    assert!(alice.file(&info.hash).unwrap().is_none());
    let part = files_in(&world, "alice").join(format!("{}.part", info.hash));
    assert!(!part.exists(), "nothing half-kept");
}

#[tokio::test(start_paused = true)]
async fn a_file_deleted_on_its_way_is_not_kept() {
    let world = World::new();
    let alice = world.start("alice", "Alice").await;
    let mut sender = Played::pair(&world, &alice, hello_from(Some("0.7.0"), &["files"])).await;
    let bytes = photo_bytes();
    let info = info_for(&bytes);
    let me = sender.id.clone();

    sender.write(with_file(&me, 1, info.clone())).await;
    let (mut send, mut recv) = sender.conn.accept_bi().await.unwrap();
    let _: FileRequest = read_json(&mut recv).await.unwrap();
    write_json(&mut send, &FileReply { found: true }).await.unwrap();
    send.write_all(&bytes[..1 << 20]).await.unwrap();

    // Halfway through, the sender deletes the message.
    let at = sender.at(1);
    let delete = Kind::Delete { of: at.clone() };
    sender.write(Entry { kind: delete, ..Entry::message(&me, 2, EPOCH_MS, "") }).await;
    until("alice sees the delete", || alice.find(&at).unwrap().deleted).await;
    send.write_all(&bytes[1 << 20..]).await.unwrap();
    send.finish().unwrap();
    send.stopped().await;

    assert!(alice.file(&info.hash).unwrap().is_none());
    let files = files_in(&world, "alice");
    assert!(!files.join(&info.hash).exists(), "not kept");
    assert!(!files.join(format!("{}.part", info.hash)).exists(), "nor half-kept");
}

#[tokio::test(start_paused = true)]
async fn a_file_the_sender_no_longer_has_ends_up_gone() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    bob.stop().await;
    until("alice notices", || !alice.status().connected).await;

    let sent = alice.send_file(&photo_bytes(), picture()).unwrap();
    let hash = sent.file.unwrap().info.hash;
    let (path, _) = alice.file(&hash).unwrap().unwrap();
    std::fs::remove_file(path).unwrap(); // tidied away by hand

    let bob = world.start("bob", "Bob").await;
    let at = Ref { author: alice.id(), seq: sent.seq };
    until("both give up", || {
        file_state(&bob, &at) == Some(FileState::Gone)
            && file_state(&alice, &at) == Some(FileState::Gone)
    })
    .await;
}

// ---- Changing messages ---------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn edits_and_deletes_reach_the_other_side() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    let sent = alice.send_message("hi bob", None, None).unwrap();
    let at = Ref { author: alice.id(), seq: sent.seq };
    until("bob gets it", || bob.find(&at).is_some()).await;

    alice.edit(at.clone(), "hi bob!").unwrap();
    until("bob sees the edit", || bob.find(&at).is_some_and(|m| m.edited && m.text == "hi bob!"))
        .await;
    let refused = bob.edit(at.clone(), "hi alice").unwrap_err().to_string();
    assert!(refused.contains("only change your own"), "{refused}");
    assert!(bob.delete(at.clone()).is_err(), "nor delete it");

    alice.delete(at.clone()).unwrap();
    until("bob sees the delete", || bob.find(&at).is_some_and(|m| m.deleted)).await;
    for side in [&alice, &bob] {
        let gone = side.find(&at).unwrap();
        assert!(gone.text.is_empty() && !gone.edited, "blanked on both sides");
    }
    assert!(alice.edit(at.clone(), "back").is_err(), "a deleted message stays deleted");
}

#[tokio::test(start_paused = true)]
async fn a_reply_quotes_its_message_on_both_sides() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    let first = alice.send_message("lunch?", None, None).unwrap();
    let to = Ref { author: alice.id(), seq: first.seq };
    until("bob gets it", || bob.find(&to).is_some()).await;

    let reply = bob.send_message("yes", Some(to.clone()), None).unwrap();
    assert_eq!(reply.reply_to.unwrap().text.as_deref(), Some("lunch?"));
    let at = Ref { author: bob.id(), seq: reply.seq };
    until("alice gets the reply", || alice.find(&at).is_some()).await;
    let quote = alice.find(&at).unwrap().reply_to.unwrap();
    assert_eq!(
        (quote.author, quote.seq, quote.text.as_deref()),
        (to.author, to.seq, Some("lunch?"))
    );

    let stranger = Ref { author: "someone else".into(), seq: 1 };
    assert!(bob.send_message("eh", Some(stranger), None).is_err(), "only to the two of us");
}

#[tokio::test(start_paused = true)]
async fn clearing_one_side_leaves_the_other_and_numbering_carries_on() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    alice.send_message("one", None, None).unwrap();
    bob.send_message("two", None, None).unwrap();
    until("both have both", || alice.texts().len() == 2 && bob.texts().len() == 2).await;

    bob.clear_history().unwrap();
    assert!(bob.texts().is_empty());
    assert_eq!(alice.texts().len(), 2, "her copy stays");
    let next = bob.send_message("fresh start", None, None).unwrap();
    assert_eq!(next.seq, 2, "his numbering carries on");
    until("alice gets it", || alice.texts().len() == 3).await;
    assert_eq!(bob.texts(), ["fresh start"]);

    // Catch-up never sends him the cleared messages again.
    world.lan.drop_out(dialer_first(&alice, &bob).0.me);
    until("reconnected", || !alice.status().connected || !bob.status().connected).await;
    until("back", || alice.status().connected && bob.status().connected).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(bob.texts(), ["fresh start"]);
}

// ---- Presence ------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn typing_reaches_the_other_side() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    alice.typing();
    until("bob sees it", || bob.count(|e| matches!(e, Event::Typing)) == 1).await;
    assert_eq!(alice.count(|e| matches!(e, Event::Typing)), 0, "not on her own screen");
}

#[tokio::test(start_paused = true)]
async fn nudges_are_rate_limited_each_way() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    let refused = alice.nudge().unwrap_err().to_string();
    assert!(refused.contains("Not connected"), "{refused}");
    world.pair(&alice, &bob).await;
    let nudges = |peer: &Peer| peer.count(|e| matches!(e, Event::Nudge));

    alice.nudge().unwrap();
    until("bob is nudged", || nudges(&bob) == 1).await;
    let refused = alice.nudge().unwrap_err().to_string();
    assert!(refused.contains("just nudged"), "{refused}");
    tokio::time::sleep(NUDGE_EVERY).await;
    alice.nudge().unwrap();
    until("bob is nudged again", || nudges(&bob) == 2).await;

    // A build that doesn't limit them still only gets through every few seconds.
    let carol = world.start("carol", "Carol").await;
    let mut eager = Played::pair(&world, &carol, hello_from(Some("9.0.0"), &[])).await;
    eager.say(&Frame::Nudge).await;
    eager.say(&Frame::Nudge).await;
    until("carol is nudged", || nudges(&carol) >= 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(nudges(&carol), 1, "the second came too soon");
    tokio::time::sleep(NUDGE_EVERY).await;
    eager.say(&Frame::Nudge).await;
    until("carol is nudged again", || nudges(&carol) == 2).await;
}

#[tokio::test(start_paused = true)]
async fn away_by_hand_lasts_until_back_or_a_message() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    let lunch = Some(Away { note: Some("lunch break".into()) });

    bob.set_away("  lunch\n  break ");
    until("alice sees it", || alice.status().peer_away == lunch).await;
    assert_eq!(bob.status().away, lunch, "his own screen says so");
    bob.set_idle(false); // using the computer doesn't end it
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!((bob.status().away, alice.status().peer_away), (lunch.clone(), lunch.clone()));

    bob.send_message("back now", None, None).unwrap();
    until("his message brings him back", || alice.status().peer_away.is_none()).await;
    assert_eq!(bob.status().away, None);

    bob.set_away("");
    until("away without a note", || alice.status().peer_away == Some(Away::default())).await;
    bob.back();
    until("back", || alice.status().peer_away.is_none()).await;
}

#[tokio::test(start_paused = true)]
async fn idle_is_away_until_input_and_ends_with_the_session() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;

    bob.set_idle(true);
    until("alice sees him idle", || alice.status().peer_away == Some(Away::default())).await;
    assert_eq!(bob.status().away, None, "his own screen doesn't show idle");
    bob.set_idle(false);
    until("alice sees him back", || alice.status().peer_away.is_none()).await;

    // What she knows of him lasts only as long as the session.
    bob.set_away("gone fishing");
    until("alice sees it", || alice.status().peer_away.is_some()).await;
    bob.stop().await;
    until("disconnected", || !alice.status().connected).await;
    assert_eq!(alice.status().peer_away, None);
}

#[tokio::test(start_paused = true)]
async fn do_not_disturb_pauses_nudges_until_it_ends() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;

    let later = world.clock.now_ms() + 3_600_000;
    bob.set_dnd(Some(later)).unwrap();
    assert!(bob.dnd() && bob.status().dnd_until == Some(later));
    until("alice sees it", || alice.status().peer_dnd_until == Some(later)).await;
    let refused = alice.nudge().unwrap_err().to_string();
    assert!(refused.contains("do not disturb"), "{refused}");

    // It lasts across a restart, and he says so again when they reconnect.
    bob.stop().await;
    until("disconnected", || !alice.status().connected).await;
    assert_eq!(alice.status().peer_dnd_until, None, "only while connected");
    let bob = world.start("bob", "Bob").await;
    assert!(bob.dnd());
    until("alice sees it again", || alice.status().peer_dnd_until == Some(later)).await;

    bob.set_dnd(None).unwrap();
    until("alice sees it end", || alice.status().peer_dnd_until.is_none()).await;
    alice.nudge().unwrap();
}

#[tokio::test(start_paused = true)]
async fn do_not_disturb_ends_on_its_own() {
    let world = World::new();
    let bob = world.start("bob", "Bob").await;
    bob.set_dnd(Some(world.clock.now_ms() + 60_000)).unwrap();
    assert!(bob.dnd());
    tokio::time::sleep(Duration::from_secs(61)).await;
    assert!(!bob.dnd() && bob.status().dnd_until.is_none());
    assert!(bob.set_dnd(Some(world.clock.now_ms() - 1)).is_ok(), "a time past turns it off");
    assert!(!bob.dnd());
}

#[tokio::test(start_paused = true)]
async fn a_new_pairing_starts_clean() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    alice.send_message("hi", None, None).unwrap();
    bob.send_message("hi yourself", None, None).unwrap();
    until("both arrive", || alice.texts().len() == 2 && bob.texts().len() == 2).await;
    alice.mark_seen().unwrap();
    bob.mark_seen().unwrap();
    until("receipts", || alice.status().peer_seen == 1).await;
    alice.unpair().unwrap();
    bob.stop().await;

    let clean = |alice: &Peer| {
        let (status, info) = (alice.status(), alice.connection_info());
        assert!(status.peer.is_none());
        assert_eq!((status.peer_has, status.peer_seen), (0, 0));
        assert_eq!(info.peer_version, None);
    };
    clean(&alice);
    alice.stop().await;
    let alice = world.start("alice", "Alice").await;
    clean(&alice); // nothing of bob's came back from the log either

    // Someone new, on an older build: nothing bob's build allowed carries over.
    let mut old = Played::pair(&world, &alice, hello_from(None, &[])).await;
    let seen = old.next("her seen receipt", |f| matches!(f, Frame::Seen { .. })).await;
    assert!(matches!(seen, Frame::Seen { up_to: 0 }), "she's seen nothing of theirs: {seen:?}");
    assert!(alice.send_message("psst", None, Some(60)).is_err(), "no vanishing messages");
    assert!(alice.send_file(b"png", picture()).is_err(), "no files");
    assert_eq!((alice.status().peer_has, alice.status().peer_seen), (0, 0));
}

#[tokio::test(start_paused = true)]
async fn any_file_goes_from_disk_by_path() {
    let world = World::new();
    let (alice, bob) = (world.start("alice", "Alice").await, world.start("bob", "Bob").await);
    world.pair(&alice, &bob).await;
    let bytes = photo_bytes();
    let original = world.root.join("Report.pdf");
    std::fs::write(&original, &bytes).unwrap();
    let new = NewFile {
        name: "Report.pdf".into(),
        mime: "application/pdf".into(),
        width: None,
        height: None,
        preview: None,
        text: "the numbers".into(),
        reply_to: None,
    };
    let sent = alice.send_file_from(&original, new).unwrap();
    let file = sent.file.unwrap();
    assert_eq!((file.state, file.info.size), (FileState::Done, bytes.len() as u64));
    assert!(file.openable, "a PDF opens in its app");

    // Her copy is the store's own: changing the original afterwards changes nothing.
    std::fs::write(&original, b"edited").unwrap();
    let at = Ref { author: alice.id(), seq: sent.seq };
    until("bob has it", || file_state(&bob, &at) == Some(FileState::Done)).await;
    let (path, mime) = bob.file(&file.info.hash).unwrap().unwrap();
    assert_eq!((std::fs::read(&path).unwrap(), mime.as_str()), (bytes, "application/pdf"));
    assert_eq!(path.file_name().unwrap(), "Report.pdf");
    assert_eq!(bob.find(&at).unwrap().text, "the numbers");

    let missing = world.root.join("not there.pdf");
    assert!(alice.send_file_from(&missing, picture()).is_err());
}
