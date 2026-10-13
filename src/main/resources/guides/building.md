# Building well

Workflow that produces good ships:
1. Block out the silhouette with a few large primitives (fill boxes, cylinders along z, ellipsoids). Get the proportions right before any detail.
2. Turn on x symmetry and work on the right side only.
3. Carve: fill with block "air" to cut hangars, windows, engine recesses and the interior.
4. Layer: a second hull colour for stripes and panels, glass for a bridge, lights for accents.
5. Soften hard edges with the wedge shape along long edges and corner shapes where three faces meet.

Proportions that read as a ship:
- Length (z) about 2.5 to 5 times the width (x); height (y) about half the width or less for most ships.
- Taper the nose: use cylinder with end_scale below 1, or stacked fills that step inward.
- Engines are distinct volumes at the stern, often cylinders along z with a glowing block inside (a light or coloured crystal armor).
- A bridge or cockpit sits on top or near the front, glazed with Glass.

Hull materials, weakest and lightest to strongest and heaviest: Basic Armor, Standard Armor, Advanced Armor. Each comes in many colours (Grey, White, Dark Grey, Black, Red, Blue, and more). Crystal Armor is translucent and good for glowing accents.

Interior: leave at least a 3 high, 3 wide corridor from an entry door to the core so the player can reach it. The core must stay reachable.

Checking: after each major step, scan a few layers (axis y for plan views, axis x for a side profile at x = 0) and look for holes, stray blocks and asymmetry.
