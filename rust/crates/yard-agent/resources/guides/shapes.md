# Shapes and directions

Directions are the pilot's: up (+Y), down (-Y), front (+Z, the nose), back (-Z, the stern), left (+X) and right (-X). Mind the last two: +X is the pilot's LEFT.

Most hull blocks come in these shapes (argument `shape`; find_blocks lists what a block has):
- cube: the full block.
- wedge: a ramp, half a cube cut along a diagonal. It has one slanted face.
- corner: a pyramid-like piece with a square base and its point above one corner of the base. It has two slanted faces.
- tetra: a small corner piece, the cube's corner cut off and kept. One slanted face.
- hepta: a cube with one corner cut off. One slanted face.
- slab_3/4, slab_1/2, slab_1/4: thin plates.

Turning them (argument `slope`, a list of directions the slanted face looks toward):
- wedge, two directions. ["up", "back"] is a ramp rising toward the front: solid at the bottom and front, open to the top and back. For the sloped upper edge of a hull's left side use ["up", "left"]; on the right side ["up", "right"]. For a nose that slopes down toward the front use ["up", "front"].
- tetra and hepta, three directions: the corner that is cut off, e.g. ["up", "left", "front"].
- corner, three directions: first the way its point rises from its base, then the two ways its slopes look, e.g. ["up", "left", "back"].
- slabs and blocks with a front take `facing`: a slab facing "up" lies on the floor of its cell, facing "down" hangs under the ceiling.
- rails and dockers take `facing` (the side the arrow face is on) and `pointing` (where the arrow points).

`mirror` turns sloped blocks correctly, so shape one side of the ship and mirror it. `replace` keeps shapes and turns when recolouring. `get_block` tells how an existing block is turned, also as the raw `orientation` number, which every placing tool accepts in place of slope and facing.

Smooth lines: a diagonal edge is a row of wedges, one per step of the stair they cover. Where two sloped edges meet at an outer corner use a corner or tetra; at an inner corner a hepta.
