//! Keeping designs in step with a game server's blueprint catalog.
//!
//! The game's side is spoken for by an agent: something that can list the
//! game's blueprints, hand one over and take one in. It calls in every few
//! seconds with the list ([`Sync::report`]) and is told what to upload and what
//! to download. Everything that has to be remembered is remembered here, so an
//! agent keeps no state and can be restarted or swapped at any time.
//!
//! Each side's copy is known by a fingerprint of its files ([`print`]). A link
//! holds the pair of fingerprints that last belonged together: the game's copy
//! and the copy here. A side whose copy no longer has its half of the pair has
//! changed. The two halves need not be equal: a blueprint in an old format is
//! converted when it arrives, and that must not look like an edit to send back.

use crate::app::AppState;
use crate::history::History;
use crate::store::Store;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use yard_core::api::{GameBlueprint, GameReport, Summary, SyncDone, SyncFetch, SyncInfo, SyncPlan, SyncStatus};
use yard_core::api::{SyncDirection, SyncVersion};
use yard_core::blueprint::{unzip, FileTree, MAX_NAME};
use yard_core::Blueprint;

mod yards;
pub use yards::LinkError;
use yards::YardLink;

/// A design goes to the game once it has been left alone this many seconds, so
/// a ship that is being worked on is not imported into the game at every pause.
pub const QUIET_SECS: u64 = 20;

/// A design the player asked to send now stays first in line this long, in
/// case the game is slow to call in.
const RUSH_FOR: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// The fingerprint of a blueprint's files: SHA-256 over every file in the
/// blueprint's folder, in byte order of the paths relative to that folder, each
/// file as its path, a zero byte, its length as eight big-endian bytes, and its
/// contents. The folder's own name is not part of it, so a renamed copy has
/// the same fingerprint. None if the files are not a blueprint.
pub fn print(files: &FileTree) -> Option<String> {
    let header = files.keys().filter(|p| p.ends_with("header.smbph")).min_by_key(|p| p.matches('/').count())?;
    let folder = header.strip_suffix("header.smbph")?;
    let mut hash = Sha256::new();
    for (path, data) in files.range(folder.to_string()..) {
        // Paths inside the folder sit together in the tree's order.
        let Some(relative) = path.strip_prefix(folder) else { break };
        hash.update(relative.as_bytes());
        hash.update([0]);
        hash.update((data.len() as u64).to_be_bytes());
        hash.update(data);
    }
    Some(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn print_of_sment(sment: &[u8]) -> Option<String> {
    print(&unzip(sment).ok()?)
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The game's catalog does not tell names apart by case, and neither do the
/// file systems most servers keep their blueprint folders on.
fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// `base`, or `base 2`, `base 3`... : the first that `taken` does not claim.
pub fn free_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let fits = |suffix: &str| {
        let room = MAX_NAME.saturating_sub(suffix.len());
        let cut: String = base.chars().take(room).collect();
        format!("{}{suffix}", cut.trim_end())
    };
    let plain = fits("");
    if !taken(&plain) {
        return plain;
    }
    (2..).map(|n| fits(&format!(" {n}"))).find(|name| !taken(name)).expect("some number is free")
}

/// The fingerprints of a design and of the game blueprint it belongs with,
/// as they were when the two last matched up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Link {
    /// The blueprint's name in the game.
    name: String,
    game: String,
    design: String,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    /// Whether a game server has ever reported in.
    #[serde(default)]
    met: bool,
    /// By design id.
    #[serde(default)]
    links: BTreeMap<String, Link>,
    /// Designs kept out of the exchange.
    #[serde(default)]
    off: BTreeSet<String>,
    /// Designs whose blueprint has disappeared from the game. They keep their
    /// link, and with it their claim on the name: if the blueprint shows up
    /// again (a report from a game whose folder was briefly unreadable must
    /// not undo every link), the two carry on as before.
    #[serde(default)]
    lost: BTreeSet<String>,
    /// Game blueprints that could not be read here, by lower-case name, with
    /// the fingerprint that was refused. They are not asked for again until
    /// the game has a different version.
    #[serde(default)]
    refused: BTreeMap<String, String>,
    /// Links to designs held in shipyards in the game, by design id.
    #[serde(default)]
    yards: BTreeMap<String, YardLink>,
}

impl State {
    /// A design is deleted: no link, no claim on a name, no switch. The game's
    /// version it was last in step with is marked as one not to take in again.
    fn forget(&mut self, id: &str) {
        self.unlink(id);
        self.off.remove(id);
        self.yards.remove(id);
    }

    /// A design lets go of the game's blueprint it was linked to, which stays
    /// in the game; that version is not taken in again. Says whether there
    /// was a link.
    fn unlink(&mut self, id: &str) -> bool {
        let link = self.links.remove(id);
        let was_lost = self.lost.remove(id);
        let linked = link.is_some();
        if let (Some(link), false) = (link, was_lost) {
            self.refused.insert(link.name.to_lowercase(), link.game);
        }
        linked
    }
}

/// What [`plan`] needs to know about a design.
struct DesignView {
    id: String,
    name: String,
    print: String,
    /// Left alone long enough to go to the game.
    quiet: bool,
}

#[derive(Debug, PartialEq)]
enum Decision {
    /// Ask the game for this blueprint: it is new there, or changed there.
    Send { name: String },
    /// Give this design to the game.
    Fetch { design: String, name: String, print: String },
    /// Both sides already hold the same thing; only the record is missing.
    Link { design: String, name: String, game: String, print: String },
    /// The game's copy was deleted: the design stays, and stops being exchanged.
    Lost { design: String },
    /// A blueprint that had disappeared from the game is back.
    Found { design: String },
    /// The name of a blueprint that was deleted in the game is in use there
    /// again, for something else. The design keeps what it has and leaves the
    /// exchange; the newcomer becomes a design of its own.
    Unrelated { design: String },
    /// A design that was never in the game has the name of something that is.
    Rename { design: String },
}

/// Compares what the game has with what is here.
fn plan(game: &[GameBlueprint], designs: &[DesignView], state: &State) -> Vec<Decision> {
    let in_game = |name: &str| game.iter().find(|g| same_name(&g.name, name));
    // Names that already belong to a design, switched off or not. A link
    // whose design is gone claims nothing: the blueprint is free to come in.
    let mut claimed: Vec<String> = state
        .links
        .iter()
        .filter(|(id, _)| designs.iter().any(|d| d.id == **id))
        .map(|(_, link)| link.name.to_lowercase())
        .collect();
    let mut out = Vec::new();
    for d in designs.iter().filter(|d| !state.off.contains(&d.id)) {
        let fetch = |name: &str| Decision::Fetch { design: d.id.clone(), name: name.to_string(), print: d.print.clone() };
        let Some(link) = state.links.get(&d.id) else {
            // Never been in the game.
            let name_taken = claimed.contains(&d.name.to_lowercase());
            match in_game(&d.name) {
                Some(theirs) if theirs.busy => {}
                // The same ship under the same name, spelled the same: the
                // game finds docked entities by that spelling.
                Some(theirs) if !name_taken && theirs.print == d.print && theirs.name == d.name => {
                    claimed.push(d.name.to_lowercase());
                    out.push(Decision::Link { design: d.id.clone(), name: d.name.clone(), game: theirs.print.clone(), print: d.print.clone() });
                }
                None if !name_taken => {
                    if d.quiet {
                        claimed.push(d.name.to_lowercase());
                        out.push(fetch(&d.name));
                    }
                }
                _ => out.push(Decision::Rename { design: d.id.clone() }),
            }
            continue;
        };
        let Some(theirs) = in_game(&link.name) else {
            if !state.lost.contains(&d.id) {
                out.push(Decision::Lost { design: d.id.clone() });
            }
            continue;
        };
        if theirs.busy {
            continue;
        }
        if state.lost.contains(&d.id) {
            if theirs.print != link.game {
                // Not the blueprint this design belonged with. Taking it in
                // here would overwrite the only copy of the deleted one.
                claimed.retain(|name| *name != link.name.to_lowercase());
                out.push(Decision::Unrelated { design: d.id.clone() });
                continue;
            }
            out.push(Decision::Found { design: d.id.clone() });
        }
        match (d.print != link.design, theirs.print != link.game) {
            (false, false) => {}
            (true, false) => {
                if d.quiet {
                    out.push(fetch(&link.name));
                }
            }
            // Changed on both sides to the same thing: nothing to move.
            (true, true) if d.print == theirs.print => out.push(Decision::Link {
                design: d.id.clone(),
                name: link.name.clone(),
                game: theirs.print.clone(),
                print: d.print.clone(),
            }),
            // Changed in the game. If it was edited here as well, the edits
            // are set aside as a copy when the game's version arrives.
            (_, true) => {
                if state.refused.get(&link.name.to_lowercase()) != Some(&theirs.print) {
                    out.push(Decision::Send { name: theirs.name.clone() });
                }
            }
        }
    }
    for g in game {
        let key = g.name.to_lowercase();
        if !g.busy && !claimed.contains(&key) && state.refused.get(&key) != Some(&g.print) {
            out.push(Decision::Send { name: g.name.clone() });
        }
    }
    out
}

/// The sync history is a record on the side: when it cannot be written, the
/// exchange itself carries on.
fn kept(design: &str, result: anyhow::Result<Option<SyncVersion>>) {
    match result {
        Ok(Some(version)) => tracing::info!("kept version {} of design {design} in its sync history", version.seq),
        Ok(None) => {}
        Err(e) => tracing::error!("sync history of design {design}: {e:#}"),
    }
}

/// Why an agent's request was turned down.
pub enum SyncError {
    /// The upload is not a blueprint this server can read.
    Unreadable(String),
    /// The upload does not have the fingerprint it was announced with: the
    /// blueprint changed while it was being packed. The agent tries again.
    Mismatch,
    /// The design that belongs to this blueprint has its syncing switched off.
    SwitchedOff,
    NoSuchDesign,
    Broken(anyhow::Error),
}

pub struct Sync {
    file: PathBuf,
    /// Seconds a design has to be left alone before it goes to the game.
    quiet_secs: u64,
    state: Mutex<State>,
    /// The last report: when, from whom, how many blueprints.
    seen: Mutex<Option<(Instant, String, usize)>>,
    /// Designs the player asked to send now, without waiting to be left
    /// alone, and since when. Not kept across restarts.
    rushed: Mutex<BTreeMap<String, Instant>>,
    /// The shipyards in the last report; None if the agent did not report any.
    yards_seen: Mutex<Option<Vec<yard_core::api::GameYard>>>,
}

impl Sync {
    pub fn open(data_dir: &Path, quiet_secs: u64) -> anyhow::Result<Self> {
        let file = data_dir.join("sync.json");
        let state = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("cannot read {}", file.display()))?,
            Err(_) => State::default(),
        };
        Ok(Self { file, quiet_secs, state: Mutex::new(state), seen: Mutex::new(None), rushed: Mutex::new(BTreeMap::new()), yards_seen: Mutex::new(None) })
    }

    /// Gives every design that is in step with the game, and has no sync
    /// history yet, its current file as the first version: designs that were
    /// synced before the history existed. Cheap enough to run at every start.
    pub fn seed_history(&self, store: &Store, history: &History) {
        let state = self.state.lock().unwrap();
        let mut seeded = 0;
        for (id, link) in &state.links {
            let Some(summary) = store.summary(id).filter(|s| s.print == link.design) else { continue };
            if !history.list(id).is_empty() {
                continue;
            }
            match history.record(store, &summary, SyncDirection::Matched, summary.modified, None) {
                Ok(Some(_)) => seeded += 1,
                Ok(None) => {}
                Err(e) => tracing::error!("sync history of design {id}: {e:#}"),
            }
        }
        if seeded > 0 {
            tracing::info!("started the sync history of {seeded} designs with the version the game has");
        }
    }

    fn persist(&self, state: &State) {
        let written = serde_json::to_vec_pretty(state).map_err(anyhow::Error::from).and_then(|json| {
            // Whole file or nothing: a torn record would make every design look new.
            let fresh = self.file.with_extension("json.new");
            std::fs::write(&fresh, json)?;
            Ok(std::fs::rename(&fresh, &self.file)?)
        });
        if let Err(e) = written {
            tracing::error!("saving {}: {e:#}", self.file.display());
        }
    }

    /// Takes a game server's list of blueprints and says what should move.
    /// Blocking: call from `spawn_blocking`.
    pub fn report(&self, app: &AppState, report: &GameReport) -> SyncPlan {
        let now = unix_now();
        let mut state = self.state.lock().unwrap();
        let summaries = app.store.list();
        let rushed = {
            let mut rushed = self.rushed.lock().unwrap();
            rushed.retain(|_, since| since.elapsed() < RUSH_FOR);
            rushed.keys().cloned().collect::<BTreeSet<String>>()
        };
        let designs: Vec<DesignView> = summaries
            .iter()
            .map(|s| DesignView {
                id: s.id.clone(),
                name: s.name.clone(),
                print: s.print.clone(),
                quiet: rushed.contains(&s.id) || now.saturating_sub(s.modified) >= self.quiet_secs,
            })
            .collect();
        let mut changed = !state.met;
        state.met = true;
        let mut out = SyncPlan::default();
        // Names handed out in this round, so two renames do not pick the same one.
        let mut renamed_to: Vec<String> = Vec::new();
        for decision in plan(&report.blueprints, &designs, &state) {
            match decision {
                Decision::Send { name } => out.send.push(name),
                Decision::Fetch { design, name, print } => out.fetch.push(SyncFetch { design, name, print }),
                Decision::Link { design, name, game, print } => {
                    tracing::info!("design {design} and the game's \"{name}\" are the same; linked");
                    if let Some(summary) = summaries.iter().find(|s| s.id == design && s.print == print) {
                        kept(&design, app.history.record(&app.store, summary, SyncDirection::Matched, now, None));
                    }
                    state.links.insert(design, Link { name, game, design: print });
                    changed = true;
                }
                Decision::Lost { design } => {
                    tracing::info!("the game no longer has the blueprint of design {design}; it stays here and is not sent back");
                    state.lost.insert(design);
                    changed = true;
                }
                Decision::Found { design } => {
                    tracing::info!("the game has the blueprint of design {design} again");
                    state.lost.remove(&design);
                    changed = true;
                }
                Decision::Unrelated { design } => {
                    tracing::info!("the game uses the name of design {design} for another blueprint now; the design stays here, unsynced");
                    state.lost.remove(&design);
                    state.links.remove(&design);
                    state.off.insert(design);
                    changed = true;
                }
                Decision::Rename { design } => {
                    let Some(old) = summaries.iter().find(|s| s.id == design) else { continue };
                    let taken = |n: &str| {
                        summaries.iter().any(|s| same_name(&s.name, n))
                            || report.blueprints.iter().any(|g| same_name(&g.name, n))
                            || renamed_to.iter().any(|r| same_name(r, n))
                    };
                    let name = free_name(&old.name, taken);
                    renamed_to.push(name.clone());
                    match app.sessions.rename(&app.store, &app.catalog, &design, &name) {
                        Ok(()) => tracing::info!("design {design} \"{}\" renamed \"{name}\": the game has another blueprint by that name", old.name),
                        Err(_) => tracing::error!("design {design} could not be renamed \"{name}\""),
                    }
                }
            }
        }
        let (yards, yards_changed) = self.plan_yards(&mut state, &designs, &report.blueprints, report.yards.as_deref(), now);
        out.yards = yards;
        if changed || yards_changed {
            self.persist(&state);
        }
        self.note_yards(report.yards.clone());
        *self.seen.lock().unwrap() = Some((Instant::now(), report.server.clone(), report.blueprints.len()));
        out
    }

    /// Takes in a blueprint from the game: a new design, or a new version of
    /// the design linked to that name. Blocking.
    pub fn receive(&self, app: &AppState, name: &str, game_print: &str, body: &[u8]) -> Result<Summary, SyncError> {
        let read = || {
            let files = unzip(body).map_err(|e| e.to_string())?;
            let sent_print = print(&files).ok_or("no header.smbph inside")?;
            let bp = Blueprint::from_files_with(&files, Some(&app.catalog)).map_err(|e| e.to_string())?;
            Ok::<_, String>((sent_print, bp))
        };
        let (sent_print, mut bp) = match read() {
            Ok((sent_print, _)) if sent_print != game_print => return Err(SyncError::Mismatch),
            Ok(read) => read,
            Err(why) => {
                // Asking for it again would get the same file: wait for the
                // game to have another version.
                tracing::warn!("the game's \"{name}\" cannot be read here and is left alone: {why}");
                let mut state = self.state.lock().unwrap();
                state.refused.insert(name.to_lowercase(), game_print.to_string());
                self.persist(&state);
                return Err(SyncError::Unreadable(why));
            }
        };
        // The agent sends each blueprint in a folder of its own name.
        let renamed = bp.name != name;
        if renamed {
            bp.rename(name);
        }
        // Old formats are kept converted, as with an upload from a browser.
        let rewritten = (bp.converted || renamed).then(|| bp.to_sment()).transpose().map_err(|e| SyncError::Broken(e.into()))?;
        let stored: &[u8] = rewritten.as_deref().unwrap_or(body);
        let stored_print = if rewritten.is_some() { print_of_sment(stored).unwrap_or_default() } else { sent_print };

        let mut state = self.state.lock().unwrap();
        let linked = state.links.iter().find(|(_, l)| same_name(&l.name, name)).map(|(id, l)| (id.clone(), l.clone()));
        let existing = linked.filter(|(id, _)| app.store.summary(id).is_some());
        if existing.as_ref().is_some_and(|(id, _)| state.off.contains(id)) {
            // Never asked for; an agent that sends it anyway changes nothing.
            return Err(SyncError::SwitchedOff);
        }
        let id = existing.as_ref().map_or_else(Store::new_id, |(id, _)| id.clone());
        let summary = app
            .sessions
            .replacing(&app.store, &app.catalog, &id, || {
                // Any unsaved edits are on disk by now. If the design was edited
                // here since the two last matched, taking the game's version
                // would lose that work: set it aside first.
                let current = app.store.summary(&id);
                if let (Some((_, link)), Some(current)) = (&existing, &current) {
                    if current.print != link.design && current.print != stored_print {
                        keep_copy(app, current)?;
                    }
                }
                let created = current.map_or_else(unix_now, |s| s.created);
                app.store.replace(Summary::of(&id, &bp, &app.catalog, created), stored)
            })
            .map_err(SyncError::Broken)?;
        tracing::info!("took \"{name}\" from the game as design {id} ({} blocks)", summary.blocks);
        kept(&id, app.history.record(&app.store, &summary, SyncDirection::FromGame, unix_now(), Some(stored)));
        state.refused.remove(&name.to_lowercase());
        state.links.insert(id, Link { name: name.to_string(), game: game_print.to_string(), design: summary.print.clone() });
        self.persist(&state);
        Ok(summary)
    }

    /// A design as the agent should import it, with its fingerprint. Blocking.
    pub fn file(&self, app: &AppState, id: &str) -> Result<(Vec<u8>, String), SyncError> {
        let bytes = app.sessions.stored_file(&app.store, &app.catalog, id).map_err(|_| SyncError::NoSuchDesign)?;
        let print = print_of_sment(&bytes).ok_or_else(|| SyncError::Broken(anyhow::anyhow!("design {id} is not a blueprint")))?;
        // Set these very bytes aside: once the game confirms it has them,
        // they are a version in the design's sync history, even if the design
        // has been edited again by then.
        let summary = app.store.summary(id).filter(|s| s.print == print).or_else(|| {
            // Saved again in this instant; describe what is being handed over.
            let created = app.store.summary(id).map_or(0, |s| s.created);
            let mut summary = Summary::of(id, &Blueprint::from_sment(&bytes).ok()?, &app.catalog, created);
            summary.print = print.clone();
            Some(summary)
        });
        if let Some(summary) = summary {
            if let Err(e) = app.history.hold(&app.store, &summary, &bytes) {
                tracing::error!("sync history of design {id}: {e:#}");
            }
        }
        Ok((bytes, print))
    }

    /// Records that the game now has a design, as its agent confirms it.
    pub fn done(&self, app: &AppState, done: &SyncDone) -> Result<(), SyncError> {
        if app.store.summary(&done.design).is_none() {
            return Err(SyncError::NoSuchDesign);
        }
        let mut state = self.state.lock().unwrap();
        // Renamed while the game was taking it in: that copy is the old name's
        // and is left in the game; the design goes there again under its own.
        if app.store.summary(&done.design).is_some_and(|s| s.name != done.name) {
            tracing::info!("the game took design {} in as \"{}\", a name it no longer has", done.design, done.name);
            state.refused.insert(done.name.to_lowercase(), done.game_print.clone());
            self.persist(&state);
            return Ok(());
        }
        tracing::info!("the game has design {} as \"{}\"", done.design, done.name);
        self.rushed.lock().unwrap().remove(&done.design);
        kept(&done.design, app.history.confirm(&app.store, &done.design, &done.design_print, unix_now()));
        state.links.insert(
            done.design.clone(),
            Link { name: done.name.clone(), game: done.game_print.clone(), design: done.design_print.clone() },
        );
        self.persist(&state);
        Ok(())
    }

    /// A design is being deleted: it leaves the exchange for good. If the game
    /// has its blueprint, that version is not brought back in (the game's copy
    /// is never deleted from here); a different version of it later is.
    pub fn forget(&self, id: &str) {
        let mut state = self.state.lock().unwrap();
        state.forget(id);
        self.persist(&state);
    }

    /// Whether the game has a blueprint this design is linked to.
    pub fn in_game(&self, id: &str) -> bool {
        let state = self.state.lock().unwrap();
        state.links.contains_key(id) && !state.lost.contains(id)
    }

    /// A design was given a new name. The game cannot rename a blueprint, so
    /// one it had under the old name stays there as it is, no longer this
    /// design's and not taken in again; the design goes to the game afresh
    /// under its new name. Says whether it had been in the game.
    pub fn renamed(&self, id: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let linked = state.unlink(id);
        if linked {
            self.persist(&state);
        }
        linked
    }

    /// Sends a design to the game at the game's next call, without waiting
    /// for it to be left alone. Its edits must be on disk already. Says
    /// whether there is anything to send.
    pub fn rush(&self, summary: &Summary) -> bool {
        let state = self.state.lock().unwrap();
        let in_step = state.links.get(&summary.id).is_some_and(|l| l.design == summary.print);
        if state.off.contains(&summary.id) || state.lost.contains(&summary.id) || in_step {
            return false;
        }
        self.rushed.lock().unwrap().insert(summary.id.clone(), Instant::now());
        true
    }

    /// Takes a design out of the exchange or puts it back. Switching on a
    /// design whose blueprint was deleted in the game sends it there afresh.
    pub fn switch(&self, id: &str, on: bool) {
        let mut state = self.state.lock().unwrap();
        let changed = if on {
            let was_lost = state.lost.remove(id);
            if was_lost {
                state.links.remove(id);
            }
            state.off.remove(id) | was_lost
        } else {
            state.off.insert(id.to_string())
        };
        if changed {
            self.persist(&state);
        }
    }

    /// Fills in where each design stands. Leaves them blank until a game
    /// server has connected at least once.
    pub fn mark(&self, summaries: &mut [Summary]) {
        let state = self.state.lock().unwrap();
        if !state.met {
            return;
        }
        for s in summaries {
            s.sync = Some(if state.off.contains(&s.id) {
                SyncStatus::Off
            } else if state.lost.contains(&s.id) {
                SyncStatus::Gone
            } else if state.links.get(&s.id).is_some_and(|l| l.design == s.print) {
                SyncStatus::Synced
            } else {
                SyncStatus::Waiting
            });
        }
    }

    pub fn info(&self) -> SyncInfo {
        let known = self.state.lock().unwrap().met;
        match &*self.seen.lock().unwrap() {
            Some((at, server, blueprints)) => {
                SyncInfo { known, server: Some(server.clone()), seconds_ago: Some(at.elapsed().as_secs()), blueprints: *blueprints }
            }
            None => SyncInfo { known, ..SyncInfo::default() },
        }
    }
}

/// Saves a design's current file as a new design under a new name.
fn keep_copy(app: &AppState, current: &Summary) -> anyhow::Result<()> {
    let bytes = app.store.file(&current.id).context("the design file is missing")?;
    let mut copy = Blueprint::from_sment(&bytes).context("the stored design cannot be read")?;
    let name = free_name(&format!("{} shipyard copy", current.name), |n| app.store.list().iter().any(|s| same_name(&s.name, n)));
    copy.rename(&name);
    let id = Store::new_id();
    app.store.save(Summary::of(&id, &copy, &app.catalog, unix_now()), &copy.to_sment()?)?;
    tracing::info!("design {} was edited here and changed in the game; the edits are kept as {id} \"{name}\"", current.id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(list: &[(&str, &str)]) -> Vec<GameBlueprint> {
        list.iter().map(|(name, print)| GameBlueprint { name: name.to_string(), print: print.to_string(), busy: false }).collect()
    }

    fn design(id: &str, name: &str, print: &str) -> DesignView {
        DesignView { id: id.into(), name: name.into(), print: print.into(), quiet: true }
    }

    fn linked(pairs: &[(&str, &str, &str, &str)]) -> State {
        let mut state = State::default();
        for (id, name, game, design) in pairs {
            state.links.insert(id.to_string(), Link { name: name.to_string(), game: game.to_string(), design: design.to_string() });
        }
        state
    }

    fn send(name: &str) -> Decision {
        Decision::Send { name: name.into() }
    }

    /// A fetch of one of the test designs, whose prints are spelled out in each test.
    fn fetch(design: &str, name: &str, print: &str) -> Decision {
        Decision::Fetch { design: design.into(), name: name.into(), print: print.into() }
    }

    #[test]
    fn the_fingerprint_covers_names_and_contents_but_not_the_folder_name() {
        let tree = |folder: &str, logic: &[u8]| -> FileTree {
            [("header.smbph", b"h".to_vec()), ("logic.smbpl", logic.to_vec()), ("DATA/a.smd3", b"blocks".to_vec())]
                .into_iter()
                .map(|(path, data)| (format!("{folder}{path}"), data))
                .collect()
        };
        let a = print(&tree("Scout/", b"l")).unwrap();
        assert_eq!(a.len(), 64);
        assert_eq!(a, print(&tree("Renamed/", b"l")).unwrap());
        assert_eq!(a, print(&tree("", b"l")).unwrap());
        assert_ne!(a, print(&tree("Scout/", b"L")).unwrap());
        // Files beside the blueprint's folder are not part of it.
        let mut extra = tree("Scout/", b"l");
        extra.insert("Zeta.txt".into(), b"x".to_vec());
        extra.insert("Alpha.txt".into(), b"x".to_vec());
        assert_eq!(a, print(&extra).unwrap());
        // Moving bytes from one file to the next changes it.
        let moved: FileTree = [("header.smbph", b"hl".to_vec()), ("logic.smbpl", b"".to_vec()), ("DATA/a.smd3", b"blocks".to_vec())]
            .into_iter()
            .map(|(p, d)| (p.to_string(), d))
            .collect();
        assert_ne!(a, print(&moved).unwrap());
        assert_eq!(print(&FileTree::new()), None);
        // Fixed value, so the agent's implementation can be checked against it.
        let fixed: FileTree = [("X/header.smbph", vec![1, 2, 3]), ("X/DATA/r.smd3", vec![]), ("X/meta.smbpm", vec![0, 0, 0, 5, 1])]
            .into_iter()
            .map(|(p, d)| (p.to_string(), d))
            .collect();
        assert_eq!(print(&fixed).unwrap(), "7776a573a470708d2175ed74d051ad3aed710301d9bd9ea032ba566af8f290f6");
    }

    #[test]
    fn whichever_side_changed_gives_its_version_to_the_other() {
        let state = linked(&[("d1", "Scout", "g1", "p1"), ("d2", "Hauler", "g2", "p2"), ("d3", "Tug", "g3", "p3")]);
        let designs = [design("d1", "Scout", "p1"), design("d2", "Hauler", "p2-edited"), design("d3", "Tug", "p3")];
        // Scout untouched, Hauler edited here, Tug saved again in the game.
        let decisions = plan(&game(&[("Scout", "g1"), ("Hauler", "g2"), ("Tug", "g3-new")]), &designs, &state);
        assert_eq!(decisions, vec![fetch("d2", "Hauler", "p2-edited"), send("Tug")]);
    }

    #[test]
    fn new_things_on_either_side_cross_over() {
        let designs = [design("d1", "Scout", "p1")];
        let decisions = plan(&game(&[("Station", "g9")]), &designs, &State::default());
        assert_eq!(decisions, vec![fetch("d1", "Scout", "p1"), send("Station")]);
    }

    #[test]
    fn a_design_still_being_edited_waits() {
        let mut busy = design("d1", "Scout", "p1");
        busy.quiet = false;
        assert_eq!(plan(&[], &[busy], &State::default()), vec![]);
        let mut busy = design("d1", "Scout", "p1-edited");
        busy.quiet = false;
        let state = linked(&[("d1", "Scout", "g1", "p1")]);
        assert_eq!(plan(&game(&[("Scout", "g1")]), &[busy], &state), vec![]);
    }

    #[test]
    fn a_deleted_design_is_not_brought_back_from_the_game_until_the_game_changes_it() {
        let mut state = linked(&[("d1", "Scout", "g1", "p1"), ("d2", "Tug", "g2", "p2")]);
        // While the design exists the game's copy is its own.
        assert_eq!(plan(&game(&[("Scout", "g1"), ("Tug", "g2")]), &[design("d1", "Scout", "p1"), design("d2", "Tug", "p2")], &state), vec![]);
        state.forget("d1");
        assert!(!state.links.contains_key("d1") && state.links.contains_key("d2"));
        let both = game(&[("Scout", "g1"), ("Tug", "g2")]);
        // The game still has Scout, unchanged: it stays out.
        assert_eq!(plan(&both, &[design("d2", "Tug", "p2")], &state), vec![]);
        // Changed in the game, it comes in again, as a design of its own.
        assert_eq!(plan(&game(&[("Scout", "g1-changed"), ("Tug", "g2")]), &[design("d2", "Tug", "p2")], &state), vec![send("Scout")]);
        // Its name is free for a new design, even in other letters.
        assert_eq!(plan(&[], &[design("d3", "scout", "p3")], &state), vec![fetch("d3", "scout", "p3")]);
        // A design that was never in the game, switched off, or lost leaves nothing behind.
        let mut state = State::default();
        state.off.insert("d1".into());
        state.forget("d1");
        assert!(state.off.is_empty() && state.refused.is_empty());
        let mut state = linked(&[("d1", "Scout", "g1", "p1")]);
        state.lost.insert("d1".into());
        state.forget("d1");
        assert!(state.lost.is_empty() && state.links.is_empty() && state.refused.is_empty());
    }

    #[test]
    fn a_blueprint_deleted_in_the_game_is_not_sent_back() {
        let mut state = linked(&[("d1", "Scout", "g1", "p1")]);
        let decisions = plan(&[], &[design("d1", "Scout", "p1")], &state);
        assert_eq!(decisions, vec![Decision::Lost { design: "d1".into() }]);
        // Once that is noted, nothing more happens while it stays away, even
        // if the design is edited: the game does not get it back.
        state.lost.insert("d1".into());
        assert_eq!(plan(&[], &[design("d1", "Scout", "p1-edited")], &state), vec![]);
        // Its name stays taken, so a new design of that name steps aside.
        let both = [design("d1", "Scout", "p1"), design("d2", "Scout", "p2")];
        assert_eq!(plan(&[], &both, &state), vec![Decision::Rename { design: "d2".into() }]);
        // If the blueprint is there again (the game's folder was unreadable
        // for a moment, say), the two carry on where they were.
        let back = plan(&game(&[("Scout", "g1")]), &[design("d1", "Scout", "p1-edited")], &state);
        assert_eq!(back, vec![Decision::Found { design: "d1".into() }, fetch("d1", "Scout", "p1-edited")]);
        // If the name comes back with something else behind it, a player has
        // saved a new ship under the old name. The design here is the only
        // copy of the deleted one: it is left as it is, out of the exchange,
        // and the new ship comes in as a design of its own.
        let other = plan(&game(&[("Scout", "g-other")]), &[design("d1", "Scout", "p1")], &state);
        assert_eq!(other, vec![Decision::Unrelated { design: "d1".into() }, send("Scout")]);
        // A design switched off by hand is left alone entirely.
        let mut state = State::default();
        state.off.insert("d1".into());
        assert_eq!(plan(&[], &[design("d1", "Scout", "p1")], &state), vec![]);
    }

    #[test]
    fn edits_on_both_sides_take_the_games_version() {
        let state = linked(&[("d1", "Scout", "g1", "p1")]);
        let decisions = plan(&game(&[("Scout", "g2")]), &[design("d1", "Scout", "p1-edited")], &state);
        assert_eq!(decisions, vec![send("Scout")]);
        // Unless both ended up with the same files.
        let decisions = plan(&game(&[("Scout", "same")]), &[design("d1", "Scout", "same")], &state);
        assert_eq!(decisions, vec![Decision::Link { design: "d1".into(), name: "Scout".into(), game: "same".into(), print: "same".into() }]);
    }

    #[test]
    fn names_are_matched_without_regard_to_case_and_clashes_are_renamed() {
        // The same ship uploaded here and present in the game: just link them.
        let decisions = plan(&game(&[("Scout", "p1")]), &[design("d1", "Scout", "p1")], &State::default());
        assert_eq!(decisions, vec![Decision::Link { design: "d1".into(), name: "Scout".into(), game: "p1".into(), print: "p1".into() }]);
        // Spelled differently it is a clash all the same: the game tells the
        // two names apart in some places and not in others.
        let decisions = plan(&game(&[("scout", "p1")]), &[design("d1", "Scout", "p1")], &State::default());
        assert_eq!(decisions, vec![Decision::Rename { design: "d1".into() }, send("scout")]);
        // A different ship with a name the game already uses: it gets a new
        // name, and the game's blueprint comes over as its own design.
        let decisions = plan(&game(&[("Scout", "other")]), &[design("d1", "Scout", "p1")], &State::default());
        assert_eq!(decisions, vec![Decision::Rename { design: "d1".into() }, send("Scout")]);
        // Two new designs with one name: the first goes, the second is renamed.
        let decisions = plan(&[], &[design("d1", "Scout", "p1"), design("d2", "SCOUT", "p2")], &State::default());
        assert_eq!(decisions, vec![fetch("d1", "Scout", "p1"), Decision::Rename { design: "d2".into() }]);
        // A switched-off design keeps its claim on its blueprint.
        let mut state = linked(&[("d1", "Scout", "g1", "p1")]);
        state.off.insert("d1".into());
        assert_eq!(plan(&game(&[("Scout", "g2")]), &[design("d1", "Scout", "p1")], &state), vec![]);
    }

    #[test]
    fn a_link_to_a_design_that_no_longer_exists_does_not_hold_the_blueprint_back() {
        let state = linked(&[("gone", "Scout", "g1", "p1")]);
        assert_eq!(plan(&game(&[("Scout", "g1")]), &[], &state), vec![send("Scout")]);
    }

    #[test]
    fn a_blueprint_the_game_is_saving_is_left_alone_and_not_taken_for_deleted() {
        let busy = vec![GameBlueprint { name: "Scout".into(), print: String::new(), busy: true }];
        // Linked and edited here, linked and untouched, or new here with the
        // same name: nothing is decided until the game has finished.
        let state = linked(&[("d1", "Scout", "g1", "p1")]);
        assert_eq!(plan(&busy, &[design("d1", "Scout", "p1-edited")], &state), vec![]);
        assert_eq!(plan(&busy, &[design("d1", "Scout", "p1")], &state), vec![]);
        assert_eq!(plan(&busy, &[design("d9", "Scout", "p9")], &State::default()), vec![]);
        assert_eq!(plan(&busy, &[], &State::default()), vec![]);
    }

    #[test]
    fn a_blueprint_that_could_not_be_read_is_not_asked_for_again_until_it_changes() {
        let mut state = State::default();
        state.refused.insert("wreck".into(), "g1".into());
        assert_eq!(plan(&game(&[("Wreck", "g1")]), &[], &state), vec![]);
        assert_eq!(plan(&game(&[("Wreck", "g2")]), &[], &state), vec![send("Wreck")]);
        // The same for a new version of a blueprint that is already a design.
        let mut state = linked(&[("d1", "Scout", "g1", "p1")]);
        state.refused.insert("scout".into(), "g2".into());
        assert_eq!(plan(&game(&[("Scout", "g2")]), &[design("d1", "Scout", "p1")], &state), vec![]);
        assert_eq!(plan(&game(&[("Scout", "g3")]), &[design("d1", "Scout", "p1")], &state), vec![send("Scout")]);
    }

    #[test]
    fn free_names_count_up_and_stay_within_the_length_limit() {
        assert_eq!(free_name("Scout", |_| false), "Scout");
        assert_eq!(free_name("Scout", |n| n == "Scout" || n == "Scout 2"), "Scout 3");
        let long = "x".repeat(80);
        let name = free_name(&long, |n| n.len() == MAX_NAME && !n.ends_with(" 2"));
        assert_eq!(name.len(), MAX_NAME);
        assert!(name.ends_with(" 2"));
    }
}
