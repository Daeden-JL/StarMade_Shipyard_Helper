# shape-dump

Generates `rust/crates/yard-core/data/shapes.json`: what every StarMade block
style looks like at every orientation value, taken from the game's own classes.

```
./run.sh [starmade_dir] [output.json]
```

`starmade_dir` defaults to `$STARMADE_DIR`, then `~/.smtest2/vol`. It needs
`StarMade.jar`, `lib/` and `data/` (a server install is enough). Java comes from
`$JAVA_HOME` (default `/opt/homebrew/opt/openjdk@21`). The install is only read:
the config files are copied to a temporary directory and the JVM runs there.
No OpenGL or native library is needed.

Written against StarMade 0.304.7. Rerun it after a game update and diff the
output; `checks.problems` must stay empty.

## Where the numbers come from

Read from the game at run time:

- `BlockShapeAlgorithm.algorithms` - one algorithm object per style and
  orientation. `getShape()` gives the collision hull, `createSide()` is what the
  mesher calls for each of the six sides, `xyMappings` etc. are the mirror tables.
- `HalfBlockArray.getHalfBlockConfig` - slab vertex offsets.
- `CubeMeshBufferContainer.getOrientationCode3/6/24` - which texture slot a
  cube shows on which world side.
- `BlockShapeAlgorithm.TexOrderStyle` maps (loaded from
  `data/textures/texOrder*.config`) - texture corner order for plain cubes.
- `Element`, `BlockStyle`, `ElementKeyMap`, `Oriencube`.

Rewritten by hand: the position formula of the cube vertex shader
(`data/shader/cube/quads13/cube-3rd.vsh`), as `ShapeDump.shaderPos`. The game's
Java port of that shader, `Segment2ObjWriter.softwareShader`, has the same
arithmetic. Its two uniforms are read from the game
(`CubeMeshQuadsShader13.quadPosMark`, `GlUtil.DIRECTIONSf`).

Faces are computed twice and compared: once as the faces of the collision hull,
once as the quads the mesher emits. They agree for every solid orientation.

## File layout

- `convention` - coordinate, winding and uv conventions. Read this first.
- `sides` - side ids, direction vectors, opposites.
- `styles` - the seven block styles with orientation counts and block statistics.
- `shapes["1".."6"].orientations[value]` - per stored orientation value:
  `hull`, `faces` (outward, counter-clockwise, with `side`/`full`),
  `sideCoverage`, `description`, `mirror`, `sameAs`, and `render` (the mesher's
  quads with texture corners).
- `cube` - texture slot tables for six sided, three sided and 24-orientation
  cubes, plus texture corner order per orientation and side.
- `slabs` - box per slab value and orientation.
- `handedness` - what the texture mapping says about how the game displays x.
- `checks` - results of the built-in sanity checks.
- `notes` - things that are easy to get wrong.
