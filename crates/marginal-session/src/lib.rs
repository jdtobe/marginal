//! Hot exit: every open buffer is journaled continuously so it survives crashes and quits, while
//! the user's file on disk is only written by an explicit save.
//!
//! Layout under the data directory (`~/Library/Application Support/Marginal` on macOS):
//!
//! ```text
//! session.json              open buffers, their order, selections and scroll positions
//! buffers/<id>/origin.md    the text when the buffer was opened (never modified)
//! buffers/<id>/journal.jsonl  one entry per edit, undo, redo and save, appended as they happen
//! ```
//!
//! Replaying the journal over `origin.md` rebuilds both the text and the full undo tree, which
//! is what makes undo history persistent across restarts.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use marginal_doc::{Document, Transaction};
use marginal_history::{EditKind, History, Selection};
use serde::{Deserialize, Serialize};

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionFile {
    pub buffers: Vec<BufferMeta>,
    pub active: Option<String>,
    /// Code block languages chosen recently, newest first.
    #[serde(default)]
    pub recent_languages: Vec<String>,
    /// View preferences and the recent files list.
    #[serde(default)]
    pub settings: Settings,
}

/// Preferences that apply across documents.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The chosen theme's name (built-in or a user theme's file stem); `None` is the default.
    pub theme: Option<String>,
    /// `"light"` or `"dark"` to override the system; `None` follows it.
    pub appearance: Option<String>,
    /// Text zoom; `None` is 100%.
    pub zoom: Option<f32>,
    /// X-ray latched on from the View menu.
    pub xray: bool,
    /// Recently opened files, newest first.
    pub recent_files: Vec<PathBuf>,
    pub find: marginal_doc::FindOptions,
}

/// How many recent files to remember.
pub const RECENT_FILES: usize = 10;

impl Settings {
    /// Moves `path` to the top of the recent files.
    pub fn remember_file(&mut self, path: &Path) {
        self.recent_files.retain(|p| p != path);
        self.recent_files.insert(0, path.to_path_buf());
        self.recent_files.truncate(RECENT_FILES);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BufferMeta {
    pub id: String,
    pub path: Option<PathBuf>,
    pub selection: Selection,
    pub scroll: f32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Entry {
    Edit { tx: Transaction, before: Selection, after: Selection, kind: EditKind, time: u64 },
    Undo,
    Redo,
    Saved { path: PathBuf },
}

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The platform data directory, overridable with `MARGINAL_DATA_DIR`.
    pub fn default_location() -> io::Result<Self> {
        let root = match std::env::var_os("MARGINAL_DATA_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => dirs::data_dir()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no data directory"))?
                .join("Marginal"),
        };
        Self::at(root)
    }

    pub fn at(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("buffers"))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The saved session, or an empty one if it's missing or unreadable.
    pub fn load_session(&self) -> SessionFile {
        fs::read(self.root.join("session.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save_session(&self, session: &SessionFile) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(session).map_err(io::Error::other)?;
        write_atomic(&self.root.join("session.json"), &json)
    }

    fn buffer_dir(&self, id: &str) -> PathBuf {
        self.root.join("buffers").join(id)
    }

    /// Starts journaling a newly opened file (or an untitled buffer when `path` is `None`).
    pub fn create_buffer(&self, path: Option<&Path>, text: &str) -> io::Result<Buffer> {
        let id = format!("{:x}-{:x}", now_ms(), std::process::id() ^ (text.len() as u32));
        let dir = self.buffer_dir(&id);
        fs::create_dir_all(&dir)?;
        write_atomic(&dir.join("origin.md"), text.as_bytes())?;
        let journal = OpenOptions::new().create(true).append(true).open(dir.join("journal.jsonl"))?;
        Ok(Buffer {
            id,
            path: path.map(Path::to_path_buf),
            doc: Document::new(text),
            history: History::new(),
            disk_text: text.to_owned(),
            saved_version: 0,
            journal,
            unsynced: false,
        })
    }

    /// Restores a journaled buffer: replays every recorded edit, undo, redo and save.
    ///
    /// A torn final line (from a crash mid-write) or an entry that no longer applies ends the
    /// replay there; everything before it is kept.
    pub fn open_buffer(&self, meta: &BufferMeta) -> io::Result<Buffer> {
        let dir = self.buffer_dir(&meta.id);
        let origin = fs::read_to_string(dir.join("origin.md"))?;
        let mut doc = Document::new(origin.as_str());
        let mut history = History::new();
        let mut disk_text = origin.clone();
        let mut saved_version = 0;
        let mut path = meta.path.clone();
        let journal_path = dir.join("journal.jsonl");
        let mut valid_len = 0u64;
        if let Ok(file) = File::open(&journal_path) {
            let mut reader = BufReader::new(file);
            let mut line = String::new();
            loop {
                line.clear();
                let n = reader.read_line(&mut line)?;
                if n == 0 || !line.ends_with('\n') {
                    break;
                }
                let Ok(entry) = serde_json::from_str::<Entry>(&line) else { break };
                match entry {
                    Entry::Edit { tx, before, after, kind, time } => {
                        if !applies(&tx, doc.text()) {
                            break;
                        }
                        history.apply(&mut doc, tx, before, after, kind, time);
                    }
                    Entry::Undo => {
                        history.undo(&mut doc);
                    }
                    Entry::Redo => {
                        history.redo(&mut doc);
                    }
                    Entry::Saved { path: p } => {
                        disk_text = doc.text().to_owned();
                        saved_version = history.version();
                        path = Some(p);
                    }
                }
                valid_len += n as u64;
            }
        }
        let journal = OpenOptions::new().create(true).append(true).open(&journal_path)?;
        // Drop any torn tail so new entries start on a clean line.
        if journal.metadata()?.len() != valid_len {
            journal.set_len(valid_len)?;
        }
        Ok(Buffer { id: meta.id.clone(), path, doc, history, disk_text, saved_version, journal, unsynced: false })
    }

    /// Deletes a buffer's journal, e.g. after it's closed.
    pub fn discard_buffer(&self, id: &str) -> io::Result<()> {
        match fs::remove_dir_all(self.buffer_dir(id)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

fn applies(tx: &Transaction, text: &str) -> bool {
    tx.edits().iter().all(|e| {
        e.range.start <= e.range.end
            && e.range.end <= text.len()
            && text.is_char_boundary(e.range.start)
            && text.is_char_boundary(e.range.end)
    })
}

/// Writes via a temporary file and rename so a crash never leaves a half-written file.
/// Writes through symlinks to their target and keeps the original file's permissions.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().map_or("file".into(), |n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{name}.marginal-{}", std::process::id()));
    let result = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Ok(meta) = fs::metadata(&target) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// An open document plus its journal.
pub struct Buffer {
    pub id: String,
    pub path: Option<PathBuf>,
    pub doc: Document,
    pub history: History,
    /// The text as last loaded from or saved to disk: the base for merging outside changes.
    pub disk_text: String,
    saved_version: usize,
    journal: File,
    unsynced: bool,
}

impl Buffer {
    fn record(&mut self, entry: &Entry) -> io::Result<()> {
        let mut line = serde_json::to_vec(entry).map_err(io::Error::other)?;
        line.push(b'\n');
        // One write per entry: once it returns, the edit survives an app crash. `sync` makes it
        // survive a power loss too, and is called when the user pauses.
        self.journal.write_all(&line)?;
        self.unsynced = true;
        Ok(())
    }

    pub fn edit(&mut self, tx: Transaction, before: Selection, after: Selection, kind: EditKind) -> io::Result<()> {
        if tx.is_empty() {
            return Ok(());
        }
        let time = now_ms();
        self.history.apply(&mut self.doc, tx.clone(), before, after, kind, time);
        self.record(&Entry::Edit { tx, before, after, kind, time })
    }

    pub fn undo(&mut self) -> io::Result<Option<Selection>> {
        let selection = self.history.undo(&mut self.doc);
        if selection.is_some() {
            self.record(&Entry::Undo)?;
        }
        Ok(selection)
    }

    pub fn redo(&mut self) -> io::Result<Option<Selection>> {
        let selection = self.history.redo(&mut self.doc);
        if selection.is_some() {
            self.record(&Entry::Redo)?;
        }
        Ok(selection)
    }

    /// Writes the document to its file. The only path by which Marginal touches user files.
    pub fn save(&mut self) -> io::Result<()> {
        let path = self.path.clone().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "untitled buffer"))?;
        self.save_as(&path)
    }

    pub fn save_as(&mut self, path: &Path) -> io::Result<()> {
        write_atomic(path, self.doc.text().as_bytes())?;
        self.path = Some(path.to_path_buf());
        self.disk_text = self.doc.text().to_owned();
        self.saved_version = self.history.version();
        self.record(&Entry::Saved { path: path.to_path_buf() })?;
        self.sync()
    }

    /// True when the text differs from the last save (undoing back to it makes this false).
    pub fn is_dirty(&self) -> bool {
        self.history.version() != self.saved_version
    }

    /// Flushes the journal to stable storage.
    pub fn sync(&mut self) -> io::Result<()> {
        if self.unsynced {
            self.journal.sync_data()?;
            self.unsynced = false;
        }
        Ok(())
    }

    pub fn meta(&self, selection: Selection, scroll: f32) -> BufferMeta {
        BufferMeta { id: self.id.clone(), path: self.path.clone(), selection, scroll }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        let _ = self.sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(at: usize) -> Selection {
        Selection::caret(at)
    }

    #[test]
    fn edits_survive_reopen_with_undo_history() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path().join("data")).unwrap();
        let file = tmp.path().join("note.md");
        fs::write(&file, "# Hi\r\n").unwrap();

        let mut buf = store.create_buffer(Some(&file), "# Hi\r\n").unwrap();
        buf.edit(Transaction::insert(4, " there"), sel(4), sel(10), EditKind::Typing).unwrap();
        buf.edit(Transaction::insert(10, "!"), sel(10), sel(11), EditKind::Command).unwrap();
        buf.undo().unwrap();
        let meta = buf.meta(sel(10), 0.0);
        drop(buf); // like a crash: nothing saved

        assert_eq!(fs::read_to_string(&file).unwrap(), "# Hi\r\n", "the user's file is untouched");
        let mut buf = store.open_buffer(&meta).unwrap();
        assert_eq!(buf.doc.text(), "# Hi there\r\n");
        assert!(buf.is_dirty());
        assert_eq!(buf.redo().unwrap(), Some(sel(11)), "redo survives restarts");
        assert_eq!(buf.doc.text(), "# Hi there!\r\n");
        buf.undo().unwrap();
        buf.undo().unwrap();
        assert_eq!(buf.doc.text(), "# Hi\r\n");
        assert!(!buf.is_dirty(), "undoing back to the saved text is clean");
    }

    #[test]
    fn save_writes_file_and_marks_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path().join("data")).unwrap();
        let file = tmp.path().join("a.md");
        fs::write(&file, "x").unwrap();
        let mut buf = store.create_buffer(Some(&file), "x").unwrap();
        buf.edit(Transaction::insert(1, "y"), sel(1), sel(2), EditKind::Typing).unwrap();
        buf.save().unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "xy");
        assert!(!buf.is_dirty());
        let meta = buf.meta(sel(2), 0.0);
        drop(buf);
        let buf = store.open_buffer(&meta).unwrap();
        assert_eq!(buf.disk_text, "xy");
        assert!(!buf.is_dirty());
    }

    #[test]
    fn torn_journal_tail_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path()).unwrap();
        let mut buf = store.create_buffer(None, "").unwrap();
        buf.edit(Transaction::insert(0, "ok"), sel(0), sel(2), EditKind::Typing).unwrap();
        let meta = buf.meta(sel(2), 0.0);
        drop(buf);
        let journal = tmp.path().join("buffers").join(&meta.id).join("journal.jsonl");
        let mut f = OpenOptions::new().append(true).open(&journal).unwrap();
        f.write_all(br#"{"op":"edit","tx":{"edits":[{"ran"#).unwrap();
        drop(f);
        let mut buf = store.open_buffer(&meta).unwrap();
        assert_eq!(buf.doc.text(), "ok");
        buf.edit(Transaction::insert(2, "!"), sel(2), sel(3), EditKind::Typing).unwrap();
        drop(buf);
        assert_eq!(store.open_buffer(&meta).unwrap().doc.text(), "ok!", "new entries land after the torn tail is cut");
    }

    #[test]
    fn saves_through_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real.md");
        let link = tmp.path().join("link.md");
        fs::write(&real, "a").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, b"b").unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "link is preserved");
        assert_eq!(fs::read_to_string(&real).unwrap(), "b");
    }

    #[test]
    fn session_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path()).unwrap();
        assert_eq!(store.load_session(), SessionFile::default());
        let session = SessionFile {
            buffers: vec![BufferMeta { id: "a".into(), path: None, selection: sel(3), scroll: 12.0 }],
            active: Some("a".into()),
            recent_languages: vec!["rust".into()],
            ..Default::default()
        };
        store.save_session(&session).unwrap();
        assert_eq!(store.load_session(), session);
    }
}
