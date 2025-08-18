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

/// Linux keyboard ioctl command constants (subset)
pub const KDGKBMODE: u32 = 0x4B44; // Get keyboard mode
pub const KDSKBMODE: u32 = 0x4B45; // Set keyboard mode

/// Linux keyboard mode values (subset)
pub const K_RAW: u32 = 0x00;
pub const K_XLATE: u32 = 0x01;

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

    match request {
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
                            Err(())
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
        _ => Ok(None),
    }
}
