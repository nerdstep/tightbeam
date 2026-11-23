//! Fills two paired test profiles with the same synthetic history, for trying the UI
//! with a long chat. Both apps must be closed.
//!
//!   cargo run --example seed -- %TEMP%\tightbeam-alice %TEMP%\tightbeam-bob 50000
//!
//! Both profiles get identical messages, so nothing needs to sync and message numbers
//! stay consistent for real messages sent afterwards.

use std::path::Path;

use anyhow::{Context, Result, ensure};
use iroh::SecretKey;
use tightbeam_lib::db::{Db, Message};

const LINES: &[&str] = &[
    "hey",
    "on my way",
    "did you see the thing I sent earlier? the one about the trip",
    "lol",
    "ok sounds good",
    "can you grab milk on the way home",
    "I was thinking we could try that new place on 5th this weekend, if you're free on Saturday afternoon",
    "👍",
    "running about ten minutes late, sorry!",
    "love you",
];

fn identity(db: &Db) -> Result<String> {
    let bytes: [u8; 32] = db
        .get("secret_key")?
        .context("no identity yet; run the app once with this profile")?
        .try_into()
        .ok()
        .context("corrupt secret key")?;
    Ok(SecretKey::from_bytes(&bytes).public().to_string())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ensure!(args.len() >= 2, "usage: seed <profile dir A> <profile dir B> [count]");
    let count: u64 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(50_000);

    let a = Db::open(&Path::new(&args[0]).join("tightbeam.db"))?;
    let b = Db::open(&Path::new(&args[1]).join("tightbeam.db"))?;
    let (id_a, id_b) = (identity(&a)?, identity(&b)?);
    ensure!(
        a.get_str("peer_id")?.as_deref() == Some(id_b.as_str())
            && b.get_str("peer_id")?.as_deref() == Some(id_a.as_str()),
        "the two profiles aren't paired with each other"
    );

    // Spread over the past year, ending an hour ago, continuing each author's numbering.
    let mut next =
        [a.max_seq(&id_a)?.max(b.max_seq(&id_a)?), a.max_seq(&id_b)?.max(b.max_seq(&id_b)?)];
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as i64;
    let start = now - 365 * 24 * 3_600_000;
    let step = (now - 3_600_000 - start) / count as i64;
    let messages: Vec<Message> = (0..count)
        .map(|i| {
            // Mostly alternate, with occasional runs from one person.
            let who = ((i / 3 + i % 2) % 2) as usize;
            next[who] += 1;
            Message {
                author: if who == 0 { id_a.clone() } else { id_b.clone() },
                seq: next[who],
                ts: start + i as i64 * step,
                text: LINES[(i as usize * 7 + who) % LINES.len()].to_string(),
            }
        })
        .collect();

    a.insert_many(&messages)?;
    b.insert_many(&messages)?;
    println!("added {count} messages to both profiles");
    Ok(())
}
