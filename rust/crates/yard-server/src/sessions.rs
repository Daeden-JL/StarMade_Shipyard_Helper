//! Designs that are being edited: the in-memory copy every step is applied to,
//! its undo history, and the background saving of both.

use crate::store::Store;
use anyhow::Context;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use yard_core::api::{StepRequest, StepResponse, Summary};
use yard_core::catalog::Catalog;
use yard_core::edit::{self, Change, EditError, Journal, Op};
use yard_core::links::{Kind, Rules};
use yard_core::Blueprint;

/// A design is written to disk once it has been left alone this long.
const SAVE_AFTER_QUIET: Duration = Duration::from_millis(1500);
/// An unchanged design nobody has touched for this long is dropped from memory.
const FORGET_AFTER: Duration = Duration::from_secs(30 * 60);

pub struct OpenDesign {
    pub blueprint: Blueprint,
    /// Undo history per entity path.
    journals: HashMap<String, Journal>,
    /// Goes up by one with every step. Starts at the time the design was opened
    /// so that a number from before a server restart can never match by chance.
    pub rev: u64,
    saved_rev: u64,
    created: u64,
    last_edit: Instant,
    last_use: Instant,
    /// The design as a file before its last change of structure (a core moved,
    /// a turret cut out), which no undo history can follow: kept so that it
    /// can be put back, once.
    before_structure: Option<Vec<u8>>,
}

/// Why a step was not applied.
pub enum StepError {
    NoSuchDesign,
    NoSuchEntity,
    /// The sender's copy of the design is out of date.
    Stale,
    Refused(EditError),
    /// The change cannot be made to this design, for a reason the player can read.
    Rejected(String),
    Broken(anyhow::Error),
}

/// A place in the open-designs table.
#[derive(Default)]
enum Slot {
    /// Reserved by the first request for a design, which is about to load it.
    #[default]
    Empty,
    Open(Box<OpenDesign>),
    /// Taken out of the table. A request that was waiting for this slot starts over.
    Gone,
}

type SharedSlot = Arc<Mutex<Slot>>;

#[derive(Default)]
pub struct Sessions {
    open: Mutex<HashMap<String, SharedSlot>>,
    /// The revision each design was at when it was dropped from memory for
    /// being idle. Its file is unchanged since, so it opens at that revision
    /// again and a page that kept it open stays in step.
    idle_revs: Mutex<HashMap<String, u64>>,
}

fn now_millis() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Sessions {
    /// Runs `f` on the open design, loading it from disk first if needed.
    /// Blocking: call from `spawn_blocking`.
    fn with<T>(&self, store: &Store, id: &str, f: impl FnOnce(&mut OpenDesign) -> Result<T, StepError>) -> Result<T, StepError> {
        let summary = store.summary(id).ok_or(StepError::NoSuchDesign)?;
        loop {
            // Take the slot under the table lock, load under the slot's own
            // lock: a big design loading does not hold up work on other designs,
            // and two requests for the same design cannot both load it.
            let slot = self.open.lock().unwrap().entry(id.to_string()).or_default().clone();
            let mut guard = slot.lock().unwrap();
            if matches!(*guard, Slot::Gone) {
                continue;
            }
            if matches!(*guard, Slot::Empty) {
                let loaded = store
                    .file(id)
                    .context("the design file is missing")
                    .and_then(|bytes| Blueprint::from_sment(&bytes).context("the stored design cannot be read"));
                match loaded {
                    Ok(blueprint) => {
                        let rev = self.idle_revs.lock().unwrap().remove(id).unwrap_or_else(now_millis);
                        *guard = Slot::Open(Box::new(OpenDesign {
                            blueprint,
                            journals: HashMap::new(),
                            rev,
                            saved_rev: rev,
                            created: summary.created,
                            last_edit: Instant::now(),
                            last_use: Instant::now(),
                            before_structure: None,
                        }));
                    }
                    Err(e) => {
                        self.open.lock().unwrap().remove(id);
                        *guard = Slot::Gone;
                        return Err(StepError::Broken(e));
                    }
                }
            }
            let Slot::Open(design) = &mut *guard else { unreachable!("loaded above") };
            design.last_use = Instant::now();
            return f(design);
        }
    }

    /// Applies one step from an editor.
    pub fn step(&self, store: &Store, catalog: &Catalog, id: &str, request: &StepRequest) -> Result<StepResponse, StepError> {
        self.with(store, id, |design| {
            if design.rev != request.rev {
                return Err(StepError::Stale);
            }
            let OpenDesign { blueprint, journals, .. } = design;
            let entity = blueprint.entity_mut(&request.entity).ok_or(StepError::NoSuchEntity)?;
            let journal = journals.entry(request.entity.clone()).or_default();
            // Links follow the game's rules, the same ones the editor applied.
            let rules = Rules::new(catalog, Kind::of_entity_type(entity.header.entity_type));
            let change = edit::run_with(&mut entity.entity, journal, &request.ops, &rules).map_err(StepError::Refused)?;
            let (undo, redo) = journal.depth();
            let from_history = matches!(request.ops.as_slice(), [Op::Undo] | [Op::Redo]);
            // After an edit "put back" would lose it.
            design.before_structure = None;
            design.rev += 1;
            design.last_edit = Instant::now();
            Ok(StepResponse { rev: design.rev, undo, redo, change: from_history.then_some(change) })
        })
    }

    /// Changes the structure of a design: what moves things about as a whole,
    /// which undo cannot follow. `change` works on the blueprint and must
    /// leave it as it was if it refuses. Every entity's undo history is
    /// dropped (its positions no longer fit); the design as it was is kept so
    /// that [`Sessions::restore`] can put it back once. Editors fall out of
    /// step and load the design again.
    pub fn restructure<T>(
        &self,
        store: &Store,
        id: &str,
        rev: u64,
        change: impl FnOnce(&mut Blueprint) -> Result<T, String>,
    ) -> Result<(u64, T), StepError> {
        self.with(store, id, |design| {
            if design.rev != rev {
                return Err(StepError::Stale);
            }
            let before = design.blueprint.to_sment().context("the design could not be written as a blueprint").map_err(StepError::Broken)?;
            let done = match change(&mut design.blueprint) {
                Ok(done) => done,
                Err(refusal) => {
                    // Whatever a refused change did halfway, the design goes back to what it was.
                    if let Ok(whole) = Blueprint::from_sment(&before) {
                        design.blueprint = whole;
                    }
                    return Err(StepError::Rejected(refusal));
                }
            };
            design.before_structure = Some(before);
            design.journals.clear();
            design.rev += 1;
            design.last_edit = Instant::now();
            Ok((design.rev, done))
        })
    }

    /// Puts a design back as it was before its last change of structure.
    pub fn restore(&self, store: &Store, id: &str, rev: u64) -> Result<u64, StepError> {
        self.with(store, id, |design| {
            if design.rev != rev {
                return Err(StepError::Stale);
            }
            let bytes = design.before_structure.take().ok_or_else(|| StepError::Rejected("There is nothing to put back.".into()))?;
            design.blueprint = Blueprint::from_sment(&bytes).context("the earlier design cannot be read").map_err(StepError::Broken)?;
            design.journals.clear();
            design.rev += 1;
            design.last_edit = Instant::now();
            Ok(design.rev)
        })
    }

    /// Applies a step that does not come from an editor (the chat agent's):
    /// it goes on top of whatever revision the design is at. Says which
    /// revision that was, which it is now, and what changed, so editors that
    /// were in step can follow.
    pub fn step_here(&self, store: &Store, catalog: &Catalog, id: &str, entity: &str, ops: &[Op]) -> Result<(u64, u64, Change), StepError> {
        self.with(store, id, |design| {
            let before = design.rev;
            let OpenDesign { blueprint, journals, .. } = design;
            let found = blueprint.entity_mut(entity).ok_or(StepError::NoSuchEntity)?;
            let journal = journals.entry(entity.to_string()).or_default();
            let rules = Rules::new(catalog, Kind::of_entity_type(found.header.entity_type));
            let change = edit::run_with(&mut found.entity, journal, ops, &rules).map_err(StepError::Refused)?;
            if change.is_empty() {
                return Ok((before, before, change));
            }
            design.before_structure = None;
            design.rev += 1;
            design.last_edit = Instant::now();
            Ok((before, design.rev, change))
        })
    }

    /// Runs `read` on an open design as it is now, loading it if need be.
    pub fn read<T>(&self, store: &Store, id: &str, read: impl FnOnce(&Blueprint) -> T) -> Result<T, StepError> {
        self.with(store, id, |design| Ok(read(&design.blueprint)))
    }

    /// The design as a `.sment` in its current state, with its revision. Saves
    /// first if there are edits not yet on disk, so the two always agree.
    pub fn file(&self, store: &Store, catalog: &Catalog, id: &str) -> Result<(Vec<u8>, u64), StepError> {
        self.with(store, id, |design| {
            if design.rev != design.saved_rev {
                let bytes = save(store, catalog, id, design).map_err(StepError::Broken)?;
                return Ok((bytes, design.rev));
            }
            let bytes = store.file(id).ok_or(StepError::NoSuchDesign)?;
            Ok((bytes, design.rev))
        })
    }

    /// The design's stored file, including any edits not yet on disk, without
    /// opening the design if nobody has it open: the game's sync agent asks
    /// for whole catalogs, and loading each ship into memory would be wasteful.
    pub fn stored_file(&self, store: &Store, catalog: &Catalog, id: &str) -> Result<Vec<u8>, StepError> {
        let slot = self.open.lock().unwrap().get(id).cloned();
        if let Some(slot) = slot {
            if let Slot::Open(design) = &mut *slot.lock().unwrap() {
                if design.rev != design.saved_rev {
                    return save(store, catalog, id, design).map_err(StepError::Broken);
                }
            }
        }
        store.file(id).ok_or(StepError::NoSuchDesign)
    }

    /// Whether a design has edits that are not on disk yet. Until they are,
    /// what the store says about the design (its fingerprint above all) is
    /// about an older state. A design that is busy this instant (being
    /// loaded, or in the middle of a step) is not waited for and counts as
    /// saved: if it was a step, the next asking sees it.
    pub fn unsaved(&self, id: &str) -> bool {
        let Some(slot) = self.open.lock().unwrap().get(id).cloned() else { return false };
        let unsaved = match slot.try_lock() {
            Ok(guard) => matches!(&*guard, Slot::Open(design) if design.rev != design.saved_rev),
            Err(_) => false,
        };
        unsaved
    }

    /// The ids of all designs with edits not on disk yet.
    pub fn all_unsaved(&self) -> Vec<String> {
        let ids: Vec<String> = self.open.lock().unwrap().keys().cloned().collect();
        ids.into_iter().filter(|id| self.unsaved(id)).collect()
    }

    /// Gives a design a new name, which is also the folder inside its file.
    /// Editors that have it open fall out of step and load it again.
    pub fn rename(&self, store: &Store, catalog: &Catalog, id: &str, name: &str) -> Result<(), StepError> {
        self.with(store, id, |design| {
            design.blueprint.rename(name);
            design.rev += 1;
            let bytes = design.blueprint.to_sment().context("the design could not be written as a blueprint").map_err(StepError::Broken)?;
            let summary = Summary::of(id, &design.blueprint, catalog, design.created);
            store.replace(summary, &bytes).map_err(StepError::Broken)?;
            design.saved_rev = design.rev;
            Ok(())
        })
    }

    /// Runs `swap`, which stores a new file for the design, with the design
    /// closed: edits not yet on disk are written first, so `swap` sees them,
    /// and every later request loads what `swap` stored. Editors that have the
    /// design open fall out of step and load it again.
    pub fn replacing<T>(&self, store: &Store, catalog: &Catalog, id: &str, swap: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
        loop {
            let slot = self.open.lock().unwrap().entry(id.to_string()).or_default().clone();
            let mut guard = slot.lock().unwrap();
            match &mut *guard {
                Slot::Gone => continue,
                Slot::Open(design) if design.rev != design.saved_rev => {
                    save(store, catalog, id, design)?;
                }
                _ => {}
            }
            let result = swap();
            // The file is new: no page's revision fits it any more.
            self.idle_revs.lock().unwrap().remove(id);
            self.open.lock().unwrap().remove(id);
            *guard = Slot::Gone;
            return result;
        }
    }

    /// Writes designs with settled edits to disk and forgets idle ones. With
    /// `everything`, writes every unsaved design at once (used at shutdown).
    pub fn flush(&self, store: &Store, catalog: &Catalog, everything: bool) {
        self.flush_forgetting_after(store, catalog, everything, FORGET_AFTER);
    }

    /// [`Sessions::flush`], dropping designs idle for `forget_after`.
    pub(crate) fn flush_forgetting_after(&self, store: &Store, catalog: &Catalog, everything: bool, forget_after: Duration) {
        let slots: Vec<(String, SharedSlot)> =
            self.open.lock().unwrap().iter().map(|(id, s)| (id.clone(), s.clone())).collect();
        for (id, slot) in slots {
            // A design that is busy right now gets its turn on the next round.
            let Ok(mut guard) = (if everything { slot.lock().map_err(|_| ()) } else { slot.try_lock().map_err(|_| ()) }) else {
                continue;
            };
            let Slot::Open(design) = &mut *guard else { continue };
            let dirty = design.rev != design.saved_rev;
            if dirty && (everything || design.last_edit.elapsed() >= SAVE_AFTER_QUIET) {
                if let Err(e) = save(store, catalog, &id, design) {
                    tracing::error!("saving design {id}: {e:#}");
                }
            } else if !dirty && design.last_use.elapsed() >= forget_after {
                self.idle_revs.lock().unwrap().insert(id.clone(), design.rev);
                self.open.lock().unwrap().remove(&id);
                *guard = Slot::Gone;
            }
        }
    }
}

fn save(store: &Store, catalog: &Catalog, id: &str, design: &mut OpenDesign) -> anyhow::Result<Vec<u8>> {
    let bytes = design.blueprint.to_sment().context("the design could not be written as a blueprint")?;
    let summary = Summary::of(id, &design.blueprint, catalog, design.created);
    store.save(summary, &bytes)?;
    design.saved_rev = design.rev;
    tracing::info!("saved design {id} at revision {}", design.rev);
    Ok(bytes)
}
