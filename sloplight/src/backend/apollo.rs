//! Apollo permission bitmask, mirroring `app/backend/apollopermissions.h`.
#![allow(dead_code)]

pub const INPUT_CONTROLLER: u32 = 0x0000_0100;
pub const INPUT_TOUCH: u32 = 0x0000_0200;
pub const INPUT_PEN: u32 = 0x0000_0400;
pub const INPUT_MOUSE: u32 = 0x0000_0800;
pub const INPUT_KBD: u32 = 0x0000_1000;
pub const CLIPBOARD_SET: u32 = 0x0001_0000;
pub const CLIPBOARD_READ: u32 = 0x0002_0000;
pub const SERVER_CMD: u32 = 0x0010_0000;
pub const LIST: u32 = 0x0100_0000;
pub const VIEW: u32 = 0x0200_0000;
pub const LAUNCH: u32 = 0x0400_0000;

pub fn has(mask: u32, bit: u32) -> bool {
    mask & bit == bit
}

/// Human-readable permission names, in display order.
pub fn describe(mask: u32) -> Vec<(&'static str, bool)> {
    [
        ("List apps", LIST),
        ("View stream", VIEW),
        ("Launch apps", LAUNCH),
        ("Keyboard", INPUT_KBD),
        ("Mouse", INPUT_MOUSE),
        ("Controller", INPUT_CONTROLLER),
        ("Touch", INPUT_TOUCH),
        ("Pen", INPUT_PEN),
        ("Set host clipboard", CLIPBOARD_SET),
        ("Read host clipboard", CLIPBOARD_READ),
        ("Server commands", SERVER_CMD),
    ]
    .into_iter()
    .map(|(name, bit)| (name, has(mask, bit)))
    .collect()
}
