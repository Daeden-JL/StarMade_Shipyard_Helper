# Logic

Logic blocks send on/off signals through links. Links have a direction: from the source block to the block it drives. Use the link tool with the source as controller.

Blocks:
- Activation Module: a switch. Players toggle it; it passes its state to everything it links to. Also the general-purpose "wire" between other logic blocks.
- Button: sends a short pulse when pressed.
- AND-Signal: on when all blocks linking into it are on.
- OR-Signal: on when any block linking into it is on.
- NOT-Signal: the inverse of its input.
- Flip Flop: toggles its state each time it receives an on signal.
- DELAY-Signal: passes its input on after a short delay. Chain several for longer delays.
- Signal (Delay-Non-Repeating): like delay, but fires once per input change.
- Trigger (Area) Controller with Trigger (Area) blocks: sends a signal when a player enters the area blocks.
- Trigger (Step On): signals when stepped on.
- Sensor: compares a ship value (shields, power, cargo fill) against a threshold.
- Wireless Logic Module: carries a signal between two entities (for example ship to turret).
- Inner Ship Remote: gives the pilot a hotbar button that toggles it.
- Display Module: shows text.

What logic can drive (link from the logic block to the target):
- Doors (Plex Door, Glass Door, Blast Door): open and close.
- Lights: on and off.
- Weapon and tool computers: fire while the signal is on.
- Rail blocks: change rail direction or type; Rail Speed Controller sets rail speed from the number of active Activation Modules linked to it.
- Other logic blocks.

Patterns:
- Door with a button on each side: Button A to OR-Signal, Button B to the same OR-Signal, OR-Signal to Flip Flop, Flip Flop to every door block.
- Clock (repeating pulse): NOT-Signal to DELAY-Signal, DELAY-Signal back to the NOT-Signal. Add more delays in the loop to slow it.
- Light switch: Activation Module linked to all the lights in a room.

Layout: keep logic blocks together on a hidden panel (under a floor or behind a wall), one block of space between unrelated circuits, and place the player-facing Button or Activation Module where it can be reached.
