# Shields

- Shield Capacitor blocks store shield strength. Shield-Recharger blocks regenerate it.
- Neither needs linking. Place them in solid groups inside the hull with `fill_inside`.
- A reasonable starting mix is about three capacitors for every recharger.
- Recharging draws power, so a ship with many rechargers needs a bigger reactor.

# Thrust

- Thruster Module blocks move the ship. They need no linking and can be anywhere, but by convention they go at the stern (low z), behind engine housings.
- In an existing hull, place them with fill_inside: block Thruster Module, start_from back, max_blocks the number wanted. For separate engines, give each a box around its opening at the stern and its own max_blocks; if the player outlined the engines, find_outlines with fill_with "Thruster Module" and blocks_each fills behind every outline in one call. inside_space shows where the room is.
- Thrust needed scales with mass. Start with thrusters equal to about 5 to 10 percent of the ship's block count for a nimble ship, less for a slow hauler.

# Other useful blocks

- Camera: lets the pilot look from another point of the ship. Put one at the nose.
- Storage and Cargo Space: cargo. Cargo Space is linked from the storage block it extends.
- Faction Module: lets the owner claim the ship for a faction.
- Build Block: lets players enter build mode from that point.
- Gravity Unit: gives players gravity inside the ship.
- Lights (White Light and other colours): interior and accent lighting. They are placed switched on.
- Plex Door, Glass Door, Blast Door: doors, opened by activating them or by logic.

Use find_blocks for the exact names before placing any of these.
