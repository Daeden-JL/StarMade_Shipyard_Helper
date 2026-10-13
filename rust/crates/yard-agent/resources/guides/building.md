# Building well

A workflow that produces good ships:
1. Block out the silhouette with a few large shapes: one `hull` call for the main body, `cylinder` for engines and barrels, `ellipsoid` for pods and domes, `fill` for decks, wings and boxes. Get the proportions right before any detail.
2. Build one side only where the ship is not centred on x = 0 (a wing, a side pod at x > 0, the pilot's left), then `mirror` with axis x. Shapes centred on x = 0 are symmetric already.
3. Carve with `fill` and block "air", or `remove`: hangars, windows, engine recesses, the interior.
4. Layer: a second hull colour for stripes and panels (`fill` or `replace` in a box), Glass for a bridge, lights for accents.
5. Soften hard edges with wedges along long edges and corners where three faces meet (guide "shapes").

Proportions that read as a ship:
- Length (z) about 2.5 to 5 times the width (x); height (y) about half the width or less.
- The nose is at high z and tapers: end a `hull` in a small or zero section, or use a `cylinder` with a smaller end_radius.
- Engines are distinct volumes at the stern (low z), often hollow cylinders along z with open ends and a glowing block inside.
- A bridge or cockpit sits on top or near the front, glazed with Glass.

Hull materials, lightest and weakest first: Basic Armor, Standard Armor, Advanced Armor. Each comes in many colours (Grey, Dark Grey, White, Black, Red, Blue and more) and in wedge, corner, tetra, hepta and slab shapes. Crystal Armor is translucent and good for glowing accents. Use find_blocks for exact names.

Interior: a hollow hull leaves room for systems. Keep a corridor at least 3 high and 3 wide from an entry door to the Ship Core so the pilot can reach it.

Checking: after each major step look at the result. `scan_region` with axis y shows deck plans, axis x a side profile, axis z cross-sections; `count_blocks` and `design_info` give numbers. Look for holes, stray blocks and lopsided halves, and fix them.
