use crate::{abi::linux::LinuxAbi, arch::Trapframe, task::mytask};

/// Linux utsname structure for uname system call
/// This structure must match Linux's struct utsname layout
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UtsName {
    /// System name (e.g., "Linux")
    pub sysname: [u8; 65],
    /// Node name (hostname)
    pub nodename: [u8; 65],
    /// Release (kernel version)
    pub release: [u8; 65],
    /// Version (kernel build info)
    pub version: [u8; 65],
    /// Machine (hardware architecture)
    pub machine: [u8; 65],
    /// Domain name (GNU extension)
    pub domainname: [u8; 65],
}

impl UtsName {
    /// Create a new UtsName with Scarlet system information
    pub fn new() -> Self {
        let mut uts = UtsName {
            sysname: [0; 65],
            nodename: [0; 65],
            release: [0; 65],
            version: [0; 65],
            machine: [0; 65],
            domainname: [0; 65],
        };

        // System name - identify as Linux for compatibility
        let sysname = b"Linux";
        uts.sysname[..sysname.len()].copy_from_slice(sysname);

        // Node name (hostname)
        let nodename = b"scarlet";
        uts.nodename[..nodename.len()].copy_from_slice(nodename);

        // Release (kernel version)
        let release = b"6.1.0-scarlet_linux_abi_module";
        uts.release[..release.len()].copy_from_slice(release);

        // Version (build info)
        let version = b"#1 SMP Scarlet";
        uts.version[..version.len()].copy_from_slice(version);

        // Machine (architecture)
        let machine = b"riscv64";
        uts.machine[..machine.len()].copy_from_slice(machine);

        // Domain name
        let domainname = b"(none)";
        uts.domainname[..domainname.len()].copy_from_slice(domainname);

        uts
    }
}

/// Linux uname system call implementation
///
/// Returns system information including system name, hostname, kernel version,
/// and hardware architecture. This provides compatibility with Linux applications
/// that query system information.
///
/// # Arguments
/// - buf: Pointer to utsname structure to fill
///
/// # Returns
/// - 0 on success
/// - usize::MAX on error (-1 in Linux)
pub fn sys_uname(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let buf_ptr = trapframe.get_arg(0);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Translate user space pointer
    let buf_vaddr = match task.vm_manager.translate_vaddr(buf_ptr) {
        Some(addr) => addr as *mut UtsName,
        None => return usize::MAX, // Invalid address
    };

    if buf_vaddr.is_null() {
        return usize::MAX; // NULL pointer
    }

    // Create and copy system information
    let uts = UtsName::new();
    unsafe {
        *buf_vaddr = uts;
    }

    0 // Success
}
