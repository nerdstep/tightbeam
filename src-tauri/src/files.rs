//! The file store: the files messages carry, kept in the data folder.
//!
//! Each file lives at `<hash>/<name>`, with `<hash>.part` while it arrives. The `files`
//! table (in `Db`) says which are here ('done') and which the sender no longer has
//! ('gone'). The rule this module keeps: a file stays exactly as long as some message has
//! it. Someone may move, delete or edit files in the folder by hand; a file that's
//! missing or changed counts as lost and is fetched again.
//!
//! Work that touches the database takes `&Db` and is quick. Work that waits on the disk
//! (hashing, a file arriving) takes no `Db`, so the caller never holds its lock across
//! an `await`.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail, ensure};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::{
    db::{Db, FileState},
    entry::{FileInfo, is_hash},
};

pub(crate) struct FileStore {
    dir: PathBuf,
}

/// What `FileStore::get` finds.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Found {
    /// Here, intact: where it is, and its type.
    Here { path: PathBuf, mime: String },
    /// Recorded as here, but missing or changed on disk. The caller forgets it
    /// (`FileStore::lost`) so it's fetched again.
    Lost,
    /// Not here, or no message has it.
    NotHere,
}

impl FileStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Keeps `bytes` as the file `info` describes, unless it's here already. Blocking.
    pub fn put_bytes(&self, db: &Db, info: &FileInfo, bytes: &[u8]) -> Result<()> {
        if db.file_state(&info.hash)? == FileState::Done {
            return Ok(());
        }
        let name = safe_name(&info.name);
        let part = self.part_path(&info.hash);
        std::fs::create_dir_all(self.dir.join(&info.hash))?;
        std::fs::write(&part, bytes)?;
        std::fs::rename(&part, self.dir.join(&info.hash).join(&name))?;
        db.set_file(&info.hash, &name, FileState::Done)
    }

    /// A file that's here and attached to a message. One that's gone, or whose size
    /// changed, is `Lost`.
    pub fn get(&self, db: &Db, hash: &str) -> Result<Found> {
        if !is_hash(hash) {
            return Ok(Found::NotHere);
        }
        let Some(stored) = db.stored_file(hash)? else { return Ok(Found::NotHere) };
        let path = self.dir.join(hash).join(&stored.name);
        let intact = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == stored.size);
        Ok(if intact { Found::Here { path, mime: stored.mime } } else { Found::Lost })
    }

    /// Forgets a lost file, so it's fetched again.
    pub fn lost(&self, db: &Db, hash: &str) -> Result<()> {
        db.forget_file(hash)
    }

    /// Whether the file at `path` still has the contents `hash` names: one edited in place
    /// keeps its size, so only hashing tells.
    pub async fn unchanged(&self, path: &Path, hash: &str) -> Result<bool> {
        let path = path.to_path_buf();
        Ok(tokio::task::spawn_blocking(move || hash_file(&path)).await?? == hash)
    }

    /// Starts receiving `file`, from where an earlier attempt broke off.
    pub async fn receive(&self, file: &FileInfo) -> Result<Part> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let path = self.part_path(&file.hash);
        let mut part = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .await?;
        let mut have = part.metadata().await?.len();
        if have > file.size {
            have = 0;
        }
        part.set_len(have).await?;
        part.seek(std::io::SeekFrom::Start(have)).await?;
        Ok(Part { file: Some(part), path, have, info: file.clone() })
    }

    /// Keeps a file that arrived whole, unless no message has it any more (deleted while
    /// it was on its way). Returns whether it was kept.
    pub fn keep(&self, db: &Db, arrived: Arrived) -> Result<bool> {
        let Arrived { path, info } = arrived;
        if !db.file_in_use(&info.hash)? {
            let _ = std::fs::remove_file(&path);
            return Ok(false);
        }
        let name = safe_name(&info.name);
        let dir = self.dir.join(&info.hash);
        std::fs::create_dir_all(&dir)?;
        std::fs::rename(&path, dir.join(&name))?;
        db.set_file(&info.hash, &name, FileState::Done)?;
        Ok(true)
    }

    /// The sender doesn't have the file any more.
    pub fn gone(&self, db: &Db, hash: &str) -> Result<()> {
        let _ = std::fs::remove_file(self.part_path(hash));
        db.set_file(hash, "", FileState::Gone)
    }

    /// Removes files no message has any more, and partial downloads of them.
    pub fn prune(&self, db: &Db) -> Result<()> {
        for hash in db.unused_files()? {
            let dir = self.dir.join(&hash);
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
            db.forget_file(&hash)?;
        }
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Ok(()) };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(hash) = name.strip_suffix(".part")
                && !db.file_in_use(hash)?
            {
                // Fails while it's still being written; the download checks again.
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(())
    }

    fn part_path(&self, hash: &str) -> PathBuf {
        self.dir.join(format!("{hash}.part"))
    }
}

/// A file on its way in.
pub(crate) struct Part {
    /// `None` once finished.
    file: Option<tokio::fs::File>,
    path: PathBuf,
    have: u64,
    info: FileInfo,
}

/// A file that arrived whole and matches its hash, waiting to be kept (`FileStore::keep`).
pub(crate) struct Arrived {
    path: PathBuf,
    info: FileInfo,
}

impl Part {
    /// How many bytes are here already: where to ask the sender to start.
    pub fn have(&self) -> u64 {
        self.have
    }

    pub async fn write(&mut self, buf: &[u8]) -> Result<()> {
        self.have += buf.len() as u64;
        ensure!(self.have <= self.info.size, "the file is bigger than they said");
        let Some(file) = &mut self.file else { bail!("already finished") };
        file.write_all(buf).await?;
        Ok(())
    }

    /// Checks the whole file against its hash. One that doesn't match is removed, so the
    /// next try starts over.
    pub async fn finish(mut self) -> Result<Arrived> {
        if let Some(mut file) = self.file.take() {
            file.flush().await?;
        }
        ensure!(self.have == self.info.size, "the file ended early");
        let path = self.path.clone();
        let hash = tokio::task::spawn_blocking(move || hash_file(&path)).await??;
        if hash != self.info.hash {
            let _ = tokio::fs::remove_file(&self.path).await;
            bail!("{} didn't match its hash", self.info.name);
        }
        Ok(Arrived { path: self.path, info: self.info })
    }
}

fn hash_file(path: &Path) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(std::fs::File::open(path)?)?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// A file name that's safe to store on Windows and macOS, keeping its extension.
fn safe_name(name: &str) -> String {
    const MAX: usize = 100;
    let name: String = name
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    let name = name.trim().trim_start_matches('.').trim_end_matches(['.', ' ']);
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.chars().count() <= 10 => (stem, ext),
        _ => (name, ""),
    };
    let room = MAX - ext.chars().count() - 1;
    let mut stem: String = stem.chars().take(room).collect::<String>().trim_end().to_string();
    if stem.is_empty() {
        stem = "file".into();
    }
    // Names Windows reserves for devices, with or without an extension.
    let device = stem.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&device.as_str())
        || (device.len() == 4
            && (device.starts_with("COM") || device.starts_with("LPT"))
            && device.as_bytes()[3].is_ascii_digit());
    if reserved {
        stem.insert(0, '_');
    }
    if ext.is_empty() { stem } else { format!("{stem}.{ext}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{Entry, Kind, Ref};

    /// A store in a fresh folder, a log in memory, and a picture with its bytes.
    fn setup(label: &str) -> (FileStore, Db, FileInfo, Vec<u8>, PathBuf) {
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("tightbeam-files-{nanos}-{label}"));
        let bytes: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let info = FileInfo {
            hash: blake3::hash(&bytes).to_hex().to_string(),
            name: "beach.jpg".into(),
            size: bytes.len() as u64,
            mime: "image/jpeg".into(),
            width: None,
            height: None,
            preview: None,
        };
        let db = Db::open(Path::new(":memory:")).unwrap();
        (FileStore::new(dir.clone()), db, info, bytes, dir)
    }

    /// A message with the file, so the store keeps it.
    fn attach(db: &Db, info: &FileInfo, seq: u64) {
        let kind = Kind::Message { reply_to: None, vanish: None, file: Some(info.clone()) };
        db.insert(&Entry { kind, ..Entry::message("me", seq, 1, "") }).unwrap();
    }

    #[test]
    fn a_kept_file_is_here_until_it_changes_on_disk() {
        let (store, db, info, bytes, dir) = setup("get");
        attach(&db, &info, 1);
        store.put_bytes(&db, &info, &bytes).unwrap();
        let path = dir.join(&info.hash).join("beach.jpg");
        let here = Found::Here { path: path.clone(), mime: "image/jpeg".into() };
        assert_eq!(store.get(&db, &info.hash).unwrap(), here);
        assert_eq!(store.get(&db, "../x").unwrap(), Found::NotHere);

        std::fs::write(&path, b"cropped").unwrap(); // edited by hand
        assert_eq!(store.get(&db, &info.hash).unwrap(), Found::Lost);
        store.lost(&db, &info.hash).unwrap();
        assert_eq!(store.get(&db, &info.hash).unwrap(), Found::NotHere, "to be fetched again");
        assert_eq!(db.wanted_files().unwrap(), [info]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn an_arriving_file_resumes_and_is_checked() {
        let (store, db, info, bytes, dir) = setup("receive");
        attach(&db, &info, 1);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{}.part", info.hash)), &bytes[..1000]).unwrap();

        let mut part = store.receive(&info).await.unwrap();
        assert_eq!(part.have(), 1000, "picks up where it broke off");
        part.write(&bytes[1000..]).await.unwrap();
        let arrived = part.finish().await.unwrap();
        assert!(store.keep(&db, arrived).unwrap());
        assert!(matches!(store.get(&db, &info.hash).unwrap(), Found::Here { .. }));

        // Too many bytes, too few, or the wrong ones: nothing kept.
        let other = FileInfo { hash: "ab".repeat(32), ..info.clone() };
        let mut part = store.receive(&other).await.unwrap();
        assert!(part.write(&vec![0; bytes.len() + 1]).await.is_err());
        let part = store.receive(&other).await.unwrap();
        assert!(part.finish().await.is_err(), "it ended early");
        let mut part = store.receive(&other).await.unwrap();
        part.write(&bytes).await.unwrap();
        assert!(part.finish().await.is_err(), "the hash doesn't match");
        assert!(!dir.join(format!("{}.part", other.hash)).exists(), "the next try starts over");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_file_no_message_has_is_not_kept_and_is_pruned() {
        let (store, db, info, bytes, dir) = setup("prune");
        // Arrives after its message was deleted.
        let mut part = store.receive(&info).await.unwrap();
        part.write(&bytes).await.unwrap();
        let arrived = part.finish().await.unwrap();
        assert!(!store.keep(&db, arrived).unwrap());
        assert!(!dir.join(&info.hash).exists());

        // Kept while a message has it; pruned once none does, with any partial download.
        attach(&db, &info, 1);
        store.put_bytes(&db, &info, &bytes).unwrap();
        std::fs::write(dir.join(format!("{}.part", "cd".repeat(32))), b"half").unwrap();
        store.prune(&db).unwrap();
        assert!(dir.join(&info.hash).exists(), "still in use");
        assert!(!dir.join(format!("{}.part", "cd".repeat(32))).exists());
        db.scrub(&Ref { author: "me".into(), seq: 1 }).unwrap(); // deleted
        store.prune(&db).unwrap();
        assert!(!dir.join(&info.hash).exists());
        assert_eq!(db.file_state(&info.hash).unwrap(), FileState::Pending, "forgotten");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_the_sender_lost_is_gone() {
        let (store, db, info, _, dir) = setup("gone");
        attach(&db, &info, 1);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{}.part", info.hash)), b"some").unwrap();
        store.gone(&db, &info.hash).unwrap();
        assert_eq!(db.file_state(&info.hash).unwrap(), FileState::Gone);
        assert!(!dir.join(format!("{}.part", info.hash)).exists());
        assert!(db.wanted_files().unwrap().is_empty(), "not asked for again");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn file_names_are_safe_to_store() {
        assert_eq!(safe_name("beach.jpg"), "beach.jpg");
        assert_eq!(safe_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_name(r"C:\Windows\a:b?.png"), "C__Windows_a_b_.png");
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name("trailing. . "), "trailing");
        assert_eq!(safe_name("con.txt"), "_con.txt");
        assert_eq!(safe_name("COM1"), "_COM1");
        assert_eq!(safe_name("company.txt"), "company.txt");
        assert_eq!(safe_name("..."), "file");
        assert_eq!(safe_name(""), "file");
        let long = format!("{}.jpeg", "x".repeat(300));
        let safe = safe_name(&long);
        assert_eq!(safe.chars().count(), 100);
        assert!(safe.ends_with(".jpeg"));
    }
}
