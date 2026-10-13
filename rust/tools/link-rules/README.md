# link-rules

Where the editor's linking rules come from, and how to check them again after
a game update.

```
./run.sh
```

runs the game's own classes without a display and writes, for a ship and for a
station, every pair of block types the game lets a player link (select the
first with C, press V on the second). The result is the test data in
`crates/yard-core/tests/data/link_pairs_*.txt`; `cargo test -p yard-core --test
link_rules` then checks that `yard_core::links::Rules::can_link` gives the same
answer for all 1516 x 1516 pairs.

Written against StarMade 0.304.7.

## What the game does

- `SegmentController.setCurrentBlockController` is where V ends up. It asks
  two questions and both must hold: `ManagerContainer.canBeControlled(a, b)`
  (the entity has a system for `b`, or the pair is in the game's fixed table
  `ElementInformation.canBeControlled`), and then `b.controlledBy` contains
  `a`, or `a` is a logic block, or the pair is a weapon combination, or the
  fixed table again.
- Which systems an entity has differs between ships and stations
  (`ShipManagerContainer`, `SpaceStationManagerContainer`), so a few pairs
  do too. The lists are `SYSTEMS`, `SHIP_SYSTEMS` and `STATION_SYSTEMS` in
  `crates/yard-core/src/links.rs`; `build/out/module_keys_*.txt` has them
  after a run. Ships also have the factory systems when the server allows
  factories on ships, which the probe's ship does not.
- V on a linked block unlinks it. With Left Shift the whole group of touching
  blocks of the same type follows what the clicked block does.
- `ControlElementMap.switchControllerForElement`: a block that is not a logic
  block, an inventory, rail track, a sensor input or a display has one
  controller. Linked from another it leaves the first. The game treats a
  weapon module found under two computers as cheating and unlinks it.
- `UsableControllableElementManager.addConnectionIfNecessary`: a weapon
  computer has one place each for a second weapon, an effect computer and a
  light. On a ship, a computer that is let go answers to the core again.
- Placing a block links it to the ship core if the core controls that type,
  else to the block selected with C where the two may be linked.

`LinkStateProbe.java` exercises the game's `ControlElementMap` for the "one
controller" cases; `pairs.txt` is a short list of pairs to print in detail.

## Notes

- The game's own code refers to block types 7, 88 and 289, which this block
  list no longer has. The probe puts placeholders in while the containers
  are built and takes them out again.
- Block 290 counts as a system because of those old docking modules, so a
  logic block may be linked to it. Nothing depends on it.
