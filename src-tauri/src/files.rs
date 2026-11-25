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

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Result, bail, ensure};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::{
    db::{Db, FileState},
    entry::{FileInfo, MAX_FILE_BYTES, is_hash},
};

/// While a file from disk is copied in, before its hash is known.
const ADDING: &str = ".adding";

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
    /// The store in `dir`. Copies left half-done when the app last closed are removed.
    pub fn new(dir: PathBuf) -> Self {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().ends_with(ADDING) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        Self { dir }
    }

    /// Copies a file from disk into the folder, hashing it on the way, never all in
    /// memory. Blocking, and it doesn't touch the log, so it can take its time; then
    /// `adopt` keeps it.
    pub fn copy_in(&self, from: &Path) -> Result<Copied> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut source = std::fs::File::open(from)?;
        let size = source.metadata()?.len();
        ensure!(size > 0, "That file is empty.");
        ensure!(size <= MAX_FILE_BYTES, "Files can be up to 1 GB.");
        std::fs::create_dir_all(&self.dir)?;
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let temp = self.dir.join(format!("{}-{n}{ADDING}", std::process::id()));
        // Removes the copy if anything below fails.
        let mut copied = Copied { temp: Some(temp.clone()), hash: String::new(), size: 0 };
        let mut out = std::io::BufWriter::new(std::fs::File::create(&temp)?);
        let (mut hasher, mut buf, mut total) = (blake3::Hasher::new(), vec![0u8; 256 * 1024], 0);
        loop {
            let n = source.read(&mut buf)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            ensure!(total <= MAX_FILE_BYTES, "Files can be up to 1 GB.");
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
        }
        out.flush()?;
        ensure!(total > 0, "That file is empty.");
        (copied.hash, copied.size) = (hasher.finalize().to_hex().to_string(), total);
        Ok(copied)
    }

    /// Keeps a copied file under `name`, unless the same file is here already.
    pub fn adopt(&self, db: &Db, mut copied: Copied, name: &str) -> Result<()> {
        let Some(temp) = copied.temp.take() else { bail!("already kept") };
        if db.file_state(&copied.hash)? == FileState::Done {
            let _ = std::fs::remove_file(&temp);
            return Ok(());
        }
        let name = safe_name(name);
        let dir = self.dir.join(&copied.hash);
        std::fs::create_dir_all(&dir)?;
        std::fs::rename(&temp, dir.join(&name))?;
        db.set_file(&copied.hash, &name, FileState::Done)
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

/// A file from disk, copied in and hashed (`FileStore::copy_in`). Removed if dropped
/// before it's kept.
pub(crate) struct Copied {
    temp: Option<PathBuf>,
    pub hash: String,
    pub size: u64,
}

impl Drop for Copied {
    fn drop(&mut self) {
        if let Some(temp) = &self.temp {
            let _ = std::fs::remove_file(temp);
        }
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

/// A file's type, from the end of its name, for files from disk. Others are just bytes.
pub(crate) fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "m4v" => "video/x-m4v",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Whether a file may open in its default app, by the end of its name. Opening runs that
/// app on it, so this is an allow-list: pictures, documents without macros, audio and
/// video. Anything else (programs, scripts, shortcuts, web pages, SVG, Office files with
/// macros, disk images, archives) can only be shown in its folder.
pub(crate) fn may_open(name: &str) -> bool {
    const OPENABLE: &[&str] = &[
        // Pictures.
        "jpg", "jpeg", "png", "gif", "webp", "avif", "bmp", "heic", "heif", "tif", "tiff",
        // Documents.
        "pdf", "txt", "md", "csv", "docx", "xlsx", "pptx", "odt", "ods", "odp",
        // Audio.
        "mp3", "m4a", "aac", "wav", "flac", "ogg", // Video.
        "mp4", "m4v", "mov", "webm", "mkv",
    ];
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    OPENABLE.contains(&ext.as_str())
}

/// Types the webview may show as an image. Not SVG, which can carry scripts.
pub(crate) fn may_show(mime: &str) -> bool {
    const SHOWABLE: &[&str] = &[
        "image/jpeg",
        "image/png",
        "image/gif",
        "image/webp",
        "image/avif",
        "image/bmp",
        "image/heic",
        "image/heif",
    ];
    SHOWABLE.contains(&mime)
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
    fn a_file_from_disk_is_copied_in_and_kept_once() {
        let (store, db, info, bytes, dir) = setup("copy");
        let original = dir.with_extension("original.jpg");
        std::fs::write(&original, &bytes).unwrap();
        let copied = store.copy_in(&original).unwrap();
        assert_eq!((copied.hash.as_str(), copied.size), (info.hash.as_str(), info.size));
        attach(&db, &info, 1);
        store.adopt(&db, copied, "beach.jpg").unwrap();
        assert!(matches!(store.get(&db, &info.hash).unwrap(), Found::Here { .. }));

        // The original can change or go: the store has its own copy.
        std::fs::remove_file(&original).unwrap();
        assert!(matches!(store.get(&db, &info.hash).unwrap(), Found::Here { .. }));

        // Copying the same file again keeps the one copy; one never kept leaves nothing.
        std::fs::write(&original, &bytes).unwrap();
        let again = store.copy_in(&original).unwrap();
        store.adopt(&db, again, "beach.jpg").unwrap();
        drop(store.copy_in(&original).unwrap());
        let adding = |dir: &Path| {
            std::fs::read_dir(dir)
                .unwrap()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(ADDING))
        };
        assert!(!adding(&dir), "nothing half-copied left behind");

        std::fs::write(&original, b"").unwrap();
        let refused = store.copy_in(&original).err().unwrap().to_string();
        assert_eq!(refused, "That file is empty.");
        std::fs::write(dir.join(format!("9-9{ADDING}")), b"interrupted").unwrap();
        let _ = FileStore::new(dir.clone());
        assert!(!adding(&dir), "an interrupted copy goes at the next start");
        let _ = std::fs::remove_file(original);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_known_kinds_open() {
        for name in ["beach.JPG", "Report.pdf", "notes.md", "budget.xlsx", "song.flac", "clip.mov"]
        {
            assert!(may_open(name), "{name}");
        }
        let unsafe_kinds = [
            "setup.exe",
            "install.msi",
            "run.bat",
            "script.ps1",
            "link.lnk",
            "site.url",
            "page.html",
            "logo.svg",
            "macros.docm",
            "sheet.xlsm",
            "disk.iso",
            "disk.dmg",
            "archive.zip",
            "Tool.app",
            "go.command",
            "go.sh",
            "app.jar",
            "no extension",
            "trick.pdf.exe",
        ];
        for name in unsafe_kinds {
            assert!(!may_open(name), "{name}");
        }
    }

    #[test]
    fn types_come_from_the_name() {
        assert_eq!(mime_for("Report.PDF"), "application/pdf");
        assert_eq!(mime_for("song.flac"), "audio/flac");
        assert_eq!(mime_for("setup.exe"), "application/octet-stream");
        assert_eq!(mime_for("no extension"), "application/octet-stream");
        assert!(may_show("image/png") && !may_show("image/svg+xml") && !may_show("text/html"));
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
