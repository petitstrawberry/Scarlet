//! X11 KeySym to Scarlet/Linux input keycode conversion.

/// Convert an RFB X11 KeySym into a Scarlet/Linux input keycode.
///
/// Printable symbols map to their physical US-layout key. Modifier state is
/// delivered independently by RFB key events, so shifted symbols intentionally
/// return the same keycode as their unshifted key.
///
/// # Arguments
///
/// * `keysym` - X11 KeySym from an RFB `KeyEvent`.
///
/// # Returns
///
/// A Scarlet/Linux input keycode, or `None` when no stable mapping exists.
pub(super) const fn scarlet_keycode(keysym: u32) -> Option<u16> {
    match keysym {
        0x20 => Some(57),
        0x21 | 0x31 => Some(2),
        0x40 | 0x32 => Some(3),
        0x23 | 0x33 => Some(4),
        0x24 | 0x34 => Some(5),
        0x25 | 0x35 => Some(6),
        0x5e | 0x36 => Some(7),
        0x26 | 0x37 => Some(8),
        0x2a | 0x38 => Some(9),
        0x28 | 0x39 => Some(10),
        0x29 | 0x30 => Some(11),
        0x5f | 0x2d => Some(12),
        0x2b | 0x3d => Some(13),
        0x51 | 0x71 => Some(16),
        0x57 | 0x77 => Some(17),
        0x45 | 0x65 => Some(18),
        0x52 | 0x72 => Some(19),
        0x54 | 0x74 => Some(20),
        0x59 | 0x79 => Some(21),
        0x55 | 0x75 => Some(22),
        0x49 | 0x69 => Some(23),
        0x4f | 0x6f => Some(24),
        0x50 | 0x70 => Some(25),
        0x7b | 0x5b => Some(26),
        0x7d | 0x5d => Some(27),
        0x41 | 0x61 => Some(30),
        0x53 | 0x73 => Some(31),
        0x44 | 0x64 => Some(32),
        0x46 | 0x66 => Some(33),
        0x47 | 0x67 => Some(34),
        0x48 | 0x68 => Some(35),
        0x4a | 0x6a => Some(36),
        0x4b | 0x6b => Some(37),
        0x4c | 0x6c => Some(38),
        0x3a | 0x3b => Some(39),
        0x22 | 0x27 => Some(40),
        0x7e | 0x60 => Some(41),
        0x7c | 0x5c => Some(43),
        0x5a | 0x7a => Some(44),
        0x58 | 0x78 => Some(45),
        0x43 | 0x63 => Some(46),
        0x56 | 0x76 => Some(47),
        0x42 | 0x62 => Some(48),
        0x4e | 0x6e => Some(49),
        0x4d | 0x6d => Some(50),
        0x3c | 0x2c => Some(51),
        0x3e | 0x2e => Some(52),
        0x3f | 0x2f => Some(53),

        0xff1b => Some(1),            // Escape
        0xff08 => Some(14),           // BackSpace
        0xff09 | 0xfe20 => Some(15),  // Tab / ISO_Left_Tab
        0xff0d => Some(28),           // Return
        0xff13 | 0xff6b => Some(119), // Pause / Break
        0xff14 => Some(70),           // Scroll Lock
        0xff50 | 0xff95 => Some(102), // Home
        0xff51 | 0xff96 => Some(105), // Left
        0xff52 | 0xff97 => Some(103), // Up
        0xff53 | 0xff98 => Some(106), // Right
        0xff54 | 0xff99 => Some(108), // Down
        0xff55 | 0xff9a => Some(104), // Page Up
        0xff56 | 0xff9b => Some(109), // Page Down
        0xff57 | 0xff9c => Some(107), // End
        0xff61 => Some(99),           // Print
        0xff63 | 0xff9e => Some(110), // Insert
        0xffff | 0xff9f => Some(111), // Delete
        0xff67 => Some(127),          // Menu
        0xff6a => Some(138),          // Help
        0xff7f => Some(69),           // Num Lock

        0xffbe => Some(59),
        0xffbf => Some(60),
        0xffc0 => Some(61),
        0xffc1 => Some(62),
        0xffc2 => Some(63),
        0xffc3 => Some(64),
        0xffc4 => Some(65),
        0xffc5 => Some(66),
        0xffc6 => Some(67),
        0xffc7 => Some(68),
        0xffc8 => Some(87),
        0xffc9 => Some(88),

        0xffe1 => Some(42),
        0xffe2 => Some(54),
        0xffe3 => Some(29),
        0xffe4 => Some(97),
        0xffe5 => Some(58),
        0xffe7 | 0xffe9 => Some(56),
        0xffe8 | 0xffea => Some(100),
        0xffeb => Some(125),
        0xffec => Some(126),

        0xff8d => Some(96),
        0xffaa => Some(55),
        0xffab => Some(78),
        0xffad => Some(74),
        0xffae => Some(83),
        0xffaf => Some(98),
        0xffb0 => Some(82),
        0xffb1 => Some(79),
        0xffb2 => Some(80),
        0xffb3 => Some(81),
        0xffb4 => Some(75),
        0xffb5 => Some(76),
        0xffb6 => Some(77),
        0xffb7 => Some(71),
        0xffb8 => Some(72),
        0xffb9 => Some(73),

        0xff22 => Some(94),  // Muhenkan
        0xff23 => Some(92),  // Henkan Mode
        0xff2a => Some(85),  // Zenkaku/Hankaku
        0xff31 => Some(122), // Hangul
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::scarlet_keycode;

    #[test]
    fn printable_symbols_share_physical_keys() {
        assert_eq!(scarlet_keycode('a' as u32), Some(30));
        assert_eq!(scarlet_keycode('A' as u32), Some(30));
        assert_eq!(scarlet_keycode('1' as u32), Some(2));
        assert_eq!(scarlet_keycode('!' as u32), Some(2));
    }

    #[test]
    fn navigation_and_modifiers_map_to_linux_codes() {
        assert_eq!(scarlet_keycode(0xff51), Some(105));
        assert_eq!(scarlet_keycode(0xffe1), Some(42));
        assert_eq!(scarlet_keycode(0xffe4), Some(97));
    }
}
