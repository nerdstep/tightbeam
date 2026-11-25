//! Two real Tightbeam cores on this machine, found via mDNS and talking over QUIC: the
//! part of the engine only the real network can check. Discovery, pairing, a message
//! each way, a restart that redials and catches up, and a file on streams of its own.
//!
//! The protocol itself (receipts, presence, edits, vanishing messages, files that go
//! missing, other builds) is tested on the in-memory network in `chat/protocol_tests.rs`.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use tightbeam_lib::{
    chat::{Core, Emit, NewFile},
    db::{FileState, Message},
    entry::Ref,
};

fn quiet() -> Emit {
    Arc::new(|_| {})
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

fn find(core: &Core, at: &Ref) -> Option<Message> {
    core.messages().unwrap().into_iter().find(|m| m.author == at.author && m.seq == at.seq)
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

    // Messages both ways, with delivery confirmation.
    let a = alice.clone();
    like_tauri(move || a.send_message("hi bob", None, None)).unwrap();
    wait_for("bob gets alice's message", 10, || texts(&bob).contains(&"hi bob".into())).await;
    bob.send_message("hi alice", None, None).unwrap();
    wait_for("alice gets bob's message", 10, || texts(&alice).contains(&"hi alice".into())).await;
    wait_for("alice sees delivery", 10, || alice.status().peer_has == 1).await;

    // Bob quits; Alice keeps writing. He comes back with the same identity, the dialer
    // finds him again, and he catches up.
    bob.shutdown().await;
    drop(bob);
    wait_for("alice notices bob left", 40, || !alice.status().connected).await;
    alice.send_message("you there?", None, None).unwrap();
    bob = Core::start_with(dir_b.clone(), quiet(), true).await.unwrap();
    assert_eq!(bob.id(), bob_id, "identity must survive a restart");
    assert_eq!(bob.status().peer.unwrap().name, "Alice", "pairing must survive a restart");
    wait_for("reconnect", 40, || alice.status().connected && bob.status().connected).await;
    wait_for("bob catches up", 10, || texts(&bob).contains(&"you there?".into())).await;
    wait_for("alice sees it delivered", 10, || alice.status().peer_has == 2).await;

    // A picture: the message arrives, then the file on a stream of its own.
    let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let new = NewFile {
        name: "sunset.png".into(),
        mime: "image/png".into(),
        width: Some(1600),
        height: Some(900),
        preview: Some("data:image/jpeg;base64,AAAA".into()),
        text: "look at this".into(),
        reply_to: None,
    };
    let (a, sent) = (alice.clone(), bytes.clone());
    let photo = like_tauri(move || a.send_file(&sent, new)).unwrap();
    let file = photo.file.clone().unwrap();
    assert_eq!(file.state, FileState::Done, "the sender has it already");
    let at = Ref { author: alice.id(), seq: photo.seq };
    wait_for("bob gets the picture", 30, || {
        find(&bob, &at).is_some_and(|m| m.file.as_ref().is_some_and(|f| f.state == FileState::Done))
    })
    .await;
    let (path, mime) = bob.file(&file.info.hash).unwrap().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(path.file_name().unwrap(), "sunset.png");
    assert_eq!(mime, "image/png");

    alice.shutdown().await;
    bob.shutdown().await;
    let _ = std::fs::remove_dir_all(dir_a);
    let _ = std::fs::remove_dir_all(dir_b);
}
