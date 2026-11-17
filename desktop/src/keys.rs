//! Virtual-key codes that the shortcut hook and the typing of the text share.

/// Shift, Ctrl, Alt, and Win: the generic, left, and right codes.
pub const MODIFIERS: [u16; 11] = [
    0x10, 0x11, 0x12, 0x5B, 0x5C, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5,
];

pub fn is_modifier(vk: u16) -> bool {
    MODIFIERS.contains(&vk)
}

/// The mark on each key event that TDT sends. The keyboard hook passes
/// these events on, so the typed text cannot start a recording. Events from
/// other programs (a remap tool, a remote desktop) have another mark.
pub const OWN_INPUT: usize = 0x5444_5401;
