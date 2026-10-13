# Weapons

A weapon is a computer plus modules linked to it.

| Weapon | Computer | Module |
|---|---|---|
| Cannon | Cannon Computer | Cannon Barrel |
| Missile | Missile Computer | Missile Tube |
| Beam | Damage Beam Computer | Damage Beam Module |

Rules:
- Place the modules touching each other in a group. A group fires one shot from its foremost block, so build groups as lines or bundles pointing along +z with a clear line of fire ahead (nothing in front of the last module).
- Link the computer to its modules with the link tool (a box around the modules is easiest). Unlinked modules do nothing.
- Several separate groups on one computer each fire, but power cost rises with the number of groups. A few big groups beat many small ones.
- More modules in a group means more damage per shot.
- The computer can sit anywhere on the ship; put it inside the hull.
- Missiles also need Missile Capacity Module blocks somewhere on the ship to hold ammunition.

Combining weapons:
- Linking one weapon computer to a second weapon computer makes the second a support system that changes how the first fires (for example cannon + cannon fires faster, cannon + beam hits harder at longer range). Support modules count relative to the main weapon's module count, with full effect at a 1 to 1 ratio.
- Effect computers (EM Effect Computer, Heat Effect Computer, Kinetic Effect Computer) with their effect modules can be linked from a weapon computer the same way to change its damage type.
- Link direction is main weapon computer to support computer or effect computer.

Other tools that work the same way (computer, modules, link): Salvage Computer and Salvage Module for mining, Astrotech Computer and Astrotech Module for repair, Tractor Beam Computer and Tractor Beam Module, Push Pulse Computer and Push Pulse Module.

Turrets are separate docked entities; see the rails guide.
