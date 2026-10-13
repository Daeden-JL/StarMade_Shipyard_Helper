#!/usr/bin/env python3
"""Turns crates/yard-core/data/shapes.json (written by ShapeDump.java from the
game's own classes) into crates/yard-core/src/shape_data.rs, the tables the
editor compiles in. Run from anywhere after re-running the dump:

    python3 rust/tools/shape-dump/gen_rust.py
"""
import json
import math
import pathlib

ROOT = pathlib.Path(__file__).resolve().parents[2]
data = json.loads((ROOT / "crates/yard-core/data/shapes.json").read_text())


def f(x):
    x = float(x)
    if abs(x) < 1e-9:
        x = 0.0
    text = repr(round(x, 6))
    return text if "." in text or "e" in text else text + ".0"


def vec(v):
    return "[" + ", ".join(f(c) for c in v) + "]"


def cross(a, b):
    return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]


def sub(a, b):
    return [a[i] - b[i] for i in range(3)]


def face(side, full, normal, verts):
    corners = ", ".join(vec(v) for v in verts)
    return f"RawFace {{ side: {side}, full: {'true' if full else 'false'}, normal: {vec(normal)}, verts: &[{corners}] }}"


def mirror(entry):
    # Flip X is a mirror across the YZ plane, flip Y across XZ, flip Z across XY.
    m = entry["mirror"]
    return f"[{m['yz']}, {m['xz']}, {m['xy']}]"


def solid(style):
    out = []
    for entry in data["shapes"][style]["orientations"]:
        faces = [
            face(-1 if fc["sideId"] is None else fc["sideId"], fc["full"], fc["normal"], fc["verts"])
            for fc in entry["faces"]
        ]
        name = entry.get("algorithm") or ""
        out.append(f"    // {entry['orientation']}: {name}. {entry.get('description', '')}\n"
                   f"    RawShape {{ faces: &[\n        " + ",\n        ".join(faces) + f",\n    ], mirror: {mirror(entry)} }},")
    return "\n".join(out)


def sprites():
    entries = data["shapes"]["3"]["orientations"]
    out = []
    for entry in entries[:6]:
        faces = []
        for quad in entry["render"]:
            if not quad["drawn"]:
                continue
            verts = [v["pos"] for v in quad["verts"]]
            n = cross(sub(verts[1], verts[0]), sub(verts[2], verts[0]))
            length = math.sqrt(sum(c * c for c in n)) or 1.0
            faces.append(face(-1, False, [c / length for c in n], verts))
        out.append(f"    // {entry['orientation']}: {entry['algorithm']}\n"
                   f"    RawShape {{ faces: &[\n        " + ",\n        ".join(faces) + f",\n    ], mirror: {mirror(entry)} }},")
    return "\n".join(out)


def slab_boxes():
    rows = []
    for slab in (1, 2, 3):
        boxes = [e["box"] for e in data["slabs"]["byOrientation"] if e["slab"] == slab]
        boxes.sort(key=lambda b: 0)  # already in orientation order
        assert len(boxes) == 6, boxes
        rows.append("    [" + ", ".join(vec(b) for b in boxes) + "],")
    return "\n".join(rows)


def rail_mirrors():
    return ", ".join(mirror(e) for e in data["shapes"]["6"]["orientations"])


# The two axes that span each cube side, lowest first: a face corner is named
# by where it sits along them.
PLANE = {0: (0, 1), 1: (0, 1), 2: (0, 2), 3: (0, 2), 4: (1, 2), 5: (1, 2)}


def corner_uvs(side, verts):
    """Texture corners at (a, b) = (0, 0), (1, 0), (0, 1) of a cube side, from the
    four vertices the game's mesher emits for it."""
    a, b = PLANE[side]
    at = {(round(v["pos"][a]), round(v["pos"][b])): v["uv"] for v in verts}
    assert len(at) == 4, (side, verts)
    for i in range(2):  # the fourth corner must follow from the other three
        assert at[(1, 1)][i] == at[(1, 0)][i] + at[(0, 1)][i] - at[(0, 0)][i], (side, verts)
    return "[" + ", ".join(f"[{at[k][0]}, {at[k][1]}]" for k in ((0, 0), (1, 0), (0, 1))) + "]"


def cube_uvs(style):
    rows = []
    table = data["cube"]["texOrder"][style]
    assert len(table) == 6
    for orientation in table:
        assert len(orientation) == 6
        rows.append("    [" + ", ".join(corner_uvs(side, verts) for side, verts in enumerate(orientation)) + "],")
    return "\n".join(rows)


def rail_faces():
    rows = []
    slots = data["cube"]["normal24"]["orientations"]
    shapes = data["shapes"]["6"]["orientations"]
    assert len(slots) == 24 and len(shapes) >= 24
    for o in range(24):
        by_side = {q["emittedForSide"]: q for q in shapes[o]["render"] if q["drawn"]}
        assert sorted(by_side) == [0, 1, 2, 3, 4, 5], (o, sorted(by_side))
        cells = [
            f"RawTex {{ slot: {slots[o]['textureSlotByWorldSide'][side]}, uv: {corner_uvs(side, by_side[side]['verts'])} }}"
            for side in range(6)
        ]
        rows.append(f"    // {o}: {slots[o]['algorithm']}, first texture toward {slots[o]['primaryName']}\n    [" + ", ".join(cells) + "],")
    return "\n".join(rows)


def rail_axes():
    """Where each orientation's own up and forward point, as steps along the axes."""
    slots = data["cube"]["normal24"]["orientations"]
    return ", ".join(f"[{vec_i(e['primaryDirSwitched'])}, {vec_i(e['secondaryDirSwitched'])}]" for e in slots)


def rail_bases():
    """The turn the game gives a 24-way block's 3D model, per orientation value."""
    slots = data["cube"]["normal24"]["orientations"]
    return ",\n".join("    [" + ", ".join(vec_i(row) for row in e["lodBasis"]) + "]" for e in slots)


def vec_i(v):
    return "[" + ", ".join(str(int(c)) for c in v) + "]"


for style in "1245":
    assert len(data["shapes"][style]["orientations"]) == 24

source = f"""//! Block geometry tables exported from the game's own classes.
//!
//! GENERATED by tools/shape-dump/gen_rust.py from data/shapes.json. Do not edit;
//! re-run the dump and the generator instead.

// The slanted normals are the game's numbers as exported, not constants to tidy up.
#![allow(clippy::approx_constant)]

/// One flat face. `side` is the game's side number (0 front +Z, 1 back -Z,
/// 2 top +Y, 3 bottom -Y, 4 +X, 5 -X) or -1 for a slanted face. Corners are in
/// the unit cell, counter-clockwise seen from outside.
pub(crate) struct RawFace {{
    pub side: i8,
    pub full: bool,
    pub normal: [f32; 3],
    pub verts: &'static [[f32; 3]],
}}

/// A block style at one stored orientation value. `mirror` is the orientation
/// value of its mirror image when flipped along X, Y and Z.
pub(crate) struct RawShape {{
    pub faces: &'static [RawFace],
    pub mirror: [u8; 3],
}}

/// How one side of a cube is textured.
pub(crate) struct RawTex {{
    /// Which of the block's six textures shows: 0 front, 1 back, 2 top,
    /// 3 bottom, 4 right, 5 left, as listed in the block config.
    pub slot: u8,
    /// The texture corner (u, v; v = 0 is the top of the image) at three
    /// corners of the side: lowest on both axes that span it, highest on the
    /// first of them, highest on the second. The axes are x and y for front
    /// and back, x and z for top and bottom, y and z for the other two.
    pub uv: [[u8; 2]; 3],
}}

pub(crate) static WEDGE: [RawShape; 24] = [
{solid("1")}
];

pub(crate) static CORNER: [RawShape; 24] = [
{solid("2")}
];

pub(crate) static TETRA: [RawShape; 24] = [
{solid("4")}
];

pub(crate) static HEPTA: [RawShape; 24] = [
{solid("5")}
];

/// Sprites (plants, decorations) are two crossed sheets, listed once per facing.
pub(crate) static SPRITE: [RawShape; 6] = [
{sprites()}
];

/// Mirror images of the 24 orientations of rails and other blocks that can be
/// turned every way. They are drawn as full cubes.
pub(crate) static RAIL_MIRROR: [[u8; 3]; 24] = [{rail_mirrors()}];

/// Textures of the blocks that turn 24 ways (rails, dockers, rotators): per
/// stored orientation value, per cell side.
pub(crate) static RAIL_TEX: [[RawTex; 6]; 24] = [
{rail_faces()}
];

/// Where a 24-way block's own up (the side its first texture faces) and its
/// own forward point, per orientation value.
pub(crate) static RAIL_AXES: [[[i8; 3]; 2]; 24] = [{rail_axes()}];

/// The turn the game gives the 3D model of a 24-way block that has one (a
/// light bar, a console), per orientation value: three rows, so that a point
/// of the model lands at row . point on each axis.
pub(crate) static RAIL_BASIS: [[[i8; 3]; 3]; 24] = [
{rail_bases()},
];

/// Texture corners of plain cubes and slabs per orientation value 0..=5 and
/// cell side, in the layout of [`RawTex::uv`]. The game turns the texture
/// with the block.
pub(crate) static CUBE_UV: [[[[u8; 2]; 3]; 6]; 6] = [
{cube_uvs("NORMAL")}
];

/// The same for blocks whose side textures point where the block points
/// (`SideTexturesPointToOrientation` in the block config).
pub(crate) static CUBE_UV_POINTING: [[[[u8; 2]; 3]; 6]; 6] = [
{cube_uvs("ORIENT")}
];

/// Slab boxes as min x, y, z, max x, y, z. Indexed by slab - 1 (three quarters,
/// half, one quarter thick), then by orientation value 0..=5.
pub(crate) static SLAB_BOX: [[[f32; 6]; 6]; 3] = [
{slab_boxes()}
];
"""
target = ROOT / "crates/yard-core/src/shape_data.rs"
target.write_text(source)
print(f"wrote {target} ({len(source)} bytes)")
