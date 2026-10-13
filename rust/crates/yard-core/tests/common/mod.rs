//! The hashes `tools/game-verify` reports for what the game read, computed the
//! same way from what this crate read. Shared by the tests and the `bphash`
//! example.

use sha2::{Digest, Sha256};
use yard_core::entity::CORE_OFFSET;
use yard_core::{Entity, Pos};

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// File-space coordinates, as the verifier reports them.
fn file_pos(p: Pos) -> [i32; 3] {
    [p.x + CORE_OFFSET, p.y + CORE_OFFSET, p.z + CORE_OFFSET]
}

/// The verifier's `contentHash`, `rawHash` and `linksHash` for an entity.
pub fn hashes(entity: &Entity) -> (String, String, String) {
    let mut blocks = Vec::new();
    entity.for_each(|p, b| blocks.push((file_pos(p), b)));
    blocks.sort_by_key(|(p, _)| *p);
    let (mut content, mut raw) = (Sha256::new(), Sha256::new());
    for (p, b) in &blocks {
        for c in p {
            content.update(c.to_be_bytes());
            raw.update(c.to_be_bytes());
        }
        content.update(b.ty().to_be_bytes());
        content.update([b.orientation(), b.active_bit() as u8, b.hp()]);
        raw.update(b.to_bytes());
    }
    let mut records: Vec<([i32; 3], u16, [i32; 3])> = Vec::new();
    for (controller, groups) in entity.links() {
        for (ty, targets) in groups {
            for target in targets {
                records.push((file_pos(*controller), *ty, file_pos(*target)));
            }
        }
    }
    records.sort();
    let mut links = Sha256::new();
    for (c, ty, t) in records {
        c.iter().for_each(|v| links.update(v.to_be_bytes()));
        links.update(ty.to_be_bytes());
        t.iter().for_each(|v| links.update(v.to_be_bytes()));
    }
    (hex(content.finalize()), hex(raw.finalize()), hex(links.finalize()))
}

