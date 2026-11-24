//! Two real Tightbeam cores on this machine, found via mDNS and talking over QUIC.
//! Covers pairing, live messages, delivery acks, seen receipts, catching up after one
//! side was off, replies and reactions, edits and deletes, typing, nudges, away and back,
//! clearing one side's history, versions, do not disturb, and vanishing messages.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tightbeam_lib::{
    chat::{Away, Core, Emit, Event},
    db::Ref,
};

fn quiet() -> Emit {
    Arc::new(|_| {})
}

/// Counts the nudges and typing signals it's told about.
fn counting(nudges: Arc<AtomicUsize>, typing: Arc<AtomicUsize>) -> Emit {
    Arc::new(move |event| match event {
        Event::Nudge => drop(nudges.fetch_add(1, Ordering::SeqCst)),
        Event::Typing => drop(typing.fetch_add(1, Ordering::SeqCst)),
        _ => {}
    })
}

fn temp_dir(label: &str) -> PathBuf {
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("tightbeam-test-{nanos}-{label}"))
}

async fn wait_for(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Runs `f` on a plain thread outside the tokio runtime, the way Tauri runs sync commands
/// (on the main thread). Catches code that assumes it's inside the runtime.
fn like_tauri<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(f).join().expect("UI action panicked")
}

fn texts(core: &Core) -> Vec<String> {
    core.messages().unwrap().into_iter().map(|m| m.text).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_chat_and_catch_up() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let (dir_a, dir_b) = (temp_dir("alice"), temp_dir("bob"));
    let alice = Core::start_with(dir_a.clone(), quiet(), true).await.unwrap();
    let mut bob = Core::start_with(dir_b.clone(), quiet(), true).await.unwrap();
    let a = alice.clone();
    like_tauri(move || a.set_name("Alice")).unwrap();
    let b = bob.clone();
    like_tauri(move || b.set_name("Bob")).unwrap();

    // Discovery: Alice sees Bob, with his name.
    let bob_id = bob.id();
    wait_for("alice to discover bob", 30, || {
        alice.status().nearby.iter().any(|p| p.id == bob_id && p.name == "Bob")
    })
    .await;

    // Pairing: both screens show the same code, Bob accepts.
    let (a, id) = (alice.clone(), bob_id.clone());
    like_tauri(move || a.pair_with(&id)).unwrap();
    wait_for("bob to get the pair request", 15, || {
        if let Some(n) = alice.status().notice {
            panic!("alice: {n}");
        }
        bob.status().pairing.is_some()
    })
    .await;
    let (code_a, code_b) =
        (alice.status().pairing.unwrap().code, bob.status().pairing.unwrap().code);
    assert_eq!(code_a, code_b, "pairing codes must match");
    assert!(bob.status().pairing.unwrap().incoming);
    let b = bob.clone();
    like_tauri(move || b.answer_pair(true));
    wait_for("both connected", 15, || alice.status().connected && bob.status().connected).await;
    assert_eq!(alice.status().peer.unwrap().name, "Bob");
    assert_eq!(bob.status().peer.unwrap().name, "Alice");

    // Live messages both ways, with delivery confirmation.
    let a = alice.clone();
    like_tauri(move || a.send_message("hi bob", None, None)).unwrap();
    wait_for("bob gets alice's message", 10, || texts(&bob).contains(&"hi bob".into())).await;
    bob.send_message("hi alice", None, None).unwrap();
    wait_for("alice gets bob's message", 10, || texts(&alice).contains(&"hi alice".into())).await;
    wait_for("alice sees delivery", 10, || alice.status().peer_has == 1).await;
    assert_eq!(alice.status().peer_seen, 0, "delivered isn't seen");

    // Bob looks at the chat: Alice's message shows as seen.
    let b = bob.clone();
    like_tauri(move || b.mark_seen()).unwrap();
    wait_for("alice sees bob saw it", 10, || alice.status().peer_seen == 1).await;

    // Bob goes offline; Alice keeps writing.
    bob.shutdown().await;
    drop(bob);
    wait_for("alice notices bob left", 40, || !alice.status().connected).await;
    alice.send_message("you there?", None, None).unwrap();
    alice.send_message("guess not", None, None).unwrap();
    // A reaction while he's away: it syncs with the catch-up.
    let first = Ref { author: bob_id.clone(), seq: 1 };
    alice.react(first.clone(), "👍").unwrap();
    assert_eq!(alice.status().peer_has, 1, "undelivered messages must not show as delivered");

    // Bob comes back with the same identity and catches up.
    let (nudges, typing) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    bob = Core::start_with(dir_b.clone(), counting(nudges.clone(), typing.clone()), true)
        .await
        .unwrap();
    assert_eq!(bob.id(), bob_id, "identity must survive a restart");
    assert_eq!(bob.status().peer.unwrap().name, "Alice", "pairing must survive a restart");
    wait_for("reconnect", 40, || alice.status().connected && bob.status().connected).await;
    wait_for("bob catches up", 10, || {
        let t = texts(&bob);
        t.contains(&"you there?".into()) && t.contains(&"guess not".into())
    })
    .await;
    wait_for("alice sees catch-up delivered", 10, || alice.status().peer_has == 4).await;
    assert_eq!(texts(&bob).len(), 4, "no duplicates after catch-up, and reactions aren't messages");
    let reactions = |core: &Core| {
        let messages = core.messages().unwrap();
        let m = messages.iter().find(|m| m.author == first.author && m.seq == first.seq);
        m.unwrap().reactions.iter().map(|r| r.emoji.clone()).collect::<Vec<_>>()
    };
    wait_for("bob sees the reaction", 10, || reactions(&bob) == ["👍"]).await;
    assert_eq!(alice.status().peer_seen, 1, "seen must survive the reconnect, and not grow");
    let b = bob.clone();
    like_tauri(move || b.mark_seen()).unwrap();
    wait_for("alice sees the catch-up seen", 10, || alice.status().peer_seen == 4).await;

    // Bob replies to Alice's first message. Then she takes her reaction back and he adds one.
    let hi_bob = Ref { author: alice.id(), seq: 1 };
    let b = bob.clone();
    let sent = like_tauri(move || b.send_message("hi again", Some(hi_bob), None)).unwrap();
    assert_eq!(sent.reply_to.unwrap().text.as_deref(), Some("hi bob"));
    wait_for("alice gets the reply", 10, || {
        alice.messages().unwrap().iter().any(|m| {
            m.text == "hi again"
                && m.reply_to.as_ref().is_some_and(|q| q.text.as_deref() == Some("hi bob"))
        })
    })
    .await;
    alice.react(first.clone(), "").unwrap();
    bob.react(first.clone(), "😂").unwrap();
    wait_for("reactions settle on both sides", 10, || {
        reactions(&alice) == ["😂"] && reactions(&bob) == ["😂"]
    })
    .await;

    // Bob edits his reply, then deletes it. Alice sees both, and the text is gone.
    let again = Ref { author: bob.id(), seq: 2 };
    bob.edit(again.clone(), "hi once more").unwrap();
    let find = |core: &Core, at: &Ref| {
        core.messages().unwrap().into_iter().find(|m| m.author == at.author && m.seq == at.seq)
    };
    wait_for("alice sees the edit", 10, || {
        find(&alice, &again).is_some_and(|m| m.edited && m.text == "hi once more")
    })
    .await;
    assert!(alice.edit(again.clone(), "not mine").is_err(), "only the author can edit");
    bob.delete(again.clone()).unwrap();
    wait_for("alice sees the delete", 10, || find(&alice, &again).is_some_and(|m| m.deleted)).await;

    // Typing reaches Bob.
    let a = alice.clone();
    like_tauri(move || a.typing());
    wait_for("bob sees alice typing", 10, || typing.load(Ordering::SeqCst) == 1).await;

    // Alice nudges Bob. A second nudge right after is refused.
    let a = alice.clone();
    like_tauri(move || a.nudge()).unwrap();
    wait_for("bob gets the nudge", 10, || nudges.load(Ordering::SeqCst) == 1).await;
    assert!(alice.nudge().is_err(), "nudges are rate-limited");

    // Bob goes away with a note. Using the computer doesn't end it; his next message does.
    bob.set_away("  lunch\n  break ");
    let lunch = Some(Away { note: Some("lunch break".into()) });
    wait_for("alice sees bob away", 10, || alice.status().peer_away == lunch).await;
    assert_eq!(bob.status().away, lunch);
    bob.set_idle(false);
    assert_eq!(bob.status().away, lunch, "still away by hand");

    // Bob clears his copy. Alice keeps hers, and numbering carries on.
    let b = bob.clone();
    like_tauri(move || b.clear_history()).unwrap();
    assert!(texts(&bob).is_empty());
    assert_eq!(texts(&alice).len(), 5);
    assert_eq!(bob.send_message("fresh start", None, None).unwrap().seq, 6);
    wait_for("alice gets it", 10, || texts(&alice).contains(&"fresh start".into())).await;
    assert_eq!(texts(&alice).len(), 6);
    assert_eq!(texts(&bob), ["fresh start"]);
    wait_for("bob's message brings him back", 10, || alice.status().peer_away.is_none()).await;

    // Idle: away without a note, until input resumes. Bob's own screen doesn't show it.
    bob.set_idle(true);
    let idle = Some(Away { note: None });
    wait_for("alice sees bob idle", 10, || alice.status().peer_away == idle).await;
    assert_eq!(bob.status().away, None);
    bob.set_idle(false);
    wait_for("alice sees bob back", 10, || alice.status().peer_away.is_none()).await;

    // Each side knows the other's version, which is what allows vanishing messages.
    let info = alice.connection_info();
    assert_eq!(info.peer_version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
    assert!(info.since.is_some() && info.address.is_some() && info.rtt_ms.is_some());

    // Do not disturb: Alice sees it, and her nudges are refused until Bob turns it off.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let later = now.as_millis() as i64 + 3_600_000;
    bob.set_dnd(Some(later)).unwrap();
    assert!(bob.dnd() && bob.status().dnd_until == Some(later));
    wait_for("alice sees bob's dnd", 10, || alice.status().peer_dnd_until == Some(later)).await;
    let refused = alice.nudge().unwrap_err().to_string();
    assert!(refused.contains("do not disturb"), "{refused}");
    bob.set_dnd(None).unwrap();
    wait_for("alice sees it end", 10, || alice.status().peer_dnd_until.is_none()).await;

    // A vanishing message: once Bob has seen it, it's gone from both sides 5 seconds later.
    let secret = alice.send_message("the code is 1234", None, Some(5)).unwrap();
    let at = Ref { author: alice.id(), seq: secret.seq };
    wait_for("bob gets it", 10, || find(&bob, &at).is_some()).await;
    assert_eq!(find(&bob, &at).unwrap().expires, None, "its clock starts when seen");
    bob.mark_seen().unwrap();
    wait_for("alice's clock starts", 10, || find(&alice, &at).is_some_and(|m| m.expires.is_some()))
        .await;
    wait_for("it vanishes on both sides", 15, || {
        find(&alice, &at).is_some_and(|m| m.vanished) && find(&bob, &at).is_some_and(|m| m.vanished)
    })
    .await;
    assert!(!texts(&bob).contains(&"the code is 1234".into()));

    alice.shutdown().await;
    bob.shutdown().await;
    let _ = std::fs::remove_dir_all(dir_a);
    let _ = std::fs::remove_dir_all(dir_b);
}
