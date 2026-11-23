//! Two real Tightbeam cores on this machine, found via mDNS and talking over QUIC.
//! Covers pairing, live messages, delivery acks, seen receipts, catching up after one
//! side was off, nudges, and clearing one side's history.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tightbeam_lib::chat::{Core, Emit, Event};

fn quiet() -> Emit {
    Arc::new(|_| {})
}

/// Counts the nudges it's told about.
fn counting_nudges(count: Arc<AtomicUsize>) -> Emit {
    Arc::new(move |event| {
        if let Event::Nudge = event {
            count.fetch_add(1, Ordering::SeqCst);
        }
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
    like_tauri(move || a.send_message("hi bob")).unwrap();
    wait_for("bob gets alice's message", 10, || texts(&bob).contains(&"hi bob".into())).await;
    bob.send_message("hi alice").unwrap();
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
    alice.send_message("you there?").unwrap();
    alice.send_message("guess not").unwrap();
    assert_eq!(alice.status().peer_has, 1, "undelivered messages must not show as delivered");

    // Bob comes back with the same identity and catches up.
    let nudges = Arc::new(AtomicUsize::new(0));
    bob = Core::start_with(dir_b.clone(), counting_nudges(nudges.clone()), true).await.unwrap();
    assert_eq!(bob.id(), bob_id, "identity must survive a restart");
    assert_eq!(bob.status().peer.unwrap().name, "Alice", "pairing must survive a restart");
    wait_for("reconnect", 40, || alice.status().connected && bob.status().connected).await;
    wait_for("bob catches up", 10, || {
        let t = texts(&bob);
        t.contains(&"you there?".into()) && t.contains(&"guess not".into())
    })
    .await;
    wait_for("alice sees catch-up delivered", 10, || alice.status().peer_has == 3).await;
    assert_eq!(texts(&bob).len(), 4, "no duplicates after catch-up");
    assert_eq!(alice.status().peer_seen, 1, "seen must survive the reconnect, and not grow");
    let b = bob.clone();
    like_tauri(move || b.mark_seen()).unwrap();
    wait_for("alice sees the catch-up seen", 10, || alice.status().peer_seen == 3).await;

    // Alice nudges Bob. A second nudge right after is refused.
    let a = alice.clone();
    like_tauri(move || a.nudge()).unwrap();
    wait_for("bob gets the nudge", 10, || nudges.load(Ordering::SeqCst) == 1).await;
    assert!(alice.nudge().is_err(), "nudges are rate-limited");

    // Bob clears his copy. Alice keeps hers, and numbering carries on.
    let b = bob.clone();
    like_tauri(move || b.clear_history()).unwrap();
    assert!(texts(&bob).is_empty());
    assert_eq!(texts(&alice).len(), 4);
    assert_eq!(bob.send_message("fresh start").unwrap().seq, 2);
    wait_for("alice gets it", 10, || texts(&alice).contains(&"fresh start".into())).await;
    assert_eq!(texts(&alice).len(), 5);
    assert_eq!(texts(&bob), ["fresh start"]);

    alice.shutdown().await;
    bob.shutdown().await;
    let _ = std::fs::remove_dir_all(dir_a);
    let _ = std::fs::remove_dir_all(dir_b);
}
