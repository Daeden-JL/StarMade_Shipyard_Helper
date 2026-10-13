# Power (reactors)

Every system draws power. Power comes from Reactor Power blocks.
- Place Reactor Power blocks touching each other so they form one group. Only the largest group on the ship is the active reactor.
- A reactor needs Reactor Stabilizer blocks to run at full output: roughly as many stabilizers as reactor blocks.
- Stabilizers only work well at a distance from the reactor, and the distance needed grows with reactor size. On small ships put the stabilizers at the opposite end of the hull from the reactor. With low stabilization the reactor produces far less power.
- Stabilizers need no link to the reactor. Neither do reactor blocks need a link to anything.
- Reactor chambers add abilities. A chamber must be connected to the reactor by a path of Reactor Conduit blocks and be about a tenth of the reactor's size or more.
- Do not use the block named "Power (Old)" or other old power blocks; the game no longer uses them.

Sizing rule of thumb: reactor blocks equal to about 5 to 10 percent of the ship's block count, the same number of stabilizers, adjusted when the player reports power problems in the game.

Typical layout: the reactor as a compact `fill` box low in the middle or rear of the hull, the stabilizers as a slab near the nose. Use find_blocks "reactor" for the exact names.
