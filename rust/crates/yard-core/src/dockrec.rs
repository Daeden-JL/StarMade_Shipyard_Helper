//! The record a parent's meta file keeps for one docked entity, changed in
//! place: positions moved when an entity is translated, and new records
//! made for a freshly docked one. The layout is described by
//! [`crate::dock::Dock::read`], which reads the same record for looking at;
//! this goes through [`crate::rawtag`] so that nothing else in it changes.

use crate::metafile::offset;
use crate::shape::{turn_step, GridTurn};
use crate::rawtag::{Member, RawTag, Value};
use crate::{Error, Pos, Result};

pub struct DockRecord {
    tag: RawTag,
}

fn bad(detail: impl Into<String>) -> Error {
    Error::Malformed { what: "dock record", detail: detail.into() }
}

/// The members of the request structure inside a record: the rail piece, the
/// docker piece, the turret turn, where the rail has moved it, the cell the
/// docker must reach, and the rest.
fn request(tag: &mut RawTag) -> Option<&mut Vec<Member>> {
    let Value::Struct(top) = &mut tag.value else { return None };
    top.get_mut(1)?.value.members_mut()?.first_mut()?.value.members_mut()
}

/// The position inside a piece (a struct of name, position, type...): the
/// first position it holds.
fn piece_pos(piece: &mut Value) -> Option<&mut [i32; 3]> {
    piece.members_mut()?.iter_mut().find_map(|m| if let Value::Vec3i(p) = &mut m.value { Some(p) } else { None })
}

fn moved(p: &mut [i32; 3], shift: i32, f: &dyn Fn(Pos) -> Pos) -> [i32; 3] {
    let q = f(Pos::new(p[0] - shift, p[1] - shift, p[2] - shift));
    let delta = [q.x + shift - p[0], q.y + shift - p[1], q.z + shift - p[2]];
    *p = [q.x + shift, q.y + shift, q.z + shift];
    delta
}

/// Turns the orientation a piece holds. The current form holds the value as
/// the blocks do; the old form (no leading byte) holds four bits of it and
/// keeps the fifth, inverted, in the "active" field after it.
fn turn_piece(piece: &mut Value, orient: &dyn Fn(u16, u8) -> u8) -> Result<()> {
    let fields = piece.members_mut().ok_or_else(|| bad("a block is missing"))?;
    let old = !matches!(fields.first().map(|m| &m.value), Some(Value::Byte(_)));
    let fields = if old { &mut fields[..] } else { &mut fields[1..] };
    let number = |i: usize, fields: &[Member]| fields.get(i).and_then(|m| m.value.int()).ok_or_else(|| bad("a block is incomplete"));
    let ty = number(2, fields)? as u16;
    let (o, active) = (number(3, fields)?, number(4, fields)?);
    let set = |v: &mut Value, n: i64| match v {
        Value::Byte(b) => *b = n as i8,
        Value::Short(s) => *s = n as i16,
        Value::Int(i) => *i = n as i32,
        _ => {}
    };
    if old {
        let turned = orient(ty, (o + if active > 0 { 0 } else { 16 }).rem_euclid(24) as u8) as i64;
        let (low, fifth) = (turned % 16, turned >= 16);
        set(&mut fields[3].value, low);
        set(&mut fields[4].value, if fifth { 0 } else { 1 });
    } else {
        set(&mut fields[3].value, orient(ty, o.rem_euclid(32) as u8) as i64);
    }
    Ok(())
}

/// Turns a transform (rows of a turn, the shift in the last column) as the
/// coordinates it is written in turn: the turn becomes `turn · t · turnᵀ`
/// and the shift `turn · s`. A grid turn only moves numbers about and flips
/// their signs, so this does just that and a turn and its reverse give back
/// the very same numbers.
fn turn_matrix(m: &mut [f32; 16], turn: &GridTurn) {
    // Row r of the turn picks axis `from[r]` with sign `sign[r]`.
    let from = turn.map(|row| row.iter().position(|v| *v != 0).unwrap_or(0));
    let sign = [0, 1, 2].map(|r| turn[r][from[r]] < 0);
    let at = |r: usize, c: usize| m[r * 4 + c];
    let flip = |v: f32, negative: bool| if negative { -v } else { v };
    let mut out = *m;
    for r in 0..3 {
        for c in 0..3 {
            out[r * 4 + c] = flip(at(from[r], from[c]), sign[r] != sign[c]);
        }
        out[r * 4 + 3] = flip(at(from[r], 3), sign[r]);
    }
    *m = out;
}

impl DockRecord {
    /// Reads a record of the kind that names its blocks: a dock, or one being
    /// made. Anything else (a request to undock) holds no positions this
    /// could vouch for, so it is refused.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut tag = RawTag::parse(bytes)?;
        let kind = match &tag.value {
            Value::Struct(top) => top.first().and_then(|m| m.value.int()),
            _ => None,
        };
        if !matches!(kind, Some(1 | 3)) {
            return Err(bad("not the record of a dock"));
        }
        let req = request(&mut tag).ok_or_else(|| bad("no dock request inside"))?;
        if req.len() < 5 || piece_pos(&mut req[0].value).is_none() || piece_pos(&mut req[1].value).is_none() || !matches!(req[4].value, Value::Vec3i(_)) {
            return Err(bad("the rail, the docker or the target cell is missing"));
        }
        Ok(Self { tag })
    }

    pub fn write(&self) -> Vec<u8> {
        self.tag.write()
    }

    /// Moves the rail block's position (on the parent) through `f`, and the
    /// cell the docker must reach, which is where the rail block's arrow
    /// points from it, by as much. `version` is the meta file's.
    pub fn map_rail(&mut self, version: i32, f: &dyn Fn(Pos) -> Pos) {
        let req = request(&mut self.tag).expect("checked when read");
        let delta = moved(piece_pos(&mut req[0].value).expect("checked when read"), offset(version), f);
        if let Value::Vec3i(target) = &mut req[4].value {
            for i in 0..3 {
                target[i] += delta[i];
            }
        }
    }

    /// Moves the docker block's position (on the docked entity) through `f`.
    pub fn map_docker(&mut self, version: i32, f: &dyn Fn(Pos) -> Pos) {
        let req = request(&mut self.tag).expect("checked when read");
        moved(piece_pos(&mut req[1].value).expect("checked when read"), offset(version), f);
    }

    /// Turns what the record holds of the rail with the parent, after
    /// [`DockRecord::map_rail`] has moved it: the rail block's orientation
    /// (through `orient`, given its type and value), the cell the docker must
    /// reach round the rail, and the rail movement the entity was saved with.
    pub fn turn_rail(&mut self, turn: &GridTurn, orient: &dyn Fn(u16, u8) -> u8) -> Result<()> {
        let req = request(&mut self.tag).expect("checked when read");
        let rail = *piece_pos(&mut req[0].value).expect("checked when read");
        turn_piece(&mut req[0].value, orient)?;
        if let Value::Vec3i(target) = &mut req[4].value {
            let step = turn_step(turn, [0, 1, 2].map(|i| target[i] - rail[i]));
            *target = [0, 1, 2].map(|i| rail[i] + step[i]);
        }
        // The two rail transforms are in the parent's coordinates.
        for i in [3, 5] {
            if let Some(Member { value: Value::Matrix4(m), .. }) = req.get_mut(i) {
                turn_matrix(m, turn);
            }
        }
        Ok(())
    }

    /// Turns what the record holds of the docker with the docked entity: the
    /// docker block's orientation and the turret turn, which is in the docked
    /// entity's coordinates.
    pub fn turn_docker(&mut self, turn: &GridTurn, orient: &dyn Fn(u16, u8) -> u8) -> Result<()> {
        let req = request(&mut self.tag).expect("checked when read");
        turn_piece(&mut req[1].value, orient)?;
        if let Some(Member { value: Value::Matrix4(m), .. }) = req.get_mut(2) {
            turn_matrix(m, turn);
        }
        Ok(())
    }

    /// The rail and docker positions, in design coordinates.
    pub fn positions(&mut self, version: i32) -> (Pos, Pos) {
        let shift = offset(version);
        let req = request(&mut self.tag).expect("checked when read");
        let mut get = |i: usize| {
            let p = *piece_pos(&mut req[i].value).expect("checked when read");
            Pos::new(p[0] - shift, p[1] - shift, p[2] - shift)
        };
        (get(0), get(1))
    }

    /// A new record for a turret at rest, in the form the current game
    /// writes: both pieces at their given places (positions in design
    /// coordinates; the rail on the parent, the docker on the child), no
    /// turn and no movement, and the docker's target cell one step along the
    /// rail's arrow side from the rail (`target`, on the parent).
    pub fn new_turret(rail: Piece<'_>, docker: Piece<'_>, target: Pos) -> Self {
        let shift = crate::entity::CORE_OFFSET;
        let unnamed = |value| Member { name: None, value };
        let piece = |p: &Piece| {
            Value::Struct(vec![
                unnamed(Value::Byte(0)),
                unnamed(Value::Text(format!("ENTITY_SHIP_{}", p.entity))),
                unnamed(Value::Vec3i([p.pos.x + shift, p.pos.y + shift, p.pos.z + shift])),
                unnamed(Value::Short(p.ty as i16)),
                unnamed(Value::Byte(p.orientation as i8)),
                unnamed(Value::Byte(0)),
                unnamed(Value::Byte(127)),
            ])
        };
        let mut identity = [0.0; 16];
        for i in 0..4 {
            identity[i * 5] = 1.0;
        }
        let mut request = vec![unnamed(piece(&rail)), unnamed(piece(&docker))];
        request.extend([Value::Matrix4(identity), Value::Matrix4(identity)].map(unnamed));
        request.push(unnamed(Value::Vec3i([target.x + shift, target.y + shift, target.z + shift])));
        request.push(unnamed(Value::Matrix4(identity)));
        request.extend((0..5).map(|_| unnamed(Value::Byte(0))));
        let top = vec![
            unnamed(Value::Byte(1)),
            unnamed(Value::Struct(vec![unnamed(Value::Struct(request))])),
            unnamed(Value::Struct(Vec::new())),
        ];
        Self { tag: RawTag { gzipped: false, version: 0, name: None, value: Value::Struct(top), trailing: Vec::new() } }
    }
}

/// One block of a new record.
pub struct Piece<'a> {
    /// The name of the entity it is on, as the game spells it after `ENTITY_SHIP_`.
    pub entity: &'a str,
    pub pos: Pos,
    pub ty: u16,
    pub orientation: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::Dock;

    fn sample() -> DockRecord {
        DockRecord::new_turret(
            Piece { entity: "Ship", pos: Pos::new(3, 4, 5), ty: 665, orientation: 14 },
            Piece { entity: "Ship0", pos: Pos::new(0, -1, 0), ty: 663, orientation: 10 },
            Pos::new(3, 5, 5),
        )
    }

    #[test]
    fn a_new_record_reads_back_as_the_dock_it_describes() {
        let record = sample();
        let bytes = record.write();
        let dock = Dock::read(&bytes, 5).unwrap().unwrap();
        assert_eq!((dock.rail.pos, dock.rail.ty, dock.rail.orientation), (Pos::new(3, 4, 5), 665, 14));
        assert_eq!((dock.docker.pos, dock.docker.ty, dock.docker.orientation), (Pos::new(0, -1, 0), 663, 10));
        assert_eq!(dock.placement().cell(dock.docker.pos), Some(Pos::new(3, 5, 5)));
        // And it is the same bytes after being read and written.
        assert_eq!(DockRecord::parse(&bytes).unwrap().write(), bytes);
    }

    #[test]
    fn moving_the_rail_moves_the_target_cell_and_moving_the_docker_touches_nothing_else() {
        let mut record = sample();
        record.map_rail(5, &|p| Pos::new(p.x - 1, p.y + 2, p.z));
        let dock = Dock::read(&record.write(), 5).unwrap().unwrap();
        assert_eq!(dock.rail.pos, Pos::new(2, 6, 5));
        let tag = RawTag::parse(&record.write()).unwrap();
        let req = tag.value.members().unwrap()[1].value.members().unwrap()[0].value.members().unwrap();
        assert_eq!(req[4].value, Value::Vec3i([2 + 16, 7 + 16, 5 + 16]));
        let before = record.write();
        record.map_docker(5, &|p| Pos::new(p.x, p.y, p.z + 3));
        assert_eq!(Dock::read(&record.write(), 5).unwrap().unwrap().docker.pos, Pos::new(0, -1, 3));
        assert_eq!(before.len(), record.write().len());
        // Old files count from 8.
        let mut old = sample();
        old.map_docker(3, &|p| p);
        assert_eq!(old.positions(3), (Pos::new(3 + 8, 4 + 8, 5 + 8), Pos::new(8, 7, 8)));
    }

    #[test]
    fn records_that_are_not_a_dock_are_refused() {
        assert!(DockRecord::parse(&[]).is_err());
        assert!(DockRecord::parse(&[0, 0, 13, 0, 0, 3]).is_err());
        // A request to undock: the first member is not a 1 or a 3.
        let mut tag = RawTag::parse(&sample().write()).unwrap();
        tag.value.members_mut().unwrap()[0].value = Value::Byte(2);
        assert!(DockRecord::parse(&tag.write()).is_err());
    }
}
