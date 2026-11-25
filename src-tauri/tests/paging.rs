//! Paging through a long history: every message exactly once, in display order, and fast.

use std::time::Instant;

use tightbeam_lib::{
    db::{Cursor, Db, Message},
    entry::Entry,
};

const COUNT: u64 = 50_000;

#[test]
fn pages_through_a_long_history() {
    let dir = std::env::temp_dir().join(format!("tightbeam-paging-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).unwrap();

    // Two authors alternating, one message a minute. Some share a timestamp to exercise
    // the (author, seq) tie-break.
    let entries: Vec<Entry> = (0..COUNT)
        .map(|i| {
            Entry::message(
                if i % 2 == 0 { "alice" } else { "bob" },
                i / 2 + 1,
                1_700_000_000_000 + (i / 4) as i64 * 60_000,
                format!("message {i}"),
            )
        })
        .collect();
    let started = Instant::now();
    db.insert_many(&entries).unwrap();
    println!("inserted {COUNT} in {:?}", started.elapsed());

    let started = Instant::now();
    let first = db.page(None, 200).unwrap();
    let first_page_time = started.elapsed();
    println!("newest page in {first_page_time:?}");
    assert_eq!(first.messages.len(), 200);
    assert!(first.has_more);

    // Walk back to the beginning.
    let mut seen = first.messages.clone();
    let mut page = first;
    while page.has_more {
        let oldest = &page.messages[0];
        let cursor = Cursor { hlc: oldest.hlc, author: oldest.author.clone(), seq: oldest.seq };
        page = db.page(Some(&cursor), 200).unwrap();
        let mut older = page.messages.clone();
        older.extend(seen);
        seen = older;
    }

    assert_eq!(seen.len() as u64, COUNT, "every message exactly once");
    let key = |m: &Message| (m.hlc, m.author.clone(), m.seq);
    assert!(seen.windows(2).all(|w| key(&w[0]) < key(&w[1])), "strictly in display order");
    assert!(
        first_page_time.as_millis() < 100,
        "newest page took {first_page_time:?}, expected an index lookup"
    );

    drop(db);
    let _ = std::fs::remove_dir_all(dir);
}
