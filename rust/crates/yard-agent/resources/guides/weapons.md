# Weapons

A weapon is a computer plus modules linked to it. `place_system` places and links both in one call.

| Weapon | Computer | Module |
|---|---|---|
| Cannon | Cannon Computer | Cannon Barrel |
| Missile | Missile Computer | Missile Tube |
| Beam | Damage Beam Computer | Damage Beam Module |

Rules:
- Place the modules touching each other in a group. A group fires one shot from its foremost block, so build groups as lines or bundles along z with a clear line of fire ahead: nothing in front of the last module.
- Unlinked modules do nothing. A module belongs to one computer only: linked from a second computer, it leaves the first.
- Several separate groups on one computer each fire, but power cost rises with the number of groups. A few big groups beat many small ones. More modules in a group means more damage per shot.
- The computer can sit anywhere; put it inside the hull. On a ship the Ship Core controls every computer placed, which is what puts the weapon on the pilot's bar. The tools make that link for you.

Combining weapons (link the main computer to the other computer with `link`):
- A weapon computer has one place for a second weapon computer, which then shapes how the first fires instead of firing itself (cannon + cannon fires faster, for example). Its modules count relative to the main weapon's, with full effect at one to one. Linking another second weapon replaces the earlier one, and the Ship Core takes that one back.
- It has one place for an effect computer with its effect modules, which changes the damage type.
- It has one place for a light block, which colours its shots.
- The direction is always main weapon computer to second, effect or light.

Other tools work the same way, computer plus modules: Salvage Computer and Salvage Module for mining, Astrotech Computer and Astrotech Module for repair. find_blocks shows what each computer controls.

Turrets are separate docked entities; see the rails guide.
