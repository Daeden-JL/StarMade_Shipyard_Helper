# Rails and docked entities

Anything that moves relative to the ship (turrets, doors on rails, elevators, drones in a hangar) is a separate entity docked to a rail on the ship. design_info lists the entities of the design; docked ones have paths like "ATTACHED_0", and every tool takes such a path as its `entity` argument to work inside that entity. Positions there are relative to that entity's own core.

Blocks:
- Rail Basic: track. A docked entity sits on it and moves along connected rails in the direction the arrows point.
- Rail Docker: the block on the docked entity that attaches to a rail. The docker rests in the cell in front of the rail's arrow face, arrow face to arrow face, arrows pointing the same way.
- Rail Turret Axis: lets the docked entity turn freely about the line through the axis block and the docker. Used for turrets.
- Rail Rotator Clock Wise and Counter Clock Wise: turn the docked entity by a quarter turn when activated.
- Rail Mass Enhancer: lets a ship carry heavier docked entities.
- Rail Speed Controller: sets the speed of rails linked to it.
- Rail Load and Rail Unload: move cargo between docked entities. Pickup Point and Pickup Rail catch entities.

Rails turn 24 ways. Place them with `facing` (the side the arrow face is on: "up" for a rail on a floor or on top of the hull) and `pointing` (where the arrow points).

A two-axis turret is two docked entities: a base docked to a Rail Turret Axis facing up on the ship (it turns left and right), and a barrel docked to a Rail Turret Axis on the side of the base (it tilts up and down), with the weapon computer and modules pointing forward and a BOBBY AI Module so it aims and fires by itself. Leave clearance for the barrel to swing.

Current limit: the tools build inside the entities a design already has, but cannot yet create a new docked entity or dock one. For a new turret, place the Rail Turret Axis where it should go, tell the player its position, and say that the turret itself has to be docked in the game or added to the design as an entity first.
