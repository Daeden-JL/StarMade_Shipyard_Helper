# Rails and docked entities

Anything that moves relative to the ship (turrets, doors on rails, elevators, drones in a hangar) is a separate entity docked to a rail on the ship.

Blocks:
- Rail Basic: track. A docked entity sits on it and can move along connected rails. The rail's facing sets which way docked entities are oriented.
- Rail Docker: the block on the docked entity that attaches to a rail. Every docked entity needs one, and its own Ship Core.
- Rail Turret Axis: a rail that lets the docked entity rotate freely around the axis pointing out of the rail's face. Used for turrets.
- Rail Rotator Clock Wise and Rail Rotator Counter Clock Wise: rotate the docked entity by a quarter turn when activated.
- Rail Mass Enhancer: lets a ship carry heavier docked entities.
- Rail Speed Controller: sets how fast entities move on linked rails.
- Rail Load and Rail Unload: move cargo between docked entities.
- Pickup Point, Pickup Rail and Shootout Rail: catch and launch entities.

A two-axis turret is two entities:
1. On the ship: a Rail Turret Axis facing up (or outward from the hull surface).
2. Turret base: its own core, a Rail Docker on its underside docked to the ship's axis (this gives left-right rotation), and a Rail Turret Axis on its side facing sideways.
3. Turret barrel: its own core, a Rail Docker docked to the base's sideways axis (this gives up-down rotation), plus the weapon computer and modules pointing forward, and a BOBBY AI Module so it aims and fires by itself.

Leave clearance: the barrel must be able to swing without hitting the ship's hull or the base.

Current limitation: this assistant can place rail blocks on the design but cannot yet create the docked entities themselves. Place the Rail Turret Axis or Rail Basic where the turret should go, tell the player the position, and describe what the docked entity needs.
