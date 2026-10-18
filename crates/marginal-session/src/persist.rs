//! A background thread for the slow half of persistence: flushing to stable storage. On macOS
//! that means `F_FULLFSYNC`, which can stall for milliseconds, so the UI thread only does the
//! cheap writes and hands the flushes here. Durability is the same; it just isn't waited on.

use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use crate::write_atomic;

enum Job {
    /// Flush a journal (a duplicated handle) to stable storage.
    Sync(File),
    /// Replace a file atomically. Only the newest pending write per path is kept.
    Write(PathBuf, Vec<u8>),
    /// Reply once everything queued before this is done.
    Flush(Sender<()>),
}

pub struct Persister {
    tx: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Persister {
    pub fn start() -> Self {
        let (tx, rx) = channel();
        let thread = std::thread::Builder::new()
            .name("marginal-persist".into())
            .spawn(move || run(rx))
            .expect("spawn persistence thread");
        Self { tx: Some(tx), thread: Some(thread) }
    }

    fn send(&self, job: Job) {
        if let Some(tx) = &self.tx {
            // The thread only stops when we drop the sender, so this can't fail in practice.
            let _ = tx.send(job);
        }
    }

    pub fn sync(&self, journal: File) {
        self.send(Job::Sync(journal));
    }

    pub fn write(&self, path: PathBuf, bytes: Vec<u8>) {
        self.send(Job::Write(path, bytes));
    }

    /// Blocks until every job queued so far has finished.
    pub fn flush(&self) {
        let (done, wait) = channel();
        self.send(Job::Flush(done));
        let _ = wait.recv();
    }
}

impl Drop for Persister {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(rx: Receiver<Job>) {
    while let Ok(first) = rx.recv() {
        // Take everything already queued so repeated session writes collapse into one.
        let mut syncs = Vec::new();
        let mut writes: HashMap<PathBuf, Vec<u8>> = HashMap::new();
        let mut order = Vec::new();
        let mut acks = Vec::new();
        for job in std::iter::once(first).chain(rx.try_iter()) {
            match job {
                Job::Sync(file) => syncs.push(file),
                Job::Write(path, bytes) => {
                    if writes.insert(path.clone(), bytes).is_none() {
                        order.push(path);
                    }
                }
                Job::Flush(ack) => acks.push(ack),
            }
        }
        for path in order {
            if let Some(bytes) = writes.remove(&path)
                && let Err(e) = write_atomic(&path, &bytes)
            {
                eprintln!("could not write {}: {e}", path.display());
            }
        }
        for file in syncs {
            if let Err(e) = file.sync_data() {
                eprintln!("journal sync failed: {e}");
            }
        }
        for ack in acks {
            let _ = ack.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_land_and_coalesce() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let p = Persister::start();
        for i in 0..50 {
            p.write(path.clone(), format!("{i}").into_bytes());
        }
        p.flush();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "49", "the newest write wins");
        let journal = File::create(dir.path().join("journal")).unwrap();
        p.sync(journal);
        drop(p); // joins after finishing the queue
    }
}
