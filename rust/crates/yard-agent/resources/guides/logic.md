# Logic

Logic blocks send on/off signals along links. A link has a direction: from the block that sends to the block it drives. Use `link` with the sender as the controller.

Blocks (find_blocks "signal", "activation", "button", "trigger" for exact names):
- Activation Module: a switch. Players toggle it; it passes its state to everything it is linked to. Also the general "wire" between other logic blocks.
- Button: sends a short pulse when pressed.
- AND-Signal: on when all blocks linked into it are on. OR-Signal: on when any is. NOT-Signal: the inverse of its input.
- Flip Flop: toggles its state each time it receives an on signal.
- DELAY-Signal: passes its input on after a short delay. Chain several for longer delays.
- Trigger (Area) Controller with Trigger (Area) blocks: signals when a player enters the area. Trigger (Step On): signals when stepped on.
- Sensor: compares a ship value (shields, power, cargo fill) or reads a linked door or storage.
- Wireless Logic Module: carries a signal between two entities, for example ship to turret.
- Inner Ship Remote: gives the pilot a bar button that toggles it.
- Display Module: shows text.

What logic can drive: doors (open and close), lights (on and off), weapon and tool computers (fire while on), rails and Rail Dockers, Rail Speed Controllers (speed follows the number of active Activation Modules linked to them), and other logic blocks. A logic block may be driven by several others and drive many.

Patterns:
- Door with a button on each side: both Buttons to one OR-Signal, the OR-Signal to a Flip Flop, the Flip Flop to every door block.
- Clock: NOT-Signal to DELAY-Signal and the DELAY-Signal back to the NOT-Signal. More delays in the loop slow it.
- Light switch: one Activation Module linked to all lights of a room (`link` with a box and block "White Light").

Layout: keep logic together on a hidden panel under a floor or behind a wall, a block of space between unrelated circuits, and put the Button or Activation Module the player uses where it can be reached.
