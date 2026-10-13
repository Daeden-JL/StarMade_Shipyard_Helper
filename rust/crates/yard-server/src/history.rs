//! The versions of each design that were in step with the game server.
//!
//! Every time a design and its blueprint in the game come to hold the same
//! thing, the design's file at that moment is kept: the last
//! [`SYNC_VERSIONS_KEPT`] per design, in `<data>/history/<design id>/` as
//! `<seq>.sment` beside an `index.json` that describes them. A kept file is a
//! hard link of the stored design wherever possible, so a version that is
//! still the current one takes no room of its own. That is safe because the
//! store never writes into a design file: it puts a new file in its place.

use crate::store::Store;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use yard_core::api::{Summary, SyncDirection, SyncVersion, SYNC_VERSIONS_KEPT};

/// A design file handed to the game is forgotten if the game has not confirmed
/// it by the time the server next starts and this much time has passed.
const PENDING_MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// A file the game's agent has downloaded and not yet confirmed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Pending {
    name: String,
    blocks: u64,
    entities: usize,
    print: String,
}

/// What `index.json` holds.
#[derive(Default, Serialize, Deserialize)]
struct Index {
    /// Oldest first.
    #[serde(default)]
    versions: Vec<SyncVersion>,
    #[serde(default)]
    pending: Option<Pending>,
}

pub struct History {
    dir: PathBuf,
    /// One change at a time: the index is read, changed and written back.
    lock: Mutex<()>,
}

/// Puts `from` at `to` as a second name for the same file, or as a copy where
/// the file system cannot do that.
pub fn link_or_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(to);
    if std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map(|_| ())
}

fn version_of(summary: &Summary, direction: SyncDirection, time: u64) -> SyncVersion {
    SyncVersion {
        seq: 0,
        time,
        direction,
        name: summary.name.clone(),
        blocks: summary.blocks,
        entities: summary.entities.len(),
        bytes: 0,
        print: summary.print.clone(),
        current: false,
    }
}

impl History {
    pub fn open(data_dir: &Path) -> Self {
        Self { dir: data_dir.join("history"), lock: Mutex::new(()) }
    }

    /// A design's folder. Ids come from URLs: only what the store would
    /// accept gets near the file system.
    /// Moves a design's sync history to `to`, if it has one. For a design that
    /// is being deleted: its versions go with it, into the trash.
    pub fn set_aside(&self, id: &str, to: &Path) -> anyhow::Result<()> {
        let _guard = self.lock.lock().unwrap();
        let Some(folder) = self.folder(id).filter(|f| f.exists()) else { return Ok(()) };
        std::fs::rename(&folder, to).with_context(|| format!("cannot move {} away", folder.display()))
    }

    fn folder(&self, id: &str) -> Option<PathBuf> {
        Store::valid_id(id).then(|| self.dir.join(id))
    }

    fn read(folder: &Path) -> Index {
        match std::fs::read(folder.join("index.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::error!("unreadable sync history in {}: {e}", folder.display());
                Index::default()
            }),
            Err(_) => Index::default(),
        }
    }

    /// Whole file or nothing.
    fn write(folder: &Path, index: &Index) -> anyhow::Result<()> {
        let fresh = folder.join("index.json.new");
        std::fs::write(&fresh, serde_json::to_vec_pretty(index)?)?;
        std::fs::rename(&fresh, folder.join("index.json"))?;
        Ok(())
    }

    fn pending_file(folder: &Path, print: &str) -> PathBuf {
        folder.join(format!("pending-{print}.sment"))
    }

    /// Forgets the file waiting for the game's confirmation, if there is one.
    fn drop_pending(folder: &Path, index: &mut Index) {
        if let Some(old) = index.pending.take() {
            let _ = std::fs::remove_file(Self::pending_file(folder, &old.print));
        }
    }

    /// Makes `version` the newest one, taking the file at `file` (which is in
    /// `folder`) as its contents, and drops what is now too old. Returns None,
    /// and removes `file`, if the newest version already has these contents.
    fn add(folder: &Path, index: &mut Index, mut version: SyncVersion, file: &Path) -> anyhow::Result<Option<SyncVersion>> {
        if index.versions.last().is_some_and(|newest| newest.print == version.print) {
            let _ = std::fs::remove_file(file);
            return Ok(None);
        }
        version.seq = index.versions.last().map_or(1, |newest| newest.seq + 1);
        version.bytes = std::fs::metadata(file)?.len();
        version.current = false;
        std::fs::rename(file, folder.join(format!("{}.sment", version.seq)))?;
        index.versions.push(version.clone());
        // Whatever was waiting for the game is out of date now. Not so when
        // this version never went near the game: the game may yet confirm.
        if version.direction != SyncDirection::Unsynced {
            Self::drop_pending(folder, index);
        }
        let extra = index.versions.len().saturating_sub(SYNC_VERSIONS_KEPT);
        let dropped: Vec<SyncVersion> = index.versions.drain(..extra).collect();
        Self::write(folder, index)?;
        for old in dropped {
            let _ = std::fs::remove_file(folder.join(format!("{}.sment", old.seq)));
        }
        Ok(Some(version))
    }

    /// A design's versions, oldest first.
    pub fn list(&self, id: &str) -> Vec<SyncVersion> {
        let Some(folder) = self.folder(id) else { return Vec::new() };
        let _guard = self.lock.lock().unwrap();
        Self::read(&folder).versions
    }

    pub fn version(&self, id: &str, seq: u64) -> Option<SyncVersion> {
        self.list(id).into_iter().find(|v| v.seq == seq)
    }

    /// The `.sment` of one version.
    pub fn file(&self, id: &str, seq: u64) -> Option<Vec<u8>> {
        self.version(id, seq)?;
        std::fs::read(self.folder(id)?.join(format!("{seq}.sment"))).ok()
    }

    /// Keeps the design's stored file as a new version, provided the store
    /// still holds the version `summary` describes. If it has moved on,
    /// `bytes` (that version's file, where the caller has it) is kept instead.
    /// Returns None if nothing was added: the newest version is this one
    /// already, or the version is gone.
    pub fn record(
        &self,
        store: &Store,
        summary: &Summary,
        direction: SyncDirection,
        time: u64,
        bytes: Option<&[u8]>,
    ) -> anyhow::Result<Option<SyncVersion>> {
        let folder = self.folder(&summary.id).context("not a design id")?;
        let _guard = self.lock.lock().unwrap();
        let mut index = Self::read(&folder);
        if index.versions.last().is_some_and(|newest| newest.print == summary.print) {
            return Ok(None);
        }
        std::fs::create_dir_all(&folder)?;
        let incoming = folder.join("incoming.tmp");
        if !Self::take_stored(store, summary, &incoming, bytes)? {
            return Ok(None);
        }
        Self::add(&folder, &mut index, version_of(summary, direction, time), &incoming)
    }

    /// Puts the file of the version `summary` describes at `to`: the stored
    /// file if it is still that version, else `bytes`. False if neither.
    fn take_stored(store: &Store, summary: &Summary, to: &Path, bytes: Option<&[u8]>) -> anyhow::Result<bool> {
        if store.snapshot(&summary.id, to).is_ok_and(|stored| stored.print == summary.print) {
            return Ok(true);
        }
        let _ = std::fs::remove_file(to);
        match bytes {
            Some(bytes) => {
                std::fs::write(to, bytes)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Sets aside the file a game's agent is downloading: `bytes`, the version
    /// `summary` describes. It becomes a version once the agent confirms that
    /// the game has it ([`History::confirm`]).
    pub fn hold(&self, store: &Store, summary: &Summary, bytes: &[u8]) -> anyhow::Result<()> {
        let folder = self.folder(&summary.id).context("not a design id")?;
        let _guard = self.lock.lock().unwrap();
        let mut index = Self::read(&folder);
        let held = Pending { name: summary.name.clone(), blocks: summary.blocks, entities: summary.entities.len(), print: summary.print.clone() };
        let file = Self::pending_file(&folder, &held.print);
        if index.pending.as_ref() == Some(&held) && file.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&folder)?;
        Self::drop_pending(&folder, &mut index);
        Self::take_stored(store, summary, &file, Some(bytes))?;
        index.pending = Some(held);
        Self::write(&folder, &index)
    }

    /// The game has the version of design `id` with this fingerprint: the
    /// file set aside when it was downloaded becomes the newest version. If no
    /// file was set aside, the stored design is taken, if it still is that
    /// version.
    pub fn confirm(&self, store: &Store, id: &str, print: &str, time: u64) -> anyhow::Result<Option<SyncVersion>> {
        let folder = self.folder(id).context("not a design id")?;
        let _guard = self.lock.lock().unwrap();
        let mut index = Self::read(&folder);
        if let Some(newest) = index.versions.last_mut().filter(|newest| newest.print == print) {
            // Kept as edits the game never saw, a moment before the game
            // said that it has them after all.
            let reached_the_game = newest.direction == SyncDirection::Unsynced;
            if reached_the_game {
                newest.direction = SyncDirection::ToGame;
                newest.time = time;
            }
            let newest = newest.clone();
            Self::drop_pending(&folder, &mut index);
            Self::write(&folder, &index)?;
            return Ok(reached_the_game.then_some(newest));
        }
        let file = Self::pending_file(&folder, print);
        let held = index.pending.clone().filter(|p| p.print == print && file.exists());
        let version = match held {
            Some(p) => SyncVersion {
                seq: 0,
                time,
                direction: SyncDirection::ToGame,
                name: p.name,
                blocks: p.blocks,
                entities: p.entities,
                bytes: 0,
                print: p.print,
                current: false,
            },
            None => {
                let Some(summary) = store.summary(id).filter(|s| s.print == print) else { return Ok(None) };
                std::fs::create_dir_all(&folder)?;
                if !Self::take_stored(store, &summary, &file, None)? {
                    return Ok(None);
                }
                version_of(&summary, SyncDirection::ToGame, time)
            }
        };
        Self::add(&folder, &mut index, version, &file)
    }

    /// Tidies up after a server that stopped in the middle of something:
    /// half-made files, and downloads the game never confirmed.
    pub fn sweep(&self) {
        let _guard = self.lock.lock().unwrap();
        let Ok(folders) = std::fs::read_dir(&self.dir) else { return };
        for folder in folders.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
            let _ = std::fs::remove_file(folder.join("incoming.tmp"));
            let _ = std::fs::remove_file(folder.join("index.json.new"));
            let mut index = Self::read(&folder);
            let Some(held) = &index.pending else { continue };
            let age = std::fs::metadata(folder.join("index.json"))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|at| SystemTime::now().duration_since(at).ok());
            if age.is_none_or(|age| age >= PENDING_MAX_AGE) || !Self::pending_file(&folder, &held.print).exists() {
                Self::drop_pending(&folder, &mut index);
                if let Err(e) = Self::write(&folder, &index) {
                    tracing::error!("tidying sync history in {}: {e:#}", folder.display());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yard_core::{Block, Blueprint, Pos};

    /// A data directory that is removed when the test ends, pass or fail.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("yard-history-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const ID: &str = "abcdefghjkmn";

    /// A ship with a core and `hull` more blocks, as the store would hold it.
    fn ship(hull: i32) -> (Summary, Vec<u8>) {
        let mut bp = Blueprint::new_ship("Scout", "test");
        for x in 1..=hull {
            bp.root.entity.set(Pos::new(x, 0, 0), Block::new(5, 0, false));
        }
        bp.root.header.refresh(&bp.root.entity);
        let summary = Summary::describe(ID, &bp, 1, |_| Some("Block".to_string()));
        (summary, bp.to_sment().unwrap())
    }

    fn open(scratch: &Scratch) -> (Store, History) {
        (Store::open(&scratch.0.join("designs")).unwrap(), History::open(&scratch.0))
    }

    #[test]
    fn a_kept_version_keeps_its_bytes_when_the_design_is_saved_again() {
        let scratch = Scratch::new("bytes");
        let (store, history) = open(&scratch);
        let (summary, first) = ship(1);
        let stored = store.save(summary, &first).unwrap();
        let kept = history.record(&store, &stored, SyncDirection::FromGame, 100, None).unwrap().unwrap();
        assert_eq!((kept.seq, kept.time, kept.direction, kept.blocks), (1, 100, SyncDirection::FromGame, 2));
        assert_eq!((kept.name.as_str(), kept.entities, kept.bytes), ("Scout", 1, first.len() as u64));
        assert_eq!(kept.print, stored.print);

        // The design moves on; the version does not.
        let (summary, second) = ship(5);
        let stored = store.save(summary, &second).unwrap();
        assert_ne!(first, second);
        assert_eq!(history.file(ID, 1).unwrap(), first);
        assert_eq!(store.file(ID).unwrap(), second);
        history.record(&store, &stored, SyncDirection::ToGame, 200, None).unwrap().unwrap();
        assert_eq!(history.file(ID, 1).unwrap(), first);
        assert_eq!(history.file(ID, 2).unwrap(), second);
        assert_eq!(history.list(ID).iter().map(|v| v.seq).collect::<Vec<_>>(), vec![1, 2]);
        assert!(history.file(ID, 3).is_none() && history.version(ID, 0).is_none());
    }

    #[test]
    fn the_newest_version_is_not_kept_twice() {
        let scratch = Scratch::new("twice");
        let (store, history) = open(&scratch);
        let (summary, bytes) = ship(1);
        let first = store.save(summary, &bytes).unwrap();
        assert!(history.record(&store, &first, SyncDirection::Matched, 1, None).unwrap().is_some());
        assert!(history.record(&store, &first, SyncDirection::ToGame, 2, None).unwrap().is_none());
        assert!(history.confirm(&store, ID, &first.print, 3).unwrap().is_none());
        assert_eq!(history.list(ID).len(), 1);
        // The same contents again after something else is a version of its own.
        let (summary, other) = ship(2);
        let second = store.save(summary, &other).unwrap();
        history.record(&store, &second, SyncDirection::FromGame, 4, None).unwrap().unwrap();
        let (summary, bytes) = ship(1);
        let back = store.save(summary, &bytes).unwrap();
        assert_eq!(back.print, first.print);
        history.record(&store, &back, SyncDirection::ToGame, 5, None).unwrap().unwrap();
        assert_eq!(history.list(ID).iter().map(|v| (v.seq, v.direction)).collect::<Vec<_>>(), vec![
            (1, SyncDirection::Matched),
            (2, SyncDirection::FromGame),
            (3, SyncDirection::ToGame)
        ]);
    }

    #[test]
    fn only_the_newest_versions_are_kept() {
        let scratch = Scratch::new("cap");
        let (store, history) = open(&scratch);
        let total = SYNC_VERSIONS_KEPT as i32 + 3;
        for hull in 1..=total {
            let (summary, bytes) = ship(hull);
            let stored = store.save(summary, &bytes).unwrap();
            history.record(&store, &stored, SyncDirection::ToGame, hull as u64, None).unwrap().unwrap();
        }
        let kept = history.list(ID);
        assert_eq!(kept.len(), SYNC_VERSIONS_KEPT);
        assert_eq!((kept[0].seq, kept.last().unwrap().seq), (4, total as u64));
        // The files of the dropped ones are gone, and nothing else is.
        let folder = scratch.0.join("history").join(ID);
        for seq in 1..=3 {
            assert!(!folder.join(format!("{seq}.sment")).exists());
            assert!(history.file(ID, seq).is_none());
        }
        let mut files: Vec<String> = std::fs::read_dir(&folder).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        files.sort();
        assert_eq!(files.len(), SYNC_VERSIONS_KEPT + 1, "{files:?}");
        assert_eq!(history.file(ID, 4).unwrap(), ship(4).1);
    }

    #[test]
    fn a_download_becomes_a_version_when_the_game_confirms_it() {
        let scratch = Scratch::new("pending");
        let (store, history) = open(&scratch);
        let (summary, sent) = ship(1);
        let stored = store.save(summary, &sent).unwrap();
        history.hold(&store, &stored, &sent).unwrap();
        // Not a version yet: the game may still turn it down.
        assert!(history.list(ID).is_empty());
        // The design is edited before the game answers.
        let (summary, newer) = ship(9);
        let edited = store.save(summary, &newer).unwrap();
        let version = history.confirm(&store, ID, &stored.print, 50).unwrap().unwrap();
        assert_eq!((version.seq, version.direction, version.blocks, version.time), (1, SyncDirection::ToGame, 2, 50));
        assert_eq!(history.file(ID, 1).unwrap(), sent);
        let folder = scratch.0.join("history").join(ID);
        assert!(!History::pending_file(&folder, &stored.print).exists());

        // A confirmation for something that was never set aside takes the
        // stored design if it is that version, and nothing otherwise.
        assert!(history.confirm(&store, ID, "0000", 60).unwrap().is_none());
        let version = history.confirm(&store, ID, &edited.print, 70).unwrap().unwrap();
        assert_eq!((version.seq, version.blocks), (2, 10));
        assert_eq!(history.file(ID, 2).unwrap(), newer);

        // A download that is overtaken by another, or by a version from the
        // game, is forgotten.
        let (summary, a) = ship(3);
        let a_stored = store.save(summary, &a).unwrap();
        history.hold(&store, &a_stored, &a).unwrap();
        let (summary, b) = ship(4);
        let b_stored = store.save(summary, &b).unwrap();
        history.hold(&store, &b_stored, &b).unwrap();
        assert!(!History::pending_file(&folder, &a_stored.print).exists());
        assert!(History::pending_file(&folder, &b_stored.print).exists());
        let (summary, c) = ship(5);
        let c_stored = store.save(summary, &c).unwrap();
        history.record(&store, &c_stored, SyncDirection::FromGame, 80, None).unwrap().unwrap();
        assert!(!History::pending_file(&folder, &b_stored.print).exists());
        assert!(history.confirm(&store, ID, &b_stored.print, 90).unwrap().is_none());
        assert_eq!(history.list(ID).len(), 3);
    }

    #[test]
    fn edits_the_game_never_saw_can_be_kept_without_forgetting_a_download_under_way() {
        let scratch = Scratch::new("unsynced");
        let (store, history) = open(&scratch);
        let (summary, sent) = ship(1);
        let stored = store.save(summary, &sent).unwrap();
        history.hold(&store, &stored, &sent).unwrap();
        // The design is edited on, and those edits are set aside (an earlier
        // version is about to be put back over them).
        let (summary, edits) = ship(2);
        let edited = store.save(summary, &edits).unwrap();
        let version = history.record(&store, &edited, SyncDirection::Unsynced, 10, None).unwrap().unwrap();
        assert_eq!((version.seq, version.direction), (1, SyncDirection::Unsynced));
        // The game then confirms the download from before.
        let version = history.confirm(&store, ID, &stored.print, 20).unwrap().unwrap();
        assert_eq!((version.seq, version.direction), (2, SyncDirection::ToGame));
        assert_eq!((history.file(ID, 1).unwrap(), history.file(ID, 2).unwrap()), (edits.clone(), sent));

        // Edits that were being downloaded just as they were set aside: when
        // the game confirms, they are no longer something it never saw.
        let (summary, more) = ship(3);
        let more_stored = store.save(summary, &more).unwrap();
        history.hold(&store, &more_stored, &more).unwrap();
        history.record(&store, &more_stored, SyncDirection::Unsynced, 30, None).unwrap().unwrap();
        let version = history.confirm(&store, ID, &more_stored.print, 40).unwrap().unwrap();
        assert_eq!((version.seq, version.direction, version.time), (3, SyncDirection::ToGame, 40));
        let folder = scratch.0.join("history").join(ID);
        assert!(!History::pending_file(&folder, &more_stored.print).exists());
        assert_eq!(history.list(ID).iter().map(|v| v.direction).collect::<Vec<_>>(), vec![
            SyncDirection::Unsynced,
            SyncDirection::ToGame,
            SyncDirection::ToGame
        ]);
        assert_eq!(history.file(ID, 3).unwrap(), more);
    }

    #[test]
    fn a_version_the_store_no_longer_holds_is_kept_from_its_bytes_or_not_at_all() {
        let scratch = Scratch::new("moved");
        let (store, history) = open(&scratch);
        let (summary, old) = ship(1);
        let was = store.save(summary, &old).unwrap();
        let (summary, new) = ship(2);
        store.save(summary, &new).unwrap();
        assert!(history.record(&store, &was, SyncDirection::ToGame, 1, None).unwrap().is_none());
        assert!(history.list(ID).is_empty());
        history.record(&store, &was, SyncDirection::ToGame, 1, Some(&old)).unwrap().unwrap();
        assert_eq!(history.file(ID, 1).unwrap(), old);
        // Ids that could not be a design's never reach the file system.
        assert!(history.list("../designs").is_empty());
        assert!(history.file("..", 1).is_none());
    }

    #[test]
    fn downloads_the_game_never_confirmed_are_cleared_at_startup() {
        let scratch = Scratch::new("sweep");
        let (store, history) = open(&scratch);
        let (summary, sent) = ship(1);
        let stored = store.save(summary, &sent).unwrap();
        history.hold(&store, &stored, &sent).unwrap();
        let folder = scratch.0.join("history").join(ID);
        std::fs::write(folder.join("incoming.tmp"), b"half").unwrap();
        // A fresh one is left alone: the game may be importing it right now.
        history.sweep();
        assert!(History::pending_file(&folder, &stored.print).exists());
        assert!(!folder.join("incoming.tmp").exists());
        // One whose file is gone is forgotten.
        std::fs::remove_file(History::pending_file(&folder, &stored.print)).unwrap();
        history.sweep();
        assert!(History::read(&folder).pending.is_none());
    }
}
