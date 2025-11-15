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
// errno module isn't public here; use literal -EFAULT encoding where needed.

/// Linux keyboard ioctl command constants (subset)
pub const KDGKBMODE: u32 = 0x4B44; // Get keyboard mode
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
pub const TCGETS: u32 = 0x5401;    // Get termios
pub const TCSETS: u32 = 0x5402;    // Set termios (no wait)
pub const TCSETSW: u32 = 0x5403;   // Set termios (drain output)
pub const TCSETSF: u32 = 0x5404;   // Set termios (drain and flush)

/// Linux keyboard mode values (subset)
pub const K_RAW: u32 = 0x00;
pub const K_XLATE: u32 = 0x01;

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
    };

    // crate::println!("tty::handle_ioctl: request={:#x}, arg={:#x}", request, arg);

    match request {
        // Minimal termios support: accept on TTY FDs as no-ops
        TCGETS | TCSETS | TCSETSW | TCSETSF => {
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

            match request {
                TCGETS => {
                    // Populate a minimal-but-sane termios structure
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
                    // Reasonable defaults: blocking reads (VMIN=1, VTIME=0), EOF=0x04 (Ctrl-D)
                    t.c_cc[VMIN] = 1;
                    t.c_cc[VTIME] = 0;
                    t.c_cc[VEOF] = 0x04;

                    let task = mytask().ok_or(())?;
                    let vaddr = arg as usize;
                    if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                        unsafe { core::ptr::write(paddr as *mut LinuxTermios, t); }
                        Ok(Some(0))
                    } else {
                        Err(())
                    }
                }
                // TCSETS*, accept as no-op
                _ => Ok(Some(0)),
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

        // Get window size (rows/cols). Return a sensible default 80x25.
        TIOCGWINSZ => {
            let task = mytask().ok_or(())?;
            let vaddr = arg as usize;
            if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                let ws = Winsize { ws_row: 25, ws_col: 80, ws_xpixel: 0, ws_ypixel: 0 };
                unsafe { core::ptr::write(paddr as *mut Winsize, ws); }
                Ok(Some(0))
            } else {
                Ok(Some((-14_isize) as usize))
            }
        }

        // Text/graphics mode stubs: accept and report KD_TEXT as default
        KDGETMODE => {
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
            // Accept KD_TEXT/KD_GRAPHICS, no-op for now
            let mode = arg as u32;
            match mode {
                KD_TEXT | KD_GRAPHICS => Ok(Some(0)),
                _ => Err(()),
            }
        }

        KDGKBMODE | KDSKBMODE => {
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
                // Query canonical mode and map to Linux keyboard mode
                match control_ops.control(SCTL_TTY_GET_CANONICAL, 0) {
                    Ok(val) => {
                        let task = mytask().ok_or(())?;
                        let is_canonical = val != 0;
                        let mode: u32 = if is_canonical { K_XLATE } else { K_RAW };
                        let vaddr = arg as usize;
                        if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                            unsafe { *(paddr as *mut u32) = mode; }
                            Ok(Some(0))
                        } else {
                            Ok(Some((-14_isize) as usize))
                        }
                    }
                    Err(_) => Err(()),
                }
            } else {
                // KDSKBMODE: arg holds the mode value directly
                let mode = arg as u32;
                let enable_canonical = match mode {
                    K_XLATE => true,
                    K_RAW => false,
                    _ => return Err(()),
                };
                let arg_bool = if enable_canonical { 1usize } else { 0usize };
                match control_ops.control(SCTL_TTY_SET_CANONICAL, arg_bool) {
                    Ok(_) => Ok(Some(0)),
                    Err(_) => Err(()),
                }
            }
        }
        // Keyboard map entry get/set (subset)
        0x4B46 /* KDGKBENT */ => {
            // struct kbentry { __u8 kb_table; __u8 kb_index; __u16 kb_value; };
            #[repr(C)]
            struct KbEntry { kb_table: u8, kb_index: u8, kb_value: u16 }

            // Only valid on TTY-capable FDs
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

            // Read user struct, set kb_value=0 as a benign default, and write back
            let task = mytask().ok_or(())?;
            let vaddr = arg as usize;
            if let Some(paddr) = task.vm_manager.translate_vaddr(vaddr) {
                unsafe {
                    let mut e = core::ptr::read(paddr as *const KbEntry);
                    e.kb_value = 0;
                    core::ptr::write(paddr as *mut KbEntry, e);
                }
                Ok(Some(0))
            } else {
                Ok(Some((-14_isize) as usize))
            }
        }
        0x4B47 /* KDSKBENT */ => {
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
