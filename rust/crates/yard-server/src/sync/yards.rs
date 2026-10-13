//! Designs linked to designs held in shipyards in the game.
//!
//! A design in a shipyard is an entity of its own, not a blueprint, so the
//! game's catalog is the way in: a design here goes to the catalog as usual
//! ([`super`]), and the agent makes the shipyard design from that blueprint,
//! in place of the one it had. The way out is direct: the agent copies the
//! loaded shipyard design as a blueprint and uploads it here.
//!
//! The game only works on shipyards of stations it has loaded, and only a
//! loaded design can be copied or replaced, so a link moves things while the
//! design is loaded in its shipyard and waits otherwise.
//!
//! The game writes a time into every part of a blueprint it copies, so two
//! copies of the same design differ as files. Copies are compared by what
//! they hold ([`content_print`]).

use super::{kept, print, same_name, unix_now, DesignView, State, Sync, SyncError};
use crate::app::AppState;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::Instant;
use yard_core::api::{GameBlueprint, GameYard, Summary, SyncDirection, YardGive, YardGiven, YardKeep, YardLinkInfo, YardLinkRequest, YardPlan, YardState, YardTake};
use yard_core::blueprint::{unzip, BlueprintEntity};
use yard_core::dock::Dock;
use yard_core::entity::Entity;
use yard_core::Blueprint;

/// How long a design is taken to be on its way into a shipyard after the
/// agent was asked to put it there, before it is asked again.
const GIVING_SECS: u64 = 180;
/// How long after a failed attempt the agent is asked again.
const RETRY_SECS: u64 = 60;

/// A design's link to a design in a shipyard.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct YardLink {
    /// The shipyard, by [`GameYard::id`].
    pub yard: String,
    pub station: String,
    pub at: [i32; 3],
    /// The shipyard design's unique name. None until one is made.
    pub uid: Option<String>,
    /// The name the shipyard shows for it.
    pub name: Option<String>,
    /// The fingerprint the design here had when the two last matched up.
    pub design: Option<String>,
    /// The fingerprint of the shipyard design's last copy that was seen here,
    /// and what that copy holds.
    pub ship: Option<String>,
    pub ship_content: Option<String>,
    /// The design here is to replace the shipyard's, whatever either holds.
    #[serde(default)]
    pub keep_here: bool,
    /// When the agent was last asked to put the design into the shipyard,
    /// and the fingerprint the design had then.
    pub giving: Option<(u64, String)>,
    /// The last thing that went wrong, from the agent.
    pub trouble: Option<String>,
}

/// What the catalog side knows of a design.
pub(super) struct InCatalog {
    /// Sync with the game is switched off for it, or its blueprint was deleted.
    pub off: bool,
    /// The game's blueprint of it, by name and fingerprint, if the game has
    /// the design as it is now.
    pub current: Option<(String, String)>,
}

#[derive(Debug, PartialEq)]
pub(super) enum Step {
    Same,
    Wait(String),
    Take(YardTake),
    Give(YardGive),
}

/// What a link calls for, given what the game reported.
pub(super) fn decide(id: &str, link: &YardLink, design: &DesignView, catalog: &InCatalog, yards: Option<&[GameYard]>, now: u64) -> Step {
    let wait = |text: &str| Step::Wait(text.to_string());
    if catalog.off {
        return wait("Sync with the game server is off for this design.");
    }
    let Some(yards) = yards else {
        return wait("The game server has not reported its shipyards.");
    };
    let Some(yard) = yards.iter().find(|y| y.id == link.yard) else {
        return wait("The station is not loaded in the game. Changes move while a player is near it.");
    };
    let give = |replace: Option<String>| {
        if let Some((since, _)) = &link.giving {
            if now.saturating_sub(*since) < GIVING_SECS {
                return wait(if link.trouble.is_some() { "Trying again shortly." } else { "Putting the design into the shipyard..." });
            }
        }
        let Some((blueprint, print)) = catalog.current.clone() else {
            return wait("Waiting for this design to reach the game's catalog.");
        };
        if !yard.room {
            return wait("The shipyard needs a free slot in its inventory to take the design.");
        }
        let name = link.name.clone().unwrap_or_else(|| design.name.clone());
        let expect = if link.keep_here { None } else { link.ship.clone() };
        Step::Give(YardGive { design: id.to_string(), yard: yard.id.clone(), replace, name, blueprint, print, expect })
    };
    let Some(uid) = &link.uid else {
        if yard.state != YardState::Idle {
            return wait("Waiting for the shipyard to be free: unload its design to let this one in.");
        }
        return give(None);
    };
    if !yard.designs.iter().any(|d| d.uid == *uid) {
        return wait("The design is no longer in the shipyard. Unlink it, or link it to another.");
    }
    if yard.loaded.as_ref() != Some(uid) || yard.state != YardState::Loaded {
        return wait("Load the design in its shipyard to sync it.");
    }
    let swap = || {
        if yard.in_use {
            return wait("A player is aboard the design in the shipyard. It is updated when they leave it.");
        }
        give(Some(uid.clone()))
    };
    if link.keep_here {
        return swap();
    }
    let Some(seen) = &yard.loaded_print else {
        return wait("Reading the design in the shipyard...");
    };
    if link.ship.as_ref() != Some(seen) {
        return Step::Take(YardTake { design: id.to_string(), uid: uid.clone(), print: seen.clone() });
    }
    if link.design.as_ref() != Some(&design.print) {
        return swap();
    }
    Step::Same
}

/// A fingerprint of what a blueprint holds, unlike [`print`] blind to when
/// the game saved it: the blocks of every entity (type and facing), their
/// links and logic, and for each docked entity the rail and docker it is
/// docked by. Docked entities count by what they hold and where they sit in
/// the tree, not by their folder names: the game numbers them afresh
/// (ATTACHED_0, ATTACHED_1...) in each copy it makes.
pub fn content_print(bp: &Blueprint) -> String {
    fn tree(e: &BlueprintEntity, path: &str, docks: &BTreeMap<String, Dock>) -> [u8; 32] {
        let mut hash = Sha256::new();
        if let Some(dock) = docks.get(path) {
            for p in [dock.rail.pos, dock.docker.pos] {
                for v in [p.x, p.y, p.z] {
                    hash.update(v.to_le_bytes());
                }
            }
        }
        hash.update([0]);
        hash.update(blocks_print(&e.entity));
        let mut docked: Vec<[u8; 32]> = e
            .children
            .iter()
            .map(|c| tree(c, &if path.is_empty() { c.dir.clone() } else { format!("{path}/{}", c.dir) }, docks))
            .collect();
        docked.sort_unstable();
        hash.update([1]);
        for d in docked {
            hash.update(d);
        }
        hash.finalize().into()
    }
    tree(&bp.root, "", &bp.docks()).iter().map(|b| format!("{b:02x}")).collect()
}

/// A fingerprint of one entity's blocks (type and facing) and links.
fn blocks_print(e: &Entity) -> [u8; 32] {
    let mut hash = Sha256::new();
    // A segment counts by its blocks: one emptied by edits may still be kept.
    let mut keys: Vec<_> = e.segments().filter(|(_, s)| s.blocks.iter().any(|b| !b.is_air())).map(|(k, _)| *k).collect();
    keys.sort_unstable();
    for key in keys {
        let segment = e.segment(key).expect("listed above");
        hash.update(key.0.to_le_bytes());
        hash.update(key.1.to_le_bytes());
        hash.update(key.2.to_le_bytes());
        for (i, b) in segment.blocks.iter().enumerate() {
            if !b.is_air() {
                hash.update((i as u16).to_le_bytes());
                hash.update(b.ty().to_le_bytes());
                hash.update([b.orientation()]);
            }
        }
    }
    for (from, kinds) in e.links() {
        for (ty, targets) in kinds {
            for to in targets {
                for v in [from.x, from.y, from.z, *ty as i32, to.x, to.y, to.z] {
                    hash.update(v.to_le_bytes());
                }
            }
        }
    }
    hash.finalize().into()
}

fn content_of_sment(bytes: &[u8]) -> Option<String> {
    Some(content_print(&Blueprint::from_sment(bytes).ok()?))
}

/// Why a link could not be made.
pub enum LinkError {
    NoSuchDesign,
    Refused(String),
}

impl Sync {
    /// What the catalog side knows of a design. With the game's list, the
    /// blueprint must also be there and not in the middle of being written.
    fn in_catalog(state: &State, design: &DesignView, game: Option<&[GameBlueprint]>) -> InCatalog {
        let off = state.off.contains(&design.id) || state.lost.contains(&design.id);
        let current = state
            .links
            .get(&design.id)
            .filter(|l| l.design == design.print)
            .filter(|l| game.is_none_or(|game| game.iter().any(|g| same_name(&g.name, &l.name) && g.print == l.game && !g.busy)))
            .map(|l| (l.name.clone(), l.game.clone()));
        InCatalog { off, current }
    }

    /// The part of a report's answer about shipyards. A design waiting to go
    /// into a shipyard is first sent to the game's catalog without waiting
    /// to be left alone.
    pub(super) fn plan_yards(&self, state: &mut State, designs: &[DesignView], game: &[GameBlueprint], yards: Option<&[GameYard]>, now: u64) -> (YardPlan, bool) {
        let mut out = YardPlan::default();
        let mut changed = false;
        // One order per shipyard at a time.
        let mut busy: Vec<String> = Vec::new();
        for (id, link) in state.yards.clone() {
            let Some(design) = designs.iter().find(|d| d.id == id) else { continue };
            let catalog = Self::in_catalog(state, design, Some(game));
            if let Some(uid) = &link.uid {
                out.watch.push(uid.clone());
            }
            match decide(&id, &link, design, &catalog, yards, now) {
                Step::Take(take) => out.take.push(take),
                Step::Give(give) if !busy.contains(&give.yard) => {
                    busy.push(give.yard.clone());
                    if let Some(l) = state.yards.get_mut(&id) {
                        l.giving = Some((now, design.print.clone()));
                        changed = true;
                    }
                    out.give.push(give);
                }
                Step::Wait(_) if catalog.current.is_none() && !catalog.off && (link.keep_here || link.uid.is_none() || link.design.as_ref() != Some(&design.print)) => {
                    self.rushed.lock().unwrap().entry(id.clone()).or_insert_with(Instant::now);
                }
                _ => {}
            }
        }
        (out, changed)
    }

    /// Where a design stands with its shipyard design, if it is linked to one.
    pub fn yard_link(&self, store: &Store, id: &str) -> Option<YardLinkInfo> {
        let state = self.state.lock().unwrap();
        let link = state.yards.get(id)?;
        let summary = store.summary(id)?;
        let design = DesignView { id: id.to_string(), name: summary.name.clone(), print: summary.print.clone(), quiet: true };
        let yards = self.yards_seen.lock().unwrap().clone();
        let step = decide(id, link, &design, &Self::in_catalog(&state, &design, None), yards.as_deref(), unix_now());
        let (in_step, mut status) = match step {
            Step::Same => (true, "The same in the shipyard.".to_string()),
            Step::Wait(text) => (false, text),
            Step::Take(_) => (false, "Bringing the shipyard's version here...".to_string()),
            Step::Give(_) => (false, "Putting the design into the shipyard...".to_string()),
        };
        if let Some(trouble) = &link.trouble {
            status = format!("{status} Last attempt: {trouble}");
        }
        Some(YardLinkInfo { station: link.station.clone(), at: link.at, name: link.name.clone(), in_step, status })
    }

    /// The shipyards in the game's last report.
    pub fn yards(&self) -> Vec<GameYard> {
        let mut yards = self.yards_seen.lock().unwrap().clone().unwrap_or_default();
        for y in &mut yards {
            y.loaded_print = None;
        }
        yards
    }

    /// Links a design to a design in a shipyard, or to a new one made from it.
    pub fn link_yard(&self, store: &Store, id: &str, request: &YardLinkRequest) -> Result<(), LinkError> {
        let refused = |text: &str| Err(LinkError::Refused(text.to_string()));
        let summary = store.summary(id).ok_or(LinkError::NoSuchDesign)?;
        let mut state = self.state.lock().unwrap();
        if state.off.contains(id) || state.lost.contains(id) {
            return refused("Switch on sync with the game server for this design first.");
        }
        let yards = self.yards_seen.lock().unwrap().clone().unwrap_or_default();
        let Some(yard) = yards.iter().find(|y| y.id == request.yard) else {
            return refused("The game no longer reports that shipyard. Its station has to be loaded in the game.");
        };
        let mut link = YardLink { yard: yard.id.clone(), station: yard.station.clone(), at: yard.at, ..YardLink::default() };
        match &request.uid {
            None => link.name = Some(summary.name.clone()),
            Some(uid) => {
                let Some(held) = yard.designs.iter().find(|d| d.uid == *uid) else {
                    return refused("The shipyard no longer holds that design.");
                };
                let taken = state.yards.iter().any(|(other, l)| other != id && l.yard == yard.id && l.uid.as_ref() == Some(uid));
                if taken {
                    return refused("Another design here is linked to that shipyard design already.");
                }
                link.uid = Some(uid.clone());
                link.name = Some(held.name.clone());
                match request.keep {
                    YardKeep::Here => link.keep_here = true,
                    // Nothing is known of the shipyard's design, so it
                    // counts as changed and comes here. What the design
                    // holds now is kept as a copy.
                    YardKeep::Shipyard => {}
                }
            }
        }
        tracing::info!("design {id} linked to {:?} in the shipyard of \"{}\" at {:?}", link.name, link.station, link.at);
        state.yards.insert(id.to_string(), link);
        self.persist(&state);
        Ok(())
    }

    /// Ends a design's link to a shipyard design. Both keep what they hold.
    pub fn unlink_yard(&self, id: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let was = state.yards.remove(id).is_some();
        if was {
            tracing::info!("design {id} is no longer linked to a shipyard design");
            self.persist(&state);
        }
        was
    }

    /// Takes in a copy of a shipyard design. If it holds the same as the
    /// last copy, only its fingerprint is noted; otherwise it becomes the
    /// design's new version. Edits made here since the two last matched are
    /// kept as a separate design. Blocking.
    pub fn take_yard(&self, app: &AppState, id: &str, uid: &str, sent_print: &str, body: &[u8]) -> Result<Option<Summary>, SyncError> {
        let files = unzip(body).map_err(|e| SyncError::Unreadable(e.to_string()))?;
        if print(&files).as_deref() != Some(sent_print) {
            return Err(SyncError::Mismatch);
        }
        let mut bp = Blueprint::from_files_with(&files, Some(&app.catalog)).map_err(|e| SyncError::Unreadable(e.to_string()))?;
        let content = content_print(&bp);

        let mut state = self.state.lock().unwrap();
        let link = state.yards.get(id).filter(|l| l.uid.as_deref() == Some(uid)).cloned().ok_or(SyncError::NoSuchDesign)?;
        let current = app.store.summary(id).ok_or(SyncError::NoSuchDesign)?;
        if link.ship_content.as_deref() == Some(content.as_str()) {
            let l = state.yards.get_mut(id).expect("found above");
            l.ship = Some(sent_print.to_string());
            l.trouble = None;
            self.persist(&state);
            return Ok(None);
        }
        // Holding the same as the design here already (as when a link is
        // made): nothing to take in, and nothing to set aside.
        let same_here = app.store.file(id).as_deref().and_then(content_of_sment).as_deref() == Some(content.as_str());
        if same_here {
            let l = state.yards.get_mut(id).expect("found above");
            l.design = Some(current.print.clone());
            l.ship = Some(sent_print.to_string());
            l.ship_content = Some(content);
            l.trouble = None;
            self.persist(&state);
            return Ok(None);
        }
        bp.rename(&current.name);
        let stored = bp.to_sment().map_err(|e| SyncError::Broken(e.into()))?;
        let summary = app
            .sessions
            .replacing(&app.store, &app.catalog, id, || {
                let now_here = app.store.summary(id);
                if let Some(now_here) = &now_here {
                    if link.design.as_deref() != Some(now_here.print.as_str()) {
                        super::keep_copy(app, now_here)?;
                    }
                }
                let created = now_here.map_or_else(unix_now, |s| s.created);
                app.store.replace(Summary::of(id, &bp, &app.catalog, created), &stored)
            })
            .map_err(SyncError::Broken)?;
        tracing::info!("took design {uid} from the shipyard of \"{}\" as design {id} ({} blocks)", link.station, summary.blocks);
        kept(id, app.history.record(&app.store, &summary, SyncDirection::FromShipyard, unix_now(), Some(&stored)));
        let l = state.yards.get_mut(id).expect("found above");
        l.design = Some(summary.print.clone());
        l.ship = Some(sent_print.to_string());
        l.ship_content = Some(content);
        l.trouble = None;
        self.persist(&state);
        // On to the game's catalog, where it would otherwise wait to be left alone.
        self.rushed.lock().unwrap().insert(id.to_string(), Instant::now());
        Ok(Some(summary))
    }

    /// Records that a shipyard holds a design made from this design's blueprint.
    pub fn yard_given(&self, app: &AppState, given: &YardGiven) -> Result<(), SyncError> {
        let mut state = self.state.lock().unwrap();
        let catalog_link = state.links.get(&given.design).cloned();
        let link = state.yards.get_mut(&given.design).ok_or(SyncError::NoSuchDesign)?;
        let Some(catalog_link) = catalog_link.filter(|l| l.game == given.print) else {
            tracing::info!("a shipyard design was made from an old blueprint of design {}", given.design);
            link.giving = None;
            link.uid = Some(given.uid.clone());
            link.ship = None;
            link.ship_content = None;
            link.design = None;
            self.persist(&state);
            return Err(SyncError::Mismatch);
        };
        // What the given version holds: from the design itself, or from its
        // sync history if it has been edited since.
        let bytes = app
            .store
            .summary(&given.design)
            .filter(|s| s.print == catalog_link.design)
            .and_then(|_| app.store.file(&given.design))
            .or_else(|| {
                let version = app.history.list(&given.design).into_iter().rev().find(|v| v.print == catalog_link.design)?;
                app.history.file(&given.design, version.seq)
            });
        tracing::info!("the shipyard of \"{}\" holds design {} as {}", link.station, given.design, given.uid);
        link.uid = Some(given.uid.clone());
        link.design = Some(catalog_link.design.clone());
        link.ship = None;
        link.ship_content = bytes.as_deref().and_then(content_of_sment);
        link.keep_here = false;
        link.giving = None;
        link.trouble = None;
        self.persist(&state);
        Ok(())
    }

    /// Notes what went wrong with a shipyard design, for the player to see,
    /// and lets the agent try again at its next report.
    pub fn yard_trouble(&self, id: &str, why: &str) {
        let mut state = self.state.lock().unwrap();
        if let Some(link) = state.yards.get_mut(id) {
            tracing::warn!("shipyard design of design {id}: {why}");
            link.trouble = Some(why.chars().take(300).collect());
            // Asked again in a while, not at the next report.
            let print = link.giving.take().map(|(_, p)| p).unwrap_or_default();
            link.giving = Some((unix_now().saturating_sub(GIVING_SECS - RETRY_SECS), print));
            self.persist(&state);
        }
    }

    /// Moves a link's last request to the agent this many seconds into the past.
    #[cfg(test)]
    pub fn age_yard_attempts(&self, id: &str, secs: u64) {
        let mut state = self.state.lock().unwrap();
        if let Some((since, _)) = state.yards.get_mut(id).and_then(|l| l.giving.as_mut()) {
            *since = since.saturating_sub(secs);
        }
    }

    pub(super) fn note_yards(&self, yards: Option<Vec<GameYard>>) {
        *self.yards_seen.lock().unwrap() = yards;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yard_core::api::YardDesign;
    use yard_core::{Block, Pos};

    fn yard(state: YardState, loaded: Option<&str>, print: Option<&str>) -> GameYard {
        GameYard {
            id: "STATION@1".into(),
            station: "Home".into(),
            at: [0, 0, 1],
            state,
            in_use: false,
            room: true,
            designs: vec![YardDesign { uid: "ENTITY_SHIP_a".into(), name: "Arrow".into() }],
            loaded: loaded.map(str::to_string),
            loaded_print: print.map(str::to_string),
        }
    }

    fn design(print: &str) -> DesignView {
        DesignView { id: "d".into(), name: "Arrow".into(), print: print.into(), quiet: true }
    }

    fn in_catalog(print: Option<&str>) -> InCatalog {
        InCatalog { off: false, current: print.map(|p| ("Arrow".to_string(), p.to_string())) }
    }

    fn linked(design: &str, ship: &str) -> YardLink {
        YardLink {
            yard: "STATION@1".into(),
            uid: Some("ENTITY_SHIP_a".into()),
            name: Some("Arrow".into()),
            design: Some(design.into()),
            ship: Some(ship.into()),
            ..YardLink::default()
        }
    }

    fn kind(step: &Step) -> &str {
        match step {
            Step::Same => "in step",
            Step::Wait(_) => "wait",
            Step::Take(_) => "take",
            Step::Give(_) => "give",
        }
    }

    #[test]
    fn a_link_moves_whichever_side_changed_while_the_design_is_loaded() {
        let loaded = |print| [yard(YardState::Loaded, Some("ENTITY_SHIP_a"), Some(print))];
        let decide = |link: &YardLink, here: &str, yards: &[GameYard]| decide("d", link, &design(here), &in_catalog(Some("g1")), Some(yards), 0);
        assert_eq!(kind(&decide(&linked("p1", "s1"), "p1", &loaded("s1"))), "in step");
        // Changed in the shipyard: comes here, even if it changed here too
        // (the edits here are kept as a copy when it arrives).
        assert_eq!(decide(&linked("p1", "s1"), "p1", &loaded("s2")), Step::Take(YardTake { design: "d".into(), uid: "ENTITY_SHIP_a".into(), print: "s2".into() }));
        assert_eq!(kind(&decide(&linked("p1", "s1"), "p2", &loaded("s2"))), "take");
        // Changed here: replaces the shipyard's design, from the catalog's blueprint.
        let Step::Give(give) = decide(&linked("p1", "s1"), "p2", &loaded("s1")) else { panic!() };
        assert_eq!((give.replace.as_deref(), give.blueprint.as_str(), give.print.as_str()), (Some("ENTITY_SHIP_a"), "Arrow", "g1"));
        // Not while a player is aboard, nor before the catalog has it.
        let mut aboard = loaded("s1");
        aboard[0].in_use = true;
        assert_eq!(kind(&decide(&linked("p1", "s1"), "p2", &aboard)), "wait");
        let behind = super::decide("d", &linked("p1", "s1"), &design("p2"), &in_catalog(None), Some(&loaded("s1")), 0);
        assert_eq!(behind, Step::Wait("Waiting for this design to reach the game's catalog.".into()));
        // Nothing moves while the design is not loaded, or before it was read.
        let unloaded = [yard(YardState::Idle, None, None)];
        assert_eq!(kind(&decide(&linked("p1", "s1"), "p2", &unloaded)), "wait");
        let unread = [yard(YardState::Loaded, Some("ENTITY_SHIP_a"), None)];
        assert_eq!(kind(&decide(&linked("p1", "s1"), "p2", &unread)), "wait");
        // Nor while the station is away, or the design is gone from the shipyard.
        assert_eq!(kind(&decide(&linked("p1", "s1"), "p2", &[])), "wait");
        let mut emptied = loaded("s1");
        emptied[0].designs.clear();
        assert_eq!(kind(&decide(&linked("p1", "s1"), "p2", &emptied)), "wait");
    }

    #[test]
    fn a_new_shipyard_design_is_made_when_the_shipyard_is_free() {
        let link = YardLink { yard: "STATION@1".into(), name: Some("Arrow".into()), ..YardLink::default() };
        let busy = [yard(YardState::Loaded, Some("ENTITY_SHIP_a"), None)];
        assert_eq!(kind(&decide("d", &link, &design("p1"), &in_catalog(Some("g1")), Some(&busy), 0)), "wait");
        let free = [yard(YardState::Idle, None, None)];
        let Step::Give(give) = decide("d", &link, &design("p1"), &in_catalog(Some("g1")), Some(&free), 0) else { panic!() };
        assert_eq!((give.replace, give.name.as_str()), (None, "Arrow"));
        // Asked once; the agent has a while to do it.
        let asked = YardLink { giving: Some((100, "p1".into())), ..link.clone() };
        assert_eq!(kind(&decide("d", &asked, &design("p1"), &in_catalog(Some("g1")), Some(&free), 150)), "wait");
        assert_eq!(kind(&decide("d", &asked, &design("p1"), &in_catalog(Some("g1")), Some(&free), 100 + GIVING_SECS)), "give");
        let mut full = free.clone();
        full[0].room = false;
        assert_eq!(kind(&decide("d", &link, &design("p1"), &in_catalog(Some("g1")), Some(&full), 0)), "wait");
    }

    #[test]
    fn keeping_this_side_replaces_the_shipyard_design_without_reading_it() {
        let link = YardLink { keep_here: true, ship: None, design: None, ..linked("", "") };
        let unread = [yard(YardState::Loaded, Some("ENTITY_SHIP_a"), None)];
        assert_eq!(kind(&decide("d", &link, &design("p1"), &in_catalog(Some("g1")), Some(&unread), 0)), "give");
        // Keeping the shipyard's: it comes here once read.
        let link = YardLink { ship: None, design: None, ..linked("", "") };
        assert_eq!(kind(&decide("d", &link, &design("p1"), &in_catalog(Some("g1")), Some(&unread), 0)), "wait");
        let read = [yard(YardState::Loaded, Some("ENTITY_SHIP_a"), Some("s1"))];
        assert_eq!(kind(&decide("d", &link, &design("p1"), &in_catalog(Some("g1")), Some(&read), 0)), "take");
    }

    #[test]
    fn the_content_print_ignores_when_the_game_saved() {
        let mut bp = Blueprint::new_ship("Arrow", "test");
        bp.root.entity.set(Pos::new(1, 0, 0), Block::new(5, 3, false));
        let before = content_print(&bp);
        for entity in bp.blocks_mut() {
            let keys: Vec<_> = entity.segments().map(|(k, _)| *k).collect();
            for key in keys {
                let old = entity.segment(key).unwrap();
                let segment = yard_core::entity::Segment { blocks: old.blocks.clone(), count: old.count, version: old.version, timestamp: old.timestamp + 1000 };
                entity.insert_segment(key, segment);
            }
        }
        assert_eq!(content_print(&bp), before);
        bp.root.entity.set(Pos::new(1, 0, 0), Block::new(5, 4, false));
        assert_ne!(content_print(&bp), before);
    }

    #[test]
    fn the_content_print_ignores_how_the_game_numbered_the_docked_entities() {
        let docked = |dir: &str, ty: u16| {
            let mut part = Blueprint::new_ship("Part", "test").root;
            part.dir = dir.to_string();
            part.entity.set(Pos::new(0, 1, 0), Block::new(ty, 0, false));
            part
        };
        let mut bp = Blueprint::new_ship("Arrow", "test");
        bp.root.children = vec![docked("ATTACHED_0", 5), docked("ATTACHED_1", 6)];
        let before = content_print(&bp);
        // The same parts, numbered the other way round.
        bp.root.children = vec![docked("ATTACHED_0", 6), docked("ATTACHED_1", 5)];
        assert_eq!(content_print(&bp), before);
        // A part that changed, or that is docked one level further down, is a change.
        bp.root.children = vec![docked("ATTACHED_0", 6), docked("ATTACHED_1", 7)];
        assert_ne!(content_print(&bp), before);
        let mut deeper = docked("ATTACHED_0", 6);
        deeper.children = vec![docked("ATTACHED_0", 5)];
        bp.root.children = vec![deeper];
        assert_ne!(content_print(&bp), before);
    }
}
