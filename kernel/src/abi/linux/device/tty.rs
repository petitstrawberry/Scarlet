//! Linux-specific ioctl translation for TTY-like devices.
//!
//! This module maps Linux ioctls (e.g., termios/keyboard subset) onto Scarlet
//! TTY control ops exposed via ControlOps on Device-backed file objects.

use crate::{
    device::{manager::DeviceManager, DeviceCapability},
    fs::FileType,
    object::KernelObject,
    task::mytask,
};
use crate::device::char::tty::tty_ctl::{
    SCTL_TTY_GET_KBMODE,
    SCTL_TTY_GET_WINSIZE,
    SCTL_TTY_SET_KBMODE,
    SCTL_TTY_SET_WINSIZE,
};
// errno module isn't public here; use literal -EFAULT encoding where needed.

/// Linux keyboard ioctl command constants (subset)
pub const KDGKBMODE: u32 = 0x4B44; // Get keyboard mode
pub const KDGKBTYPE: u32 = 0x4B33; // Get keyboard type
pub const KB_101: u32 = 0x02;      // English 101/102-key keyboard
pub const KDSKBMODE: u32 = 0x4B45; // Set keyboard mode
/// Linux VT (virtual terminal) ioctl command constants (subset)
pub const VT_OPENQRY: u32 = 0x5600; // Find available VT number
pub const VT_GETMODE: u32 = 0x5601; // Get VT mode
pub const VT_SETMODE: u32 = 0x5602; // Set VT mode
pub const VT_GETSTATE: u32 = 0x5603; // Get VT state
pub const VT_ACTIVATE: u32 = 0x5606; // Make VT active
pub const VT_WAITACTIVE: u32 = 0x560C; // Wait until VT is active

/// Termios/winsize ioctl constants (subset)
pub const TIOCGWINSZ: u32 = 0x5413; // Get window size
pub const TIOCSWINSZ: u32 = 0x5414; // Set window size
pub const TCGETS: u32 = 0x5401;    // Get termios
pub const TCSETS: u32 = 0x5402;    // Set termios (no wait)
pub const TCSETSW: u32 = 0x5403;   // Set termios (drain output)
pub const TCSETSF: u32 = 0x5404;   // Set termios (drain and flush)

/// Linux keyboard mode values (subset)
pub const K_RAW: u32 = 0x00;
pub const K_XLATE: u32 = 0x01;
// Additional keyboard modes sometimes requested by SDL (for example,
// SDL 1.2 when running on fbcon). Treat these permissively as
// raw-equivalent modes for compatibility.
pub const K_MEDIUMRAW: u32 = 0x02;
pub const K_UNICODE: u32 = 0x03;
pub const K_OFF: u32 = 0x04;

/// KD text/graphics mode (subset)
pub const KDSETMODE: u32 = 0x4B3A; // Set text/graphics mode
pub const KDGETMODE: u32 = 0x4B3B; // Get text/graphics mode
pub const KD_TEXT: u32 = 0x00;
pub const KD_GRAPHICS: u32 = 0x01;

#[repr(C)]
struct Winsize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

/// Minimal Linux termios (asm-generic) layout for TCGETS.
/// This mirrors asm-generic: 4x tcflag_t (u32), 1x cc line (u8),
/// c_cc[19] (u8), and ispeed/ospeed (u32 each).
#[repr(C)]
struct LinuxTermios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 19],
    c_ispeed: u32,
    c_ospeed: u32,
}

// Common termios cc index constants (asm-generic)
const VEOF: usize = 4;
const VTIME: usize = 5;
const VMIN: usize = 6;

/// Handle Linux TTY-related ioctls for a given kernel object representing an
/// open file descriptor. Returns Ok(Some(ret)) if handled, Ok(None) if not
/// applicable, and Err(()) on error (mapped to -1 by caller).
pub fn handle_ioctl(
    request: u32,
    arg: usize,
    kernel_object: &KernelObject,
) -> Result<Option<usize>, ()> {
    use crate::device::char::tty::tty_ctl::{
        SCTL_TTY_GET_CANONICAL, SCTL_TTY_SET_CANONICAL,
        SCTL_TTY_GET_READ_POLICY, SCTL_TTY_SET_READ_POLICY,
        SCTL_TTY_GET_ECHO, SCTL_TTY_SET_ECHO,
    };

    const LOG_TTY_IOCTL: bool = false;
    match request {
        KDGKBTYPE => {
            // Always return KB_101
            let task = mytask().ok_or(())?;
            let vaddr = arg as usize;
            if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                unsafe { *(paddr as *mut u32) = KB_101; }
                Ok(Some(0))
            } else {
                Ok(Some((-14_isize) as usize))
            }
        }
        // Minimal termios support with Scarlet TTY mapping (canonical + read policy)
        TCGETS | TCSETS | TCSETSW | TCSETSF => {
            if LOG_TTY_IOCTL { crate::println!("[tty_ioctl] termios op {:#06x}", request); }
            // Validate the FD is a char device with TTY capability
            let is_tty = if let Some(file_obj) = kernel_object.as_file() {
                if let Ok(metadata) = file_obj.metadata() {
                    if let FileType::CharDevice(info) = metadata.file_type {
                        if let Some(dev) = DeviceManager::get_manager().get_device(info.device_id) {
                            dev.capabilities().iter().any(|c| *c == DeviceCapability::Tty)
                        } else { false }
                    } else { false }
                } else { false }
            } else { false };
            if !is_tty { return Err(()); }

            match request {
                TCGETS => {
                    let mut t = LinuxTermios {
                        c_iflag: 0,
                        c_oflag: 0,
                        c_cflag: 0,
                        c_lflag: 0,
                        c_line: 0,
                        c_cc: [0; 19],
                        c_ispeed: 0,
                        c_ospeed: 0,
                    };
                    let mut canonical = false;
                    let mut echo = true;
                    let mut min_ready: u16 = 1;
                    let mut timeout_ms: u16 = 0;
                    if let Some(control_ops) = kernel_object.as_control() {
                        if let Ok(val) = control_ops.control(SCTL_TTY_GET_CANONICAL, 0) { canonical = val != 0; }
                        if let Ok(val) = control_ops.control(SCTL_TTY_GET_ECHO, 0) { echo = val != 0; }
                        if let Ok(packed) = control_ops.control(SCTL_TTY_GET_READ_POLICY, 0) {
                            let packed_u = packed as u32;
                            min_ready = (packed_u & 0xFFFF) as u16;
                            timeout_ms = ((packed_u >> 16) & 0xFFFF) as u16;
                        }
                    }
                    if canonical { t.c_lflag |= 0x0000_0002; }
                    if echo { t.c_lflag |= 0x0000_0008; }
                    t.c_cc[VMIN] = core::cmp::min(min_ready as usize, 255) as u8;
                    let vtime_tenths = core::cmp::min(((timeout_ms as u32 + 99) / 100) as usize, 255) as u8;
                    t.c_cc[VTIME] = vtime_tenths;
                    t.c_cc[VEOF] = 0x04; // Ctrl-D
                    let task = mytask().ok_or(())?;
                    let vaddr = arg as usize;
                    if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                        unsafe { core::ptr::write(paddr as *mut LinuxTermios, t); }
                        Ok(Some(0))
                    } else { Err(()) }
                }
                _ => {
                    let task = mytask().ok_or(())?;
                    let vaddr = arg as usize;
                    if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                        let t = unsafe { core::ptr::read(paddr as *const LinuxTermios) };
                        let canonical_new = (t.c_lflag & 0x0000_0002) != 0;
                        let echo_new = (t.c_lflag & 0x0000_0008) != 0;
                        let vmin = t.c_cc[VMIN] as u16;
                        let vtime_tenths = t.c_cc[VTIME] as u16;
                        let timeout_ms: u16 = vtime_tenths.saturating_mul(100);
                        if let Some(control_ops) = kernel_object.as_control() {
                            let prev_canonical = control_ops.control(SCTL_TTY_GET_CANONICAL, 0).unwrap_or(-1) != 0;
                            let prev_echo = control_ops.control(SCTL_TTY_GET_ECHO, 0).unwrap_or(-1) != 0;
                            let _ = control_ops.control(SCTL_TTY_SET_CANONICAL, if canonical_new { 1 } else { 0 });
                            let _ = control_ops.control(SCTL_TTY_SET_ECHO, if echo_new { 1 } else { 0 });
                            let packed = ((timeout_ms as u32) << 16) | (vmin as u32);
                            let _ = control_ops.control(SCTL_TTY_SET_READ_POLICY, packed as usize);
                            if LOG_TTY_IOCTL && (prev_canonical != canonical_new || prev_echo != echo_new) {
                                crate::println!("[linux TTY] termios change: canonical={} echo={} vmin={} timeout_ms={}",
                                    canonical_new, echo_new, vmin, timeout_ms);
                            }
                        }
                        Ok(Some(0))
                    } else { Err(()) }
                }
            }
        }
        // Virtual terminal: get current VT state
        VT_GETSTATE => {
            // Structure per linux/vt.h
            #[repr(C)]
            struct VtStat { v_active: u16, v_signal: u16, v_state: u16 }
            let task = mytask().ok_or(())?;
            let vaddr = arg as usize;
            if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                let state = VtStat { v_active: 1, v_signal: 0, v_state: 1 << 1 };
                unsafe { core::ptr::write(paddr as *mut VtStat, state); }
                Ok(Some(0))
            } else {
                Ok(Some((-14_isize) as usize))
            }
        }
        // Virtual terminal: return a plausible available VT number (e.g., 1)
        VT_OPENQRY => {
            // Must be a TTY-capable device
            let is_tty = if let Some(file_obj) = kernel_object.as_file() {
                if let Ok(metadata) = file_obj.metadata() {
                    if let FileType::CharDevice(info) = metadata.file_type {
                        if let Some(dev) = DeviceManager::get_manager().get_device(info.device_id)
                        {
                            dev.capabilities()
                                .iter()
                                .any(|c| *c == DeviceCapability::Tty)
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if !is_tty {
                return Err(());
            }

            // Write VT number into user pointer
            let task = mytask().ok_or(())?;
            let vaddr = arg as usize;
            if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                unsafe { *(paddr as *mut i32) = 1; }
                Ok(Some(0))
            } else {
                Ok(Some((-14_isize) as usize))
            }
        }

        // Virtual terminal: get mode (stub values)
        VT_GETMODE => {
            // Validate TTY FD
            let is_tty = if let Some(file_obj) = kernel_object.as_file() {
                if let Ok(metadata) = file_obj.metadata() {
                    if let FileType::CharDevice(info) = metadata.file_type {
                        if let Some(dev) = DeviceManager::get_manager().get_device(info.device_id) {
                            dev.capabilities().iter().any(|c| *c == DeviceCapability::Tty)
                        } else { false }
                    } else { false }
                } else { false }
            } else { false };
            if !is_tty { return Err(()); }

            // Linux struct vt_mode has several fields; we write zeros as a benign default
            let task = mytask().ok_or(())?;
            let vaddr = arg as usize;
            if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                // Write 8 bytes of zeros (enough for minimal fields on common ABIs)
                unsafe { core::ptr::write_bytes(paddr as *mut u8, 0, 8); }
                Ok(Some(0))
            } else {
                Ok(Some((-14_isize) as usize))
            }
        }

        // Virtual terminal: set mode (accept as no-op)
        VT_SETMODE => {
            // Validate TTY FD
            let is_tty = if let Some(file_obj) = kernel_object.as_file() {
                if let Ok(metadata) = file_obj.metadata() {
                    if let FileType::CharDevice(info) = metadata.file_type {
                        if let Some(dev) = DeviceManager::get_manager().get_device(info.device_id) {
                            dev.capabilities().iter().any(|c| *c == DeviceCapability::Tty)
                        } else { false }
                    } else { false }
                } else { false }
            } else { false };
            if !is_tty { return Err(()); }
            Ok(Some(0))
        }

        // Activate a VT number (stub: accept VT 1)
        VT_ACTIVATE => {
            // Validate TTY FD
            let is_tty = if let Some(file_obj) = kernel_object.as_file() {
                if let Ok(metadata) = file_obj.metadata() {
                    if let FileType::CharDevice(info) = metadata.file_type {
                        if let Some(dev) = DeviceManager::get_manager().get_device(info.device_id) {
                            dev.capabilities().iter().any(|c| *c == DeviceCapability::Tty)
                        } else { false }
                    } else { false }
                } else { false }
            } else { false };
            if !is_tty { return Err(()); }

            // Linux expects arg to be VT number; we accept 1..=1 for now
            let vtno = arg as u32;
            if vtno == 1 { Ok(Some(0)) } else { Ok(Some(0)) } // be permissive
        }

        // Wait until the specified VT becomes active (stub: succeed immediately)
        0x5607 => { // VT_WAITACTIVE
            // Validate TTY FD
            let is_tty = if let Some(file_obj) = kernel_object.as_file() {
                if let Ok(metadata) = file_obj.metadata() {
                    if let FileType::CharDevice(info) = metadata.file_type {
                        if let Some(dev) = DeviceManager::get_manager().get_device(info.device_id) {
                            dev.capabilities().iter().any(|c| *c == DeviceCapability::Tty)
                        } else { false }
                    } else { false }
                } else { false }
            } else { false };
            if !is_tty { return Err(()); }

            Ok(Some(0))
        }

        // Unlock VT switching (stub: succeed)
        0x560C => { // VT_UNLOCKSWITCH
            let is_tty = if let Some(file_obj) = kernel_object.as_file() {
                if let Ok(metadata) = file_obj.metadata() {
                    if let FileType::CharDevice(info) = metadata.file_type {
                        if let Some(dev) = DeviceManager::get_manager().get_device(info.device_id) {
                            dev.capabilities().iter().any(|c| *c == DeviceCapability::Tty)
                        } else { false }
                    } else { false }
                } else { false }
            } else { false };
            if !is_tty { return Err(()); }
            Ok(Some(0))
        }

        // Get window size (rows/cols). Fall back to 80x25 if unavailable.
        TIOCGWINSZ => {
            if LOG_TTY_IOCTL { crate::println!("[tty_ioctl] TIOCGWINSZ"); }
            if !is_tty_kernel_object(kernel_object) { return Err(()); }

            let task = mytask().ok_or(())?;
            let vaddr = arg as usize;
            if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                let (cols, rows) = if let Some(control_ops) = kernel_object.as_control() {
                    match control_ops.control(SCTL_TTY_GET_WINSIZE, 0) {
                        Ok(packed) => {
                            let packed_u = packed as u32;
                            let cols = (packed_u >> 16) as u16;
                            let rows = (packed_u & 0xFFFF) as u16;
                            (cols, rows)
                        }
                        Err(_) => (80, 25),
                    }
                } else {
                    (80, 25)
                };

                let ws = Winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
                unsafe { core::ptr::write(paddr as *mut Winsize, ws); }
                Ok(Some(0))
            } else {
                Ok(Some((-14_isize) as usize))
            }
        }

        // Set window size from Linux winsize structure.
        TIOCSWINSZ => {
            if LOG_TTY_IOCTL { crate::println!("[tty_ioctl] TIOCSWINSZ"); }
            if !is_tty_kernel_object(kernel_object) { return Err(()); }

            let task = mytask().ok_or(())?;
            let vaddr = arg as usize;
            if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                let ws = unsafe { core::ptr::read(paddr as *const Winsize) };
                if let Some(control_ops) = kernel_object.as_control() {
                    let packed = ((ws.ws_col as usize) << 16) | (ws.ws_row as usize);
                    control_ops.control(SCTL_TTY_SET_WINSIZE, packed).map_err(|_| ())?;
                } else {
                    return Err(());
                }
                Ok(Some(0))
            } else {
                Ok(Some((-14_isize) as usize))
            }
        }

        // Text/graphics mode stubs: accept and report KD_TEXT as default
        KDGETMODE => {
            if LOG_TTY_IOCTL { crate::println!("[tty_ioctl] KDGETMODE"); }
            let task = mytask().ok_or(())?;
            let vaddr = arg as usize;
            if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                unsafe { *(paddr as *mut u32) = KD_TEXT; }
                Ok(Some(0))
            } else {
                Ok(Some((-14_isize) as usize))
            }
        }
        KDSETMODE => {
            if LOG_TTY_IOCTL { crate::println!("[tty_ioctl] KDSETMODE mode={:#x}", arg as u32); }
            // Accept KD_TEXT/KD_GRAPHICS, no-op for now
            let mode = arg as u32;
            match mode {
                KD_TEXT | KD_GRAPHICS => Ok(Some(0)),
                _ => Err(()),
            }
        }

        KDGKBMODE | KDSKBMODE => {
            if LOG_TTY_IOCTL {
                if request == KDGKBMODE { crate::println!("[tty_ioctl] KDGKBMODE"); } else { crate::println!("[tty_ioctl] KDSKBMODE -> {:#x}", arg as u32); }
            }
            // Validate the FD is a char device with TTY capability
            let is_tty = if let Some(file_obj) = kernel_object.as_file() {
                if let Ok(metadata) = file_obj.metadata() {
                    if let FileType::CharDevice(info) = metadata.file_type {
                        if let Some(dev) = DeviceManager::get_manager().get_device(info.device_id)
                        {
                            dev.capabilities()
                                .iter()
                                .any(|c| *c == DeviceCapability::Tty)
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if !is_tty {
                return Err(());
            }

            // Must have ControlOps capability to talk to device
            let control_ops = kernel_object.as_control().ok_or(())?;

            if request == KDGKBMODE {
                // Query Scarlet kb_mode and map to Linux value
                match control_ops.control(SCTL_TTY_GET_KBMODE, 0) {
                    Ok(v) => {
                        let task = mytask().ok_or(())?;
                        let mode: u32 = match v as u32 { 0 => K_XLATE, 1 => K_MEDIUMRAW, 2 => K_RAW, _ => K_RAW };
                        let vaddr = arg as usize;
                        if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                            unsafe { *(paddr as *mut u32) = mode; }
                            Ok(Some(0))
                        } else { Ok(Some((-14_isize) as usize)) }
                    }
                    Err(_) => Err(()),
                }
            } else {
                // KDSKBMODE: arg holds the mode value directly
                let mode = arg as u32;
                // Be permissive: treat any non-XLATE mode as non-canonical ("raw-ish").
                // This covers K_RAW, K_MEDIUMRAW, K_UNICODE, K_OFF commonly used by SDL.
                let enable_canonical = match mode {
                    K_XLATE => true,
                    K_RAW | K_MEDIUMRAW | K_UNICODE | K_OFF => false,
                    _ => false, // Unknown modes: accept but consider non-canonical
                };
                let arg_bool = if enable_canonical { 1usize } else { 0usize };
                let prev_canonical = control_ops.control(SCTL_TTY_GET_CANONICAL, 0).unwrap_or(-1) != 0;
                match control_ops.control(SCTL_TTY_SET_CANONICAL, arg_bool) {
                    Ok(_) => {
                        let new_canonical = enable_canonical;
                        if LOG_TTY_IOCTL && prev_canonical != new_canonical {
                            crate::println!("[linux TTY] keyboard mode -> canonical={}", new_canonical);
                        }
                        // Also set Scarlet kb_mode for read() semantics
                        let kb = match mode { K_XLATE => 0usize, K_MEDIUMRAW => 1usize, K_RAW | K_UNICODE | K_OFF => 2usize, _ => 2usize };
                        let _ = control_ops.control(SCTL_TTY_SET_KBMODE, kb);
                        Ok(Some(0))
                    },
                    Err(_) => Err(()),
                }
            }
        }
        // Keyboard map entry get/set (subset)
        0x4B46 /* KDGKBENT */ => {
            #[repr(C)]
            struct KbEntry { kb_table: u8, kb_index: u8, kb_value: u16 }

            if !is_tty_kernel_object(kernel_object) { return Err(()); }

            fn us_kmap(norm: bool, idx: usize) -> u16 {
                // Linux console keymap encoding: K(t,v) = ((t<<8)|v)
                const KT_FN: u16 = 1;
                const KT_CUR: u16 = 6;
                // Cursor keys
                const K_DOWN: u16 = (KT_CUR << 8) | 0; // 0x0600
                const K_LEFT: u16 = (KT_CUR << 8) | 1; // 0x0601
                const K_RIGHT: u16 = (KT_CUR << 8) | 2; // 0x0602
                const K_UP: u16 = (KT_CUR << 8) | 3; // 0x0603
                // Function-like navigation
                const K_FIND: u16 = (KT_FN << 8) | 20;   // Home
                const K_INSERT: u16 = (KT_FN << 8) | 21; // Insert
                const K_REMOVE: u16 = (KT_FN << 8) | 22; // Delete
                const K_SELECT: u16 = (KT_FN << 8) | 23; // End
                const K_PGUP: u16 = (KT_FN << 8) | 24;   // PageUp
                const K_PGDN: u16 = (KT_FN << 8) | 25;   // PageDown
                let ascii_passthrough = |i: usize| -> u16 {
                    if (b'a' as usize..=b'z' as usize).contains(&i)
                        || (b'0' as usize..=b'9' as usize).contains(&i)
                        || i == b' ' as usize || i == b'\t' as usize || i == b'\n' as usize
                        || [b'-', b'=', b'[', b']', b'\\', b';', b'\'', b'`', b',', b'.', b'/'].iter().any(|c| *c as usize == i)
                    { i as u16 } else { 0 }
                };
                let ascii_shift = |i: usize| -> u16 {
                    match i as u8 {
                        b'a'..=b'z' => (i as u8).to_ascii_uppercase() as u16,
                        b'1' => b'!' as u16, b'2' => b'@' as u16, b'3' => b'#' as u16,
                        b'4' => b'$' as u16, b'5' => b'%' as u16, b'6' => b'^' as u16,
                        b'7' => b'&' as u16, b'8' => b'*' as u16, b'9' => b'(' as u16,
                        b'0' => b')' as u16,
                        b'-' => b'_' as u16, b'=' => b'+' as u16,
                        b'[' => b'{' as u16, b']' => b'}' as u16, b'\\' => b'|' as u16,
                        b';' => b':' as u16, b'\'' => b'"' as u16, b'`' => b'~' as u16,
                        b',' => b'<' as u16, b'.' => b'>' as u16, b'/' => b'?' as u16,
                        other => other as u16,
                    }
                };
                match (norm, idx) {
                    // Special keys by Linux keycode index (from input-event-codes):
                    // 102=HOME, 103=UP, 104=PGUP, 105=LEFT, 106=RIGHT, 107=END, 108=DOWN, 109=PGDN, 110=INSERT, 111=DELETE
                    // Map to Linux console keysyms so SDL can resolve keysym.
                    (_, 103) => K_UP,
                    (_, 108) => K_DOWN,
                    (_, 105) => K_LEFT,
                    (_, 106) => K_RIGHT,
                    (_, 102) => K_FIND,   // Home
                    (_, 107) => K_SELECT, // End
                    (_, 104) => K_PGUP,
                    (_, 109) => K_PGDN,
                    (_, 110) => K_INSERT,
                    (_, 111) => K_REMOVE,
                    (true, 2) => b'1' as u16, (true, 3) => b'2' as u16, (true, 4) => b'3' as u16,
                    (true, 5) => b'4' as u16, (true, 6) => b'5' as u16, (true, 7) => b'6' as u16,
                    (true, 8) => b'7' as u16, (true, 9) => b'8' as u16, (true, 10) => b'9' as u16,
                    (true, 11) => b'0' as u16, (true, 12) => b'-' as u16, (true, 13) => b'=' as u16,
                    (true, 15) => b'\t' as u16, (true, 28) => b'\n' as u16, (true, 57) => b' ' as u16,
                    (true, 16) => b'q' as u16, (true, 17) => b'w' as u16, (true, 18) => b'e' as u16,
                    (true, 19) => b'r' as u16, (true, 20) => b't' as u16, (true, 21) => b'y' as u16,
                    (true, 22) => b'u' as u16, (true, 23) => b'i' as u16, (true, 24) => b'o' as u16,
                    (true, 25) => b'p' as u16, (true, 26) => b'[' as u16, (true, 27) => b']' as u16,
                    (true, 30) => b'a' as u16, (true, 31) => b's' as u16, (true, 32) => b'd' as u16,
                    (true, 33) => b'f' as u16, (true, 34) => b'g' as u16, (true, 35) => b'h' as u16,
                    (true, 36) => b'j' as u16, (true, 37) => b'k' as u16, (true, 38) => b'l' as u16,
                    (true, 39) => b';' as u16, (true, 40) => b'\'' as u16, (true, 41) => b'`' as u16,
                    (true, 44) => b'z' as u16, (true, 45) => b'x' as u16, (true, 46) => b'c' as u16,
                    (true, 47) => b'v' as u16, (true, 48) => b'b' as u16, (true, 49) => b'n' as u16,
                    (true, 50) => b'm' as u16, (true, 51) => b',' as u16, (true, 52) => b'.' as u16,
                    (true, 53) => b'/' as u16,
                    (false, 2) => b'!' as u16, (false, 3) => b'@' as u16, (false, 4) => b'#' as u16,
                    (false, 5) => b'$' as u16, (false, 6) => b'%' as u16, (false, 7) => b'^' as u16,
                    (false, 8) => b'&' as u16, (false, 9) => b'*' as u16, (false, 10) => b'(' as u16,
                    (false, 11) => b')' as u16, (false, 12) => b'_' as u16, (false, 13) => b'+' as u16,
                    (false, 15) => b'\t' as u16, (false, 28) => b'\n' as u16, (false, 57) => b' ' as u16,
                    (false, 16) => b'Q' as u16, (false, 17) => b'W' as u16, (false, 18) => b'E' as u16,
                    (false, 19) => b'R' as u16, (false, 20) => b'T' as u16, (false, 21) => b'Y' as u16,
                    (false, 22) => b'U' as u16, (false, 23) => b'I' as u16, (false, 24) => b'O' as u16,
                    (false, 25) => b'P' as u16, (false, 26) => b'{' as u16, (false, 27) => b'}' as u16,
                    (false, 30) => b'A' as u16, (false, 31) => b'S' as u16, (false, 32) => b'D' as u16,
                    (false, 33) => b'F' as u16, (false, 34) => b'G' as u16, (false, 35) => b'H' as u16,
                    (false, 36) => b'J' as u16, (false, 37) => b'K' as u16, (false, 38) => b'L' as u16,
                    (false, 39) => b':' as u16, (false, 40) => b'"' as u16, (false, 41) => b'~' as u16,
                    (false, 44) => b'Z' as u16, (false, 45) => b'X' as u16, (false, 46) => b'C' as u16,
                    (false, 47) => b'V' as u16, (false, 48) => b'B' as u16, (false, 49) => b'N' as u16,
                    (false, 50) => b'M' as u16, (false, 51) => b'<' as u16, (false, 52) => b'>' as u16,
                    (false, 53) => b'?' as u16,
                    _ => {
                        // Fallback: if SDL is passing ASCII in scancode, map it directly
                        if norm { ascii_passthrough(idx) } else { ascii_shift(idx) }
                    },
                }
            }

            let task = mytask().ok_or(())?;
            let vaddr = arg as usize;
            if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                unsafe {
                    let mut e = core::ptr::read(paddr as *const KbEntry);
                    let idx = e.kb_index as usize;
                    let val = match e.kb_table { 0 => us_kmap(true, idx), 1 => us_kmap(false, idx), _ => 0 };
                    e.kb_value = val;
                    core::ptr::write(paddr as *mut KbEntry, e);
                }
                Ok(Some(0))
            } else {
                Ok(Some((-14_isize) as usize))
            }
        }
        0x4B47 /* KDSKBENT */ => {
            crate::println!("[tty_ioctl] KDSKBENT");
            // Accept and ignore (no-op)
            let is_tty = if let Some(file_obj) = kernel_object.as_file() {
                if let Ok(metadata) = file_obj.metadata() {
                    if let FileType::CharDevice(info) = metadata.file_type {
                        if let Some(dev) = DeviceManager::get_manager().get_device(info.device_id) {
                            dev.capabilities().iter().any(|c| *c == DeviceCapability::Tty)
                        } else { false }
                    } else { false }
                } else { false }
            } else { false };
            if !is_tty { return Err(()); }
            Ok(Some(0))
        }
        _ => Ok(None),
    }
}

fn is_tty_kernel_object(kernel_object: &KernelObject) -> bool {
    if let Some(file_obj) = kernel_object.as_file() {
        if let Ok(metadata) = file_obj.metadata() {
            if let FileType::CharDevice(info) = metadata.file_type {
                if let Some(dev) = DeviceManager::get_manager().get_device(info.device_id) {
                    return dev.capabilities().iter().any(|c| *c == DeviceCapability::Tty);
                }
            }
        }
    }
    false
}
