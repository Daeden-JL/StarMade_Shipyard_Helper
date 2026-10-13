# Linking

A link runs from a controller to a block it controls. In the game a player selects the controller with C and presses V on the other block; the `link` tool does the same and follows the same rules. `place_system` links as it places. `list_links` and `get_block` show what is linked.

What may be linked:
- A computer to its own modules: Cannon Computer to Cannon Barrel, Missile Computer to Missile Tube, and so on. find_blocks shows "controls" and "controlled by" for a block.
- The Ship Core to computers. This happens by itself when a computer is placed on a ship.
- A weapon computer to one second weapon computer, one effect computer and one light (see the weapons guide). Each of these places holds one block: a new one replaces the old.
- A logic block (Activation Module, Button, AND, OR, NOT, Flip Flop, Delay...) to anything that can be switched: lights, doors, other logic blocks, rails, Rail Dockers, computers.
- Storage and factories to each other (items flow along the link), and to Cargo Space.
- A Rail Speed Controller to rail track and to Activation Modules.

What happens besides:
- Most blocks have one controller. Linked from another, such a block leaves the one it had. Logic blocks, doors, rail track, storage and displays keep every controller they are given.
- A computer that stops being another computer's second weapon or effect answers to the Ship Core again.
- A block that is removed or replaced by another type loses all its links.
- Pairs the game does not allow are not linked; the tool names what it left out. Hull never takes part in links.

Station and ship differ in a few pairs (a ship has thrusters, a station has shops and warp gates); the tools use the rules of the entity they work on.
