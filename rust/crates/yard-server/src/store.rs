//! Designs on disk: each is the uploaded blueprint plus a small summary file.

use anyhow::Context;
use rand::Rng;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use yard_core::api::Summary;

pub struct Store {
    dir: PathBuf,
    summaries: RwLock<BTreeMap<String, Summary>>,
    /// Held while a design's file and summary are put in place, so that
    /// [`Store::snapshot`] never sees one without the other.
    writing: Mutex<()>,
}

/// Writes a file so that readers see the old contents or the new, never a
/// part: the new contents go to a file beside it, which then takes its name.
/// The old file itself is not written to, so other names it has (the sync
/// history keeps hard links of design files) keep the old contents.
fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut fresh = path.as_os_str().to_owned();
    fresh.push(".new");
    let fresh = PathBuf::from(fresh);
    std::fs::write(&fresh, bytes)?;
    std::fs::rename(&fresh, path)
}

impl Store {
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let mut summaries = BTreeMap::new();
        for entry in std::fs::read_dir(dir)?.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "new") {
                // Left behind by a server that stopped in the middle of a save.
                let _ = std::fs::remove_file(&path);
            } else if path.extension().is_some_and(|e| e == "json") {
                match std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Summary>(&b).ok()) {
                    Some(mut s) => {
                        // Designs stored before fingerprints existed get theirs now.
                        if s.print.is_empty() {
                            let file = std::fs::read(path.with_extension("sment")).ok();
                            s.print = file.and_then(|bytes| crate::sync::print_of_sment(&bytes)).unwrap_or_default();
                            s.modified = s.created;
                            write_whole(&path, &serde_json::to_vec_pretty(&s)?)?;
                        }
                        summaries.insert(s.id.clone(), s);
                    }
                    None => tracing::warn!("ignoring unreadable summary {}", path.display()),
                }
            }
        }
        Ok(Self { dir: dir.to_path_buf(), summaries: RwLock::new(summaries), writing: Mutex::new(()) })
    }

    /// Ids are random, not sequential: while the site has no accounts, knowing a
    /// design's id is what gives access to it.
    pub fn new_id() -> String {
        const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
        let mut rng = rand::thread_rng();
        (0..12).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
    }

    /// Ids come from URLs, so only accept what `new_id` can produce before
    /// touching the filesystem with one.
    pub(crate) fn valid_id(id: &str) -> bool {
        id.len() == 12 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    }

    /// Stores a design's file with its summary, and returns the summary as
    /// stored: with the file's fingerprint and the time of the change.
    pub fn save(&self, summary: Summary, sment: &[u8]) -> anyhow::Result<Summary> {
        self.write(summary, sment, false)
    }

    /// Like [`Store::save`], for a file that did not come from the design's
    /// editors: open editors are told, through the summary, to load it again.
    pub fn replace(&self, summary: Summary, sment: &[u8]) -> anyhow::Result<Summary> {
        self.write(summary, sment, true)
    }

    fn write(&self, mut summary: Summary, sment: &[u8], from_outside: bool) -> anyhow::Result<Summary> {
        anyhow::ensure!(Self::valid_id(&summary.id), "not a design id");
        let _writing = self.writing.lock().unwrap();
        let epoch = self.summaries.read().unwrap().get(&summary.id).map_or(0, |old| old.epoch);
        summary.epoch = epoch + from_outside as u64;
        summary.print = crate::sync::print_of_sment(sment).context("the design is not a readable blueprint")?;
        summary.modified = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        summary.sync = None;
        write_whole(&self.dir.join(format!("{}.sment", summary.id)), sment)?;
        write_whole(&self.dir.join(format!("{}.json", summary.id)), &serde_json::to_vec_pretty(&summary)?)?;
        self.summaries.write().unwrap().insert(summary.id.clone(), summary.clone());
        Ok(summary)
    }

    /// Whether any design has this name, ignoring case.
    pub fn has_name(&self, name: &str) -> bool {
        let wanted = name.to_lowercase();
        self.summaries.read().unwrap().values().any(|s| s.name.to_lowercase() == wanted)
    }

    pub fn list(&self) -> Vec<Summary> {
        let mut all: Vec<_> = self.summaries.read().unwrap().values().cloned().collect();
        all.sort_by(|a, b| b.created.cmp(&a.created));
        all
    }

    pub fn summary(&self, id: &str) -> Option<Summary> {
        self.summaries.read().unwrap().get(id).cloned()
    }

    pub fn file(&self, id: &str) -> Option<Vec<u8>> {
        if !Self::valid_id(id) {
            return None;
        }
        std::fs::read(self.dir.join(format!("{id}.sment"))).ok()
    }

    /// Takes a design out of the list and sets its files aside in the `trash`
    /// folder beside the designs' (named by the time, so that nothing is ever
    /// overwritten there): nothing is erased, an administrator can put the
    /// two files back. Returns the summary the design had.
    pub fn trash(&self, id: &str, when: u64) -> anyhow::Result<Summary> {
        anyhow::ensure!(Self::valid_id(id), "not a design id");
        let _writing = self.writing.lock().unwrap();
        let summary = self.summary(id).context("no such design")?;
        let bin = self.trash_dir();
        std::fs::create_dir_all(&bin).with_context(|| format!("cannot create {}", bin.display()))?;
        for extension in ["sment", "json"] {
            let from = self.dir.join(format!("{id}.{extension}"));
            if from.exists() {
                std::fs::rename(&from, bin.join(format!("{id}-{when}.{extension}"))).with_context(|| format!("cannot move {} away", from.display()))?;
            }
        }
        self.summaries.write().unwrap().remove(id);
        Ok(summary)
    }

    /// Where deleted designs' files are set aside.
    pub fn trash_dir(&self) -> PathBuf {
        self.dir.parent().unwrap_or(&self.dir).join("trash")
    }

    /// Puts the design's file as it is right now at `to`, as a hard link where
    /// the file system allows and as a copy where not, and returns the summary
    /// that describes exactly that file. No save can fall between the two.
    pub fn snapshot(&self, id: &str, to: &Path) -> anyhow::Result<Summary> {
        anyhow::ensure!(Self::valid_id(id), "not a design id");
        let _writing = self.writing.lock().unwrap();
        let summary = self.summary(id).context("no such design")?;
        crate::history::link_or_copy(&self.dir.join(format!("{id}.sment")), to)
            .with_context(|| format!("cannot keep a copy of design {id}"))?;
        Ok(summary)
    }
}
