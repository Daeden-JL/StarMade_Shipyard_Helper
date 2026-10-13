//! One block's stored state.

/// A block as StarMade stores it: 24 bits holding type (11 bits), hit points
/// (7), active flag (1) and orientation (5). The raw value is kept so a design
/// that is loaded and saved without edits comes back bit-identical.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Block(pub u32);

const TYPE_MASK: u32 = 0x7FF;
const HP_SHIFT: u32 = 11;
const HP_MASK: u32 = 0x7F;
const ACTIVE_SHIFT: u32 = 18;
const ORIENT_SHIFT: u32 = 19;
const ORIENT_MASK: u32 = 0x1F;

impl Block {
    pub const AIR: Block = Block(0);
    /// Hit points are stored as a fraction of the block's maximum; 127 is full.
    pub const FULL_HP: u8 = 127;

    pub fn new(ty: u16, orientation: u8, active: bool) -> Self {
        let mut b = Block(0);
        b.set_type(ty);
        b.set_orientation(orientation);
        b.set_active(active);
        b.set_hp(Self::FULL_HP);
        b
    }

    pub fn from_bytes(b: [u8; 3]) -> Self {
        Block(b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32) << 16)
    }

    pub fn to_bytes(self) -> [u8; 3] {
        [self.0 as u8, (self.0 >> 8) as u8, (self.0 >> 16) as u8]
    }

    pub fn ty(self) -> u16 {
        (self.0 & TYPE_MASK) as u16
    }

    pub fn is_air(self) -> bool {
        self.ty() == 0
    }

    pub fn hp(self) -> u8 {
        ((self.0 >> HP_SHIFT) & HP_MASK) as u8
    }

    /// The stored bit. For blocks that can be switched, the game writes 0 for
    /// "on" and 1 for "off"; callers that care should go through the catalog.
    pub fn active_bit(self) -> bool {
        (self.0 >> ACTIVE_SHIFT) & 1 == 1
    }

    pub fn orientation(self) -> u8 {
        ((self.0 >> ORIENT_SHIFT) & ORIENT_MASK) as u8
    }

    pub fn set_type(&mut self, ty: u16) {
        self.0 = (self.0 & !TYPE_MASK) | (ty as u32 & TYPE_MASK);
    }

    pub fn set_hp(&mut self, hp: u8) {
        self.0 = (self.0 & !(HP_MASK << HP_SHIFT)) | ((hp as u32 & HP_MASK) << HP_SHIFT);
    }

    pub fn set_active(&mut self, active: bool) {
        self.0 = (self.0 & !(1 << ACTIVE_SHIFT)) | ((active as u32) << ACTIVE_SHIFT);
    }

    pub fn set_orientation(&mut self, o: u8) {
        self.0 = (self.0 & !(ORIENT_MASK << ORIENT_SHIFT)) | ((o as u32 & ORIENT_MASK) << ORIENT_SHIFT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_pack_into_the_games_bit_layout() {
        let b = Block::new(598, 23, true);
        assert_eq!(b.ty(), 598);
        assert_eq!(b.orientation(), 23);
        assert!(b.active_bit());
        assert_eq!(b.hp(), 127);
        assert_eq!(Block::from_bytes(b.to_bytes()), b);
        // type in the low 11 bits, orientation in the top 5 of 24
        assert_eq!(b.0 & 0x7FF, 598);
        assert_eq!(b.0 >> 19, 23);
    }

    #[test]
    fn setters_leave_other_fields_alone() {
        let mut b = Block::new(2047, 31, true);
        b.set_type(1);
        assert_eq!((b.ty(), b.orientation(), b.active_bit(), b.hp()), (1, 31, true, 127));
        b.set_orientation(0);
        b.set_active(false);
        assert_eq!((b.ty(), b.orientation(), b.active_bit(), b.hp()), (1, 0, false, 127));
    }
}
