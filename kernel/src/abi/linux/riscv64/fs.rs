use crate::{
    abi::linux::riscv64::LinuxRiscv64Abi,
    arch::Trapframe,
    device::manager::DeviceManager,
    executor::TransparentExecutor,
    fs::{DirectoryEntry, FileType, SeekFrom},
    library::std::string::{
        cstring_to_string, parse_c_string_from_userspace, parse_string_array_from_userspace,
    },
    object::capability::StreamError,
    sched::scheduler::get_scheduler,
    task::mytask,
};
use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::{Once, RwLock};

use super::errno;

static XORSHIFT_STATE: AtomicU64 = AtomicU64::new(0);
static NEXT_EPOLL_HANDLE_ID: AtomicU32 = AtomicU32::new(1);
static EPOLL_INTERESTS: Once<RwLock<Vec<EpollInterest>>> = Once::new();

const EPOLL_HANDLE_BASE: u32 = 0x3000_0000;
const EPOLLIN: u32 = 0x0001;
const EPOLLPRI: u32 = 0x0002;
const EPOLLOUT: u32 = 0x0004;
const EPOLLERR: u32 = 0x0008;
const EPOLLHUP: u32 = 0x0010;
const EPOLL_CTL_ADD: i32 = 1;
const EPOLL_CTL_DEL: i32 = 2;
const EPOLL_CTL_MOD: i32 = 3;
const EPOLL_EVENT_DATA_OFFSET: usize = 8;
const EPOLL_EVENT_SIZE: usize = 16;

#[derive(Clone, Copy)]
struct EpollInterest {
    epoll_handle: u32,
    fd: i32,
    events: u32,
    data: u64,
}

fn epoll_interests() -> &'static RwLock<Vec<EpollInterest>> {
    EPOLL_INTERESTS.call_once(|| RwLock::new(Vec::new()))
}

fn is_epoll_handle(handle: u32) -> bool {
    (handle & 0xf000_0000) == EPOLL_HANDLE_BASE
}

fn read_linux_epoll_event(task: &crate::task::Task, event_ptr: usize) -> Option<(u32, u64)> {
    if event_ptr == 0 {
        return None;
    }
    let mut buf = [0u8; EPOLL_EVENT_SIZE];
    if copy_from_user_pagewise(&mut buf, event_ptr, &task.vm_manager) != EPOLL_EVENT_SIZE {
        return None;
    }
    let events = u32::from_ne_bytes(buf[0..4].try_into().ok()?);
    let data = u64::from_ne_bytes(
        buf[EPOLL_EVENT_DATA_OFFSET..EPOLL_EVENT_DATA_OFFSET + 8]
            .try_into()
            .ok()?,
    );
    Some((events, data))
}

fn write_linux_epoll_event(
    task: &crate::task::Task,
    event_ptr: usize,
    index: usize,
    events: u32,
    data: u64,
) -> bool {
    let mut buf = [0u8; EPOLL_EVENT_SIZE];
    buf[0..4].copy_from_slice(&events.to_ne_bytes());
    buf[EPOLL_EVENT_DATA_OFFSET..EPOLL_EVENT_DATA_OFFSET + 8].copy_from_slice(&data.to_ne_bytes());
    let ptr = event_ptr.saturating_add(index.saturating_mul(EPOLL_EVENT_SIZE));
    copy_to_user_pagewise(ptr, &buf, &task.vm_manager) == EPOLL_EVENT_SIZE
}

fn epoll_ready_events(
    abi: &LinuxRiscv64Abi,
    task: &crate::task::Task,
    interest: EpollInterest,
) -> u32 {
    let Some(handle) = abi.get_handle(interest.fd as usize) else {
        return EPOLLERR | EPOLLHUP;
    };
    let Some(kobj) = task.handle_table.get(handle) else {
        return EPOLLERR | EPOLLHUP;
    };

    let mut ready = 0u32;
    let want_read = (interest.events & EPOLLIN) != 0;
    let want_write = (interest.events & EPOLLOUT) != 0;
    let want_except = (interest.events & EPOLLPRI) != 0;

    if let Some(sel) = kobj.as_selectable() {
        let rs = sel.current_ready(crate::object::capability::selectable::ReadyInterest {
            read: want_read,
            write: want_write,
            except: want_except,
        });
        if want_read && rs.read {
            ready |= EPOLLIN;
        }
        if want_write && rs.write {
            ready |= EPOLLOUT;
        }
        if want_except && rs.except {
            ready |= EPOLLPRI;
        }
    } else {
        if want_read {
            ready |= EPOLLIN;
        }
        if want_write {
            ready |= EPOLLOUT;
        }
    }

    ready
}

/// Copy bytes from a kernel buffer into a userspace virtual address range,
/// resolving each page independently via `translate_to_kva`.
///
/// Returns the number of bytes actually copied.
fn copy_to_user_pagewise(
    dst_user_vaddr: usize,
    src: &[u8],
    vm_manager: &crate::vm::manager::VirtualMemoryManager,
) -> usize {
    let src_len = src.len();
    if src_len == 0 {
        return 0;
    }
    let mut copied = 0usize;
    let mut cur_user = dst_user_vaddr;
    while copied < src_len {
        let page_off = cur_user & (crate::environment::PAGE_SIZE - 1);
        let chunk = core::cmp::min(crate::environment::PAGE_SIZE - page_off, src_len - copied);
        match vm_manager.translate_to_kva(cur_user) {
            Some(kva) => unsafe {
                core::ptr::copy_nonoverlapping(
                    src.as_ptr().wrapping_add(copied),
                    kva as *mut u8,
                    chunk,
                );
            },
            None => break,
        }
        copied += chunk;
        cur_user += chunk;
    }
    copied
}

/// Copy bytes from a userspace virtual address range into a kernel buffer,
/// resolving each page independently via `translate_to_kva`.
///
/// Returns the number of bytes actually copied.
fn copy_from_user_pagewise(
    dst: &mut [u8],
    src_user_vaddr: usize,
    vm_manager: &crate::vm::manager::VirtualMemoryManager,
) -> usize {
    let dst_len = dst.len();
    if dst_len == 0 {
        return 0;
    }
    let mut copied = 0usize;
    let mut cur_user = src_user_vaddr;
    while copied < dst_len {
        let page_off = cur_user & (crate::environment::PAGE_SIZE - 1);
        let chunk = core::cmp::min(crate::environment::PAGE_SIZE - page_off, dst_len - copied);
        match vm_manager.translate_to_kva(cur_user) {
            Some(kva) => unsafe {
                core::ptr::copy_nonoverlapping(
                    kva as *const u8,
                    dst.as_mut_ptr().wrapping_add(copied),
                    chunk,
                );
            },
            None => break,
        }
        copied += chunk;
        cur_user += chunk;
    }
    copied
}

fn remap_shm_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("/dev/shm/") {
        alloc::format!("/tmp/{}", rest)
    } else if path == "/dev/shm" || path == "/dev/shm/" {
        "/tmp".to_string()
    } else {
        path.to_string()
    }
}

fn is_wl_shm_path(path: &str) -> bool {
    path.starts_with("/tmp/wl_shm-")
}

/// Linux stat structure for RISC-V 64-bit
/// Matches asm-generic struct stat layout used by musl on 64-bit.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct LinuxStat {
    pub st_dev: u64,        // Device ID of device containing file
    pub st_ino: u64,        // Inode number
    pub st_mode: u32,       // File type and mode
    pub st_nlink: u32,      // Number of hard links
    pub st_uid: u32,        // User ID of owner
    pub st_gid: u32,        // Group ID of owner
    pub st_rdev: u64,       // Device ID (if special file)
    pub __pad1: u64,        // Padding for alignment
    pub st_size: i64,       // Total size, in bytes
    pub st_blksize: i32,    // Block size for filesystem I/O
    pub __pad2: i32,        // Padding for alignment
    pub st_blocks: i64,     // Number of 512B blocks allocated
    pub st_atime: i64,      // Time of last access (seconds)
    pub st_atime_nsec: u64, // Time of last access (nanoseconds)
    pub st_mtime: i64,      // Time of last modification (seconds)
    pub st_mtime_nsec: u64, // Time of last modification (nanoseconds)
    pub st_ctime: i64,      // Time of last status change (seconds)
    pub st_ctime_nsec: u64, // Time of last status change (nanoseconds)
    pub __unused4: u32,     // Reserved
    pub __unused5: u32,     // Reserved
}

/// Linux statx timestamp structure
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct LinuxStatxTimestamp {
    pub tv_sec: i64,
    pub tv_nsec: u32,
    pub __reserved: i32,
}

/// Linux statx structure (matches Linux UAPI layout)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct LinuxStatx {
    pub stx_mask: u32,
    pub stx_blksize: u32,
    pub stx_attributes: u64,
    pub stx_nlink: u32,
    pub stx_uid: u32,
    pub stx_gid: u32,
    pub stx_mode: u16,
    pub __spare0: [u16; 1],
    pub stx_ino: u64,
    pub stx_size: u64,
    pub stx_blocks: u64,
    pub stx_attributes_mask: u64,
    pub stx_atime: LinuxStatxTimestamp,
    pub stx_btime: LinuxStatxTimestamp,
    pub stx_ctime: LinuxStatxTimestamp,
    pub stx_mtime: LinuxStatxTimestamp,
    pub stx_rdev_major: u32,
    pub stx_rdev_minor: u32,
    pub stx_dev_major: u32,
    pub stx_dev_minor: u32,
    pub stx_mnt_id: u64,
    pub stx_dio_mem_align: u32,
    pub stx_dio_offset_align: u32,
    pub stx_subvol: u64,
    pub stx_atomic_write_unit_min: u32,
    pub stx_atomic_write_unit_max: u32,
    pub stx_atomic_write_segments_max: u32,
    pub stx_dio_read_offset_align: u32,
    pub __spare3: [u64; 9],
}

// Linux file type constants for st_mode field
#[allow(dead_code)]
pub const S_IFMT: u32 = 0o170000; // Bit mask for the file type bit field
pub const S_IFSOCK: u32 = 0o140000; // Socket
pub const S_IFLNK: u32 = 0o120000; // Symbolic link
pub const S_IFREG: u32 = 0o100000; // Regular file
pub const S_IFBLK: u32 = 0o060000; // Block device
pub const S_IFDIR: u32 = 0o040000; // Directory
pub const S_IFCHR: u32 = 0o020000; // Character device
pub const S_IFIFO: u32 = 0o010000; // FIFO

// Linux permission constants
#[allow(dead_code)]
pub const S_IRWXU: u32 = 0o0700; // User (file owner) has read, write, and execute permission
pub const S_IRUSR: u32 = 0o0400; // User has read permission
pub const S_IWUSR: u32 = 0o0200; // User has write permission
pub const S_IXUSR: u32 = 0o0100; // User has execute permission
#[allow(dead_code)]
pub const S_IRWXG: u32 = 0o0070; // Group has read, write, and execute permission
pub const S_IRGRP: u32 = 0o0040; // Group has read permission
#[allow(dead_code)]
pub const S_IWGRP: u32 = 0o0020; // Group has write permission
pub const S_IXGRP: u32 = 0o0010; // Group has execute permission
#[allow(dead_code)]
pub const S_IRWXO: u32 = 0o0007; // Others have read, write, and execute permission
pub const S_IROTH: u32 = 0o0004; // Others have read permission
#[allow(dead_code)]
pub const S_IWOTH: u32 = 0o0002; // Others have write permission
pub const S_IXOTH: u32 = 0o0001; // Others have execute permission

// Linux statx mask bits
#[allow(dead_code)]
pub const STATX_TYPE: u32 = 0x00000001;
#[allow(dead_code)]
pub const STATX_MODE: u32 = 0x00000002;
#[allow(dead_code)]
pub const STATX_NLINK: u32 = 0x00000004;
#[allow(dead_code)]
pub const STATX_UID: u32 = 0x00000008;
#[allow(dead_code)]
pub const STATX_GID: u32 = 0x00000010;
#[allow(dead_code)]
pub const STATX_ATIME: u32 = 0x00000020;
#[allow(dead_code)]
pub const STATX_MTIME: u32 = 0x00000040;
#[allow(dead_code)]
pub const STATX_CTIME: u32 = 0x00000080;
#[allow(dead_code)]
pub const STATX_INO: u32 = 0x00000100;
#[allow(dead_code)]
pub const STATX_SIZE: u32 = 0x00000200;
#[allow(dead_code)]
pub const STATX_BLOCKS: u32 = 0x00000400;
pub const STATX_BASIC_STATS: u32 = 0x000007ff;
#[allow(dead_code)]
pub const STATX_BTIME: u32 = 0x00000800;

// Linux getrandom flags
#[allow(dead_code)]
pub const GRND_NONBLOCK: u32 = 0x0001;
#[allow(dead_code)]
pub const GRND_RANDOM: u32 = 0x0002;
#[allow(dead_code)]
pub const GRND_INSECURE: u32 = 0x0004;

// Linux fcntl command constants
pub const F_DUPFD: u32 = 0; // Duplicate file descriptor
pub const F_GETFD: u32 = 1; // Get file descriptor flags
pub const F_SETFD: u32 = 2; // Set file descriptor flags
pub const F_GETFL: u32 = 3; // Get file status flags
pub const F_SETFL: u32 = 4; // Set file status flags
pub const F_GETLK: u32 = 5; // Get record locking information
pub const F_SETLK: u32 = 6; // Set record lock (non-blocking)
pub const F_SETLKW: u32 = 7; // Set record lock (blocking)
pub const F_SETOWN: u32 = 8; // Set owner (process receiving SIGIO/SIGURG)
pub const F_GETOWN: u32 = 9; // Get owner (process receiving SIGIO/SIGURG)
pub const F_SETSIG: u32 = 10; // Set signal sent when I/O is possible
pub const F_GETSIG: u32 = 11; // Get signal sent when I/O is possible
pub const F_SETLEASE: u32 = 1024; // Set a lease
pub const F_GETLEASE: u32 = 1025; // Get current lease
pub const F_NOTIFY: u32 = 1026; // Request notifications on a directory
pub const F_DUPFD_CLOEXEC: u32 = 1030; // Duplicate with close-on-exec

// Linux file descriptor flags
pub const FD_CLOEXEC: u32 = 1; // Close-on-exec flag

// Linux open flags
#[allow(dead_code)]
pub const O_RDONLY: i32 = 0o0; // Read only
#[allow(dead_code)]
pub const O_WRONLY: i32 = 0o1; // Write only
#[allow(dead_code)]
pub const O_RDWR: i32 = 0o2; // Read and write
pub const O_CREAT: i32 = 0o100; // Create file if it doesn't exist
pub const O_EXCL: i32 = 0o200; // Fail if file exists (with O_CREAT)
#[allow(dead_code)]
pub const O_NOCTTY: i32 = 0o400; // Don't assign controlling terminal
pub const O_TRUNC: i32 = 0o1000; // Truncate file to zero length
pub const O_APPEND: i32 = 0o2000; // Append mode
#[allow(dead_code)]
pub const O_NONBLOCK: i32 = 0o4000; // Non-blocking mode
#[allow(dead_code)]
pub const O_DSYNC: i32 = 0o10000; // Data sync
#[allow(dead_code)]
pub const O_ASYNC: i32 = 0o20000; // Asynchronous I/O
#[allow(dead_code)]
pub const O_DIRECT: i32 = 0o40000; // Direct I/O
#[allow(dead_code)]
pub const O_LARGEFILE: i32 = 0o100000; // Large file support
pub const O_DIRECTORY: i32 = 0o200000; // Must be a directory
#[allow(dead_code)]
pub const O_NOFOLLOW: i32 = 0o400000; // Don't follow symlinks
#[allow(dead_code)]
pub const O_NOATIME: i32 = 0o1000000; // Don't update access time
pub const O_CLOEXEC: i32 = 0o2000000; // Close-on-exec
#[allow(dead_code)]
pub const O_SYNC: i32 = O_DSYNC; // Data and metadata sync
#[allow(dead_code)]
pub const O_PATH: i32 = 0o10000000; // Path-based operations only
#[allow(dead_code)]
pub const O_TMPFILE: i32 = 0o20000000; // Create temporary file

use crate::device::DeviceCapability;

impl LinuxStat {
    /// Create a new LinuxStat from Scarlet FileMetadata
    pub fn from_metadata(metadata: &crate::fs::FileMetadata) -> Self {
        let st_mode = match metadata.file_type {
            FileType::RegularFile => S_IFREG,
            FileType::Directory => S_IFDIR,
            FileType::CharDevice(_) => S_IFCHR,
            FileType::BlockDevice(_) => S_IFBLK,
            FileType::SymbolicLink(_) => S_IFLNK,
            FileType::Pipe => S_IFIFO,
            FileType::Socket(_) => S_IFSOCK,
            FileType::Unknown => S_IFREG, // Default to regular file
        } | if metadata.permissions.read {
            S_IRUSR | S_IRGRP | S_IXGRP | S_IROTH
        } else {
            0
        } | if metadata.permissions.write {
            S_IWUSR
        } else {
            0
        } | if metadata.permissions.execute {
            S_IXUSR | S_IXGRP | S_IXOTH
        } else {
            0
        };

        Self {
            st_dev: 0, // Virtual device ID
            st_ino: metadata.file_id,
            st_mode,
            st_nlink: metadata.link_count as u32,
            st_uid: 0,  // Root user
            st_gid: 0,  // Root group
            st_rdev: 0, // Not a special file by default
            __pad1: 0,
            st_size: metadata.size as i64,
            st_blksize: 4096, // Standard block size
            __pad2: 0,
            st_blocks: ((metadata.size + 511) / 512) as i64, // Number of 512-byte blocks
            st_atime: metadata.accessed_time as i64,
            st_atime_nsec: 0,
            st_mtime: metadata.modified_time as i64,
            st_mtime_nsec: 0,
            st_ctime: metadata.created_time as i64,
            st_ctime_nsec: 0,
            __unused4: 0,
            __unused5: 0,
        }
    }
}

fn statx_timestamp_from_secs(seconds: u64) -> LinuxStatxTimestamp {
    LinuxStatxTimestamp {
        tv_sec: seconds as i64,
        tv_nsec: 0,
        __reserved: 0,
    }
}

fn next_pseudo_random_u64() -> u64 {
    loop {
        let mut state = XORSHIFT_STATE.load(Ordering::Relaxed);
        if state == 0 {
            state = crate::time::current_time() ^ 0x9e3779b97f4a7c15;
            if state == 0 {
                state = 0x4f1bbcdcb7a43413;
            }
        }
        let mut x = state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        if XORSHIFT_STATE
            .compare_exchange(state, x, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return x;
        }
    }
}

fn fill_pseudo_random(buffer: &mut [u8]) {
    let mut offset = 0;
    while offset < buffer.len() {
        let bytes = next_pseudo_random_u64().to_le_bytes();
        let to_copy = core::cmp::min(bytes.len(), buffer.len() - offset);
        buffer[offset..offset + to_copy].copy_from_slice(&bytes[..to_copy]);
        offset += to_copy;
    }
}

fn fill_statx_from_stat(
    statx: &mut LinuxStatx,
    stat: &LinuxStat,
    created_time: u64,
    requested_mask: u32,
) {
    let supported_mask = STATX_BASIC_STATS | STATX_BTIME;
    let effective_mask = if requested_mask == 0 {
        STATX_BASIC_STATS
    } else {
        requested_mask
    };

    statx.stx_mask = supported_mask & effective_mask;
    statx.stx_blksize = stat.st_blksize as u32;
    statx.stx_attributes = 0;
    statx.stx_nlink = stat.st_nlink;
    statx.stx_uid = stat.st_uid;
    statx.stx_gid = stat.st_gid;
    statx.stx_mode = stat.st_mode as u16;
    statx.__spare0 = [0; 1];
    statx.stx_ino = stat.st_ino;
    statx.stx_size = stat.st_size as u64;
    statx.stx_blocks = stat.st_blocks as u64;
    statx.stx_attributes_mask = 0;
    statx.stx_atime = statx_timestamp_from_secs(stat.st_atime as u64);
    statx.stx_btime = statx_timestamp_from_secs(created_time);
    statx.stx_ctime = statx_timestamp_from_secs(stat.st_ctime as u64);
    statx.stx_mtime = statx_timestamp_from_secs(stat.st_mtime as u64);
    statx.stx_rdev_major = 0;
    statx.stx_rdev_minor = 0;
    statx.stx_dev_major = 0;
    statx.stx_dev_minor = 0;
    statx.stx_mnt_id = 0;
    statx.stx_dio_mem_align = 0;
    statx.stx_dio_offset_align = 0;
    statx.stx_subvol = 0;
    statx.stx_atomic_write_unit_min = 0;
    statx.stx_atomic_write_unit_max = 0;
    statx.stx_atomic_write_segments_max = 0;
    statx.stx_dio_read_offset_align = 0;
    statx.__spare3 = [0; 9];
}

// /// Convert Scarlet DirectoryEntry to Linux Dirent and write to buffer
// fn read_directory_as_Linux_dirent(buf_ptr: *mut u8, count: usize, buffer_data: &[u8]) -> usize {
//     if count < Dirent::DIRENT_SIZE {
//         return 0; // Buffer too small for even one entry
//     }

//     // Parse DirectoryEntry from buffer data
//     if let Some(dir_entry) = DirectoryEntry::parse(buffer_data) {
//         // Convert Scarlet DirectoryEntry to Linux Dirent
//         let inum = (dir_entry.file_id & 0xFFFF) as u16; // Use lower 16 bits as inode number
//         let name = dir_entry.name_str().unwrap_or("");

//         let Linux_dirent = Dirent::new(inum, name);

//         // Check if we have enough space
//         if count >= Dirent::DIRENT_SIZE {
//             // Copy the dirent to the buffer
//             let dirent_bytes = Linux_dirent.as_bytes();
//             unsafe {
//                 core::ptr::copy_nonoverlapping(
//                     dirent_bytes.as_ptr(),
//                     buf_ptr,
//                     Dirent::DIRENT_SIZE
//                 );
//             }
//             return Dirent::DIRENT_SIZE;
//         }
//     }

//     0 // No data or error
// }

const MAX_PATH_LENGTH: usize = 1024; // Increased to handle long command lines
const MAX_ARG_COUNT: usize = 64;

/// Linux sys_exec system call implementation
/// Executes a program specified by the given path, replacing the current process image with a new one.
/// Also allows passing arguments and environment variables to the new program.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) on error
#[allow(dead_code)]
pub fn sys_exec(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();

    // Increment PC to avoid infinite loop if execve fails
    trapframe.increment_pc_next(task);

    // Get arguments from trapframe
    let path_ptr = trapframe.get_arg(0);
    let argv_ptr = trapframe.get_arg(1);

    // Parse path
    let path_str = match parse_c_string_from_userspace(task, path_ptr, MAX_PATH_LENGTH) {
        Ok(path) => match to_absolute_path_v2(&task, &path) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX, // Path error
        },
        Err(_) => return usize::MAX, // Path parsing error
    };

    // Parse argv and envp
    let argv_strings =
        match parse_string_array_from_userspace(task, argv_ptr, MAX_ARG_COUNT, MAX_PATH_LENGTH) {
            Ok(args) => args,
            Err(_) => return usize::MAX, // argv parsing error
        };

    // Convert Vec<String> to Vec<&str> for TransparentExecutor
    let argv_refs: Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();

    // Use TransparentExecutor for cross-ABI execution
    match TransparentExecutor::execute_binary(&path_str, &argv_refs, &[], task, trapframe, false) {
        Ok(_) => {
            // execve normally should not return on success - the process is replaced
            // However, if ABI module sets trapframe return value and returns here,
            // we should respect that value instead of hardcoding 0
            trapframe.get_return_value()
        }
        Err(_) => {
            // Execution failed - return error code
            // The trap handler will automatically set trapframe return value from our return
            usize::MAX // Error return value
        }
    }
}

#[repr(i32)]
#[allow(dead_code)]
enum OpenMode {
    ReadOnly = 0x000,
    WriteOnly = 0x001,
    ReadWrite = 0x002,
    Create = 0x200,
    Truncate = 0x400,
}

/// Linux sys_openat implementation for Scarlet VFS v2
///
/// Opens a file relative to a directory file descriptor (dirfd) and path.
/// If dirfd == AT_FDCWD, uses the current working directory as the base.
/// Otherwise, resolves the base directory from the file descriptor.
/// Uses VfsManager::open_relative for safe and efficient path resolution.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///
/// Returns:
/// - File descriptor on success
/// - usize::MAX (Linux -1) on error
pub fn sys_openat(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let dirfd = trapframe.get_arg(0) as i32;
    let path_ptr = task
        .vm_manager
        .translate_to_kva(trapframe.get_arg(1))
        .unwrap() as *const u8;
    let flags = trapframe.get_arg(2) as i32;

    // Increment PC to avoid infinite loop if openat fails
    trapframe.increment_pc_next(task);

    // Parse path from user space
    let path_str = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((path, _)) => path,
        Err(_) => return errno::to_result(errno::EFAULT), // Invalid UTF-8 or bad address
    };

    let path_str = remap_shm_path(&path_str);

    // crate::println!("sys_openat: epc={:#x}, dirfd={}, path='{}', flags={:#o}", trapframe.epc, dirfd, path_str, flags);

    let vfs_guard = task.vfs.read();
    let vfs = vfs_guard.as_deref().unwrap();

    // Determine base directory (entry and mount) for path resolution
    use crate::fs::vfs_v2::core::VfsFileObject;

    const AT_FDCWD: i32 = -100;
    let (base_entry, base_mount) = if dirfd == AT_FDCWD {
        // Use current working directory as base
        vfs.get_cwd().unwrap_or_else(|| {
            let root_mount = vfs.mount_tree.root_mount.read().clone();
            (root_mount.root.clone(), root_mount)
        })
    } else {
        // Use directory file descriptor as base
        let handle = match abi.get_handle(dirfd as usize) {
            Some(h) => h,
            None => return errno::to_result(errno::EBADF), // Bad file descriptor
        };
        let kernel_obj = match task.handle_table.get(handle) {
            Some(obj) => obj,
            None => return errno::to_result(errno::EBADF), // Bad file descriptor
        };
        let file_obj = match kernel_obj.as_file() {
            Some(f) => f,
            None => return errno::to_result(errno::ENOTDIR), // Not a directory
        };
        let vfs_file_obj = file_obj
            .as_any()
            .downcast_ref::<VfsFileObject>()
            .ok_or(())
            .unwrap();
        (
            vfs_file_obj.get_vfs_entry().clone(),
            vfs_file_obj.get_mount_point().clone(),
        )
    };

    // Open the file using VfsManager::open_relative
    // Apply a few Linux-compat path translations for devices
    let mapped_path = if path_str == "/dev/tty" {
        "/dev/tty0".to_string()
    } else if let Some(rest) = path_str.strip_prefix("/dev/vc/") {
        // Map /dev/vc/N -> /dev/ttyN; if ttyN doesn't exist, we may further alias below
        alloc::format!("/dev/tty{}", rest)
    } else if let Some(n) = path_str.strip_prefix("/dev/tty") {
        // If requesting a numbered tty other than 0, alias to tty0 for minimal support
        // This is a compatibility shim until multiple VTs are implemented
        if n != "0" && n.chars().all(|c| c.is_ascii_digit()) {
            "/dev/tty0".to_string()
        } else {
            path_str.clone()
        }
    } else {
        path_str.clone()
    };

    // crate::println!("sys_openat: attempting to open '{}' with flags {:#o} (dirfd={})", mapped_path, flags, dirfd);

    // // Log flags details
    // let flags_table = [
    //     (O_RDONLY, "O_RDONLY"),
    //     (O_WRONLY, "O_WRONLY"),
    //     (O_RDWR, "O_RDWR"),
    //     (O_CREAT, "O_CREAT"),
    //     (O_EXCL, "O_EXCL"),
    //     (O_TRUNC, "O_TRUNC"),
    //     (O_APPEND, "O_APPEND"),
    //     (O_DIRECTORY, "O_DIRECTORY"),
    //     (O_CLOEXEC, "O_CLOEXEC"),
    // ];
    // for (flag, name) in flags_table.iter() {
    //     if (flags & *flag) != 0 {
    //         crate::println!("  Flag set: {}", name);
    //     }
    // }

    let file = vfs.open_from(&base_entry, &base_mount, &mapped_path, flags as u32);

    let kernel_obj = match file {
        Ok(obj) => {
            // crate::println!("sys_openat: successfully opened '{}'", mapped_path);
            obj
        }
        Err(e) => {
            // crate::println!("sys_openat: failed to open '{}' -> {:?}", mapped_path, e);
            // If open failed and O_CREAT flag is set, try to create the file
            if flags & O_CREAT != 0 {
                // crate::println!("sys_openat: O_CREAT flag set, attempting to create file '{}'", path_str);
                // Build absolute path for file creation before getting mutable VFS reference
                let absolute_path = if mapped_path.starts_with('/') {
                    mapped_path.to_string()
                } else {
                    // Construct absolute path by resolving relative to current working directory
                    match to_absolute_path_v2(&task, &mapped_path) {
                        Ok(p) => p,
                        Err(_) => return errno::to_result(errno::ENOENT), // Path resolution failed
                    }
                };

                // Get mutable VFS reference for file creation
                let vfs_mut = match task.vfs.read().clone() {
                    Some(v) => v,
                    None => return errno::to_result(errno::EIO), // VFS not available
                };

                // Create the file (regular file type)
                match vfs_mut.create_file(&absolute_path, FileType::RegularFile) {
                    Ok(_) => {
                        // File created successfully, now try to open it
                        // Get immutable VFS reference again for opening
                        let vfs_guard = task.vfs.read();
                        let vfs = vfs_guard.as_deref().unwrap();
                        match vfs.open_from(&base_entry, &base_mount, &mapped_path, flags as u32) {
                            Ok(obj) => obj,
                            Err(err) => return errno::to_result(errno::from_fs_error(&err)),
                        }
                    }
                    Err(create_err) => {
                        // Check if file already exists and O_EXCL is set
                        if flags & O_EXCL != 0
                            && create_err.kind == crate::fs::FileSystemErrorKind::AlreadyExists
                        {
                            return errno::to_result(errno::EEXIST); // File exists and O_EXCL is set
                        }
                        // Try to open the existing file
                        let vfs_guard = task.vfs.read();
                        let vfs = vfs_guard.as_deref().unwrap();
                        let reopen_flags = (flags as u32) & !((O_CREAT | O_EXCL) as u32);
                        match vfs.open_from(&base_entry, &base_mount, &mapped_path, reopen_flags) {
                            Ok(obj) => obj,
                            Err(open_err) => {
                                return errno::to_result(errno::from_fs_error(&open_err));
                            }
                        }
                    }
                }
            } else {
                return errno::to_result(errno::from_fs_error(&e)); // Return appropriate error based on VFS error
            }
        }
    };

    // Post-open flag handling (O_DIRECTORY, O_TRUNC, O_APPEND)
    if let Some(file_obj) = kernel_obj.as_file() {
        if (flags & O_DIRECTORY) != 0 {
            if let Ok(meta) = file_obj.metadata() {
                if !matches!(meta.file_type, FileType::Directory) {
                    return errno::to_result(errno::ENOTDIR);
                }
            }
        }
        if (flags & O_TRUNC) != 0 {
            let _ = file_obj.truncate(0);
        }
        if (flags & O_APPEND) != 0 {
            let _ = file_obj.seek(SeekFrom::End(0));
        }
    }

    // Register the file with the task using HandleTable
    let handle = task.handle_table.insert(kernel_obj);
    match handle {
        Ok(handle) => {
            match abi.allocate_fd(handle as u32) {
                Ok(fd) => {
                    // crate::println!("sys_openat: allocated fd {} for '{}'", fd, path_str);
                    // Initialize file status flags (e.g., O_NONBLOCK) from open flags
                    let mut status_flags: u32 = 0;
                    if (flags & O_NONBLOCK) != 0 {
                        status_flags |= O_NONBLOCK as u32;
                    }
                    let _ = abi.set_file_status_flags(fd, status_flags);

                    // Propagate non-blocking to the underlying object Selectable if available
                    if let Some(obj) = task.handle_table.get(handle) {
                        if let Some(sel) = obj.as_selectable() {
                            sel.set_nonblocking(((status_flags as i32) & O_NONBLOCK) != 0);
                        }
                    }
                    fd
                }
                Err(_) => errno::to_result(errno::EMFILE), // Too many open files
            }
        }
        Err(_) => errno::to_result(errno::ENFILE), // Handle table full
    }
}

pub fn sys_dup(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    trapframe.increment_pc_next(task);

    // Get handle from Linux fd
    if let Some(old_handle) = abi.get_handle(fd) {
        // Use clone_for_dup to get proper dup() semantics (increments Pipe reader/writer counts etc.)
        if let Some(kernel_obj) = task.handle_table.clone_for_dup(old_handle) {
            let handle = task.handle_table.insert(kernel_obj);
            match handle {
                Ok(new_handle) => {
                    match abi.allocate_fd(new_handle as u32) {
                        Ok(fd) => fd,
                        Err(_) => errno::to_result(errno::EMFILE), // Too many open files
                    }
                }
                Err(_) => errno::to_result(errno::ENFILE), // Handle table full
            }
        } else {
            errno::to_result(errno::EBADF) // Handle not found in handle table
        }
    } else {
        errno::to_result(errno::EBADF) // Invalid file descriptor
    }
}

pub fn sys_dup3(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let oldfd = trapframe.get_arg(0) as usize;
    let newfd = trapframe.get_arg(1) as usize;
    let flags = trapframe.get_arg(2) as u32;
    trapframe.increment_pc_next(task);

    // dup3 does not allow oldfd and newfd to be the same
    if oldfd == newfd {
        return usize::MAX; // EINVAL
    }

    // Only O_CLOEXEC flag is valid for dup3
    if flags != 0 && flags != (O_CLOEXEC as u32) {
        return usize::MAX; // EINVAL
    }

    // Get handle from old fd
    if let Some(old_handle) = abi.get_handle(oldfd) {
        // Use clone_for_dup to get proper dup() semantics (increments Pipe reader/writer counts etc.)
        if let Some(kernel_obj) = task.handle_table.clone_for_dup(old_handle) {
            let handle = task.handle_table.insert(kernel_obj);
            match handle {
                Ok(new_handle) => {
                    // Close newfd if it's already open
                    if abi.get_handle(newfd).is_some() {
                        if let Some(old_new_handle) = abi.remove_fd(newfd) {
                            let _ = task.handle_table.remove(old_new_handle);
                        }
                    }

                    // Allocate specific fd
                    match abi.allocate_specific_fd(newfd, new_handle as u32) {
                        Ok(()) => {
                            // Set flags if O_CLOEXEC is specified
                            if flags & (O_CLOEXEC as u32) != 0 {
                                let _ = abi.set_fd_flags(newfd, FD_CLOEXEC);
                            }
                            newfd
                        }
                        Err(_) => usize::MAX, // Cannot allocate specific fd
                    }
                }
                Err(_) => usize::MAX, // Handle table full
            }
        } else {
            usize::MAX // Handle not found in handle table
        }
    } else {
        usize::MAX // Invalid old file descriptor
    }
}

pub fn sys_close(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    trapframe.increment_pc_next(task);

    // Get handle from Linux fd and remove mapping
    if let Some(handle) = abi.remove_fd(fd) {
        if task.handle_table.remove(handle).is_some() {
            0 // Success
        } else {
            usize::MAX // Handle not found in handle table
        }
    } else {
        usize::MAX // Invalid file descriptor
    }
}

pub fn sys_read(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let user_buf = trapframe.get_arg(1);
    let count = trapframe.get_arg(2) as usize;

    // Get handle from Linux fd
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.increment_pc_next(task);
            return usize::MAX;
        }
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => {
            trapframe.increment_pc_next(task);
            return usize::MAX;
        }
    };

    // Determine non-blocking mode
    let nonblocking = abi
        .get_file_status_flags(fd)
        .map(|f| ((f as i32) & O_NONBLOCK) != 0)
        .unwrap_or(false);

    // Check if this is a directory by getting file metadata
    let is_directory = if let Some(file_obj) = kernel_obj.as_file() {
        if let Ok(metadata) = file_obj.metadata() {
            matches!(metadata.file_type, FileType::Directory)
        } else {
            false
        }
    } else {
        false
    };

    let stream = match kernel_obj.as_stream() {
        Some(stream) => stream,
        None => {
            trapframe.increment_pc_next(task);
            return usize::MAX;
        }
    };

    if is_directory {
        trapframe.increment_pc_next(task);
        return usize::MAX;
    }

    // Fast path: buffer fits within a single page
    let page_offset = user_buf & (crate::environment::PAGE_SIZE - 1);
    if page_offset + count <= crate::environment::PAGE_SIZE {
        let buf_ptr = match task.vm_manager.translate_to_kva(user_buf) {
            Some(kva) => kva as *mut u8,
            None => {
                trapframe.increment_pc_next(task);
                return usize::MAX;
            }
        };
        let mut buffer = unsafe { core::slice::from_raw_parts_mut(buf_ptr, count) };
        match stream.read(&mut buffer) {
            Ok(n) => {
                trapframe.increment_pc_next(task);
                n
            }
            Err(e) => {
                trapframe.increment_pc_next(task);
                match e {
                    StreamError::EndOfStream => 0,
                    StreamError::WouldBlock => {
                        if nonblocking {
                            return errno::to_result(errno::EAGAIN);
                        } else {
                            schedule(trapframe);
                            usize::MAX
                        }
                    }
                    _ => usize::MAX,
                }
            }
        }
    } else {
        // Multi-page path: read in PAGE_SIZE chunks via kernel stack buffer
        let mut page_buf = [0u8; crate::environment::PAGE_SIZE];
        let mut total_read = 0usize;
        let mut remaining = count;
        let mut cur_user = user_buf;

        while remaining > 0 {
            let page_off = cur_user & (crate::environment::PAGE_SIZE - 1);
            let chunk_size = core::cmp::min(crate::environment::PAGE_SIZE - page_off, remaining);
            let mut chunk_buf = &mut page_buf[..chunk_size];

            let n = match stream.read(&mut chunk_buf) {
                Ok(n) => n,
                Err(StreamError::EndOfStream) => break,
                Err(StreamError::WouldBlock) => {
                    if nonblocking {
                        if total_read > 0 {
                            break;
                        }
                        trapframe.increment_pc_next(task);
                        return errno::to_result(errno::EAGAIN);
                    } else {
                        break;
                    }
                }
                Err(_) => {
                    if total_read == 0 {
                        trapframe.increment_pc_next(task);
                        return usize::MAX;
                    }
                    break;
                }
            };

            if n == 0 {
                break;
            }

            let copied = copy_to_user_pagewise(cur_user, &page_buf[..n], &task.vm_manager);
            if copied != n {
                break;
            }

            total_read += n;
            remaining -= n;
            cur_user += n;
        }

        trapframe.increment_pc_next(task);
        total_read
    }
}

pub fn sys_write(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let user_buf = trapframe.get_arg(1);
    let count = trapframe.get_arg(2) as usize;

    trapframe.increment_pc_next(task);

    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return usize::MAX,
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX,
    };

    let stream = match kernel_obj.as_stream() {
        Some(stream) => stream,
        None => return usize::MAX,
    };

    let nonblocking = abi
        .get_file_status_flags(fd)
        .map(|f| ((f as i32) & O_NONBLOCK) != 0)
        .unwrap_or(false);

    // Fast path: buffer fits within a single page
    let page_offset = user_buf & (crate::environment::PAGE_SIZE - 1);
    if page_offset + count <= crate::environment::PAGE_SIZE {
        let buf_ptr = match task.vm_manager.translate_to_kva(user_buf) {
            Some(kva) => kva as *const u8,
            None => return usize::MAX,
        };
        let buffer = unsafe { core::slice::from_raw_parts(buf_ptr, count) };
        match stream.write(buffer) {
            Ok(n) => n,
            Err(StreamError::WouldBlock) => {
                if nonblocking {
                    return errno::to_result(errno::EAGAIN);
                } else {
                    schedule(trapframe);
                    usize::MAX
                }
            }
            Err(_) => usize::MAX,
        }
    } else {
        // Multi-page path: copy from userspace page-by-page into kernel buffer
        let mut page_buf = [0u8; crate::environment::PAGE_SIZE];
        let mut total_written = 0usize;
        let mut remaining = count;
        let mut cur_user = user_buf;

        while remaining > 0 {
            let page_off = cur_user & (crate::environment::PAGE_SIZE - 1);
            let chunk_size = core::cmp::min(crate::environment::PAGE_SIZE - page_off, remaining);

            let copied =
                copy_from_user_pagewise(&mut page_buf[..chunk_size], cur_user, &task.vm_manager);
            if copied != chunk_size {
                break;
            }

            let n = match stream.write(&page_buf[..chunk_size]) {
                Ok(n) => n,
                Err(StreamError::WouldBlock) => {
                    if nonblocking && total_written == 0 {
                        return errno::to_result(errno::EAGAIN);
                    }
                    break;
                }
                Err(_) => {
                    if total_written == 0 {
                        return usize::MAX;
                    }
                    break;
                }
            };

            total_written += n;
            remaining -= n;
            cur_user += n;
        }

        total_written
    }
}

pub fn sys_pread64(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let buf_addr = trapframe.get_arg(1);
    let count = trapframe.get_arg(2) as usize;
    let position = trapframe.get_arg(3) as i64;

    if position < 0 {
        trapframe.increment_pc_next(task);
        return errno::to_result(errno::EINVAL);
    }

    if count == 0 {
        trapframe.increment_pc_next(task);
        return 0;
    }

    let buf_ptr = match task.vm_manager.translate_to_kva(buf_addr) {
        Some(ptr) => ptr as *mut u8,
        None => {
            trapframe.increment_pc_next(task);
            return errno::to_result(errno::EFAULT);
        }
    };

    if buf_ptr.is_null() {
        trapframe.increment_pc_next(task);
        return errno::to_result(errno::EFAULT);
    }

    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.increment_pc_next(task);
            return errno::to_result(errno::EBADF);
        }
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => {
            trapframe.increment_pc_next(task);
            return errno::to_result(errno::EBADF);
        }
    };

    let file = match kernel_obj.as_file() {
        Some(file) => file,
        None => {
            trapframe.increment_pc_next(task);
            if kernel_obj.as_stream().is_some() {
                return errno::to_result(errno::ESPIPE);
            }
            return errno::to_result(errno::EBADF);
        }
    };

    let mut buffer = unsafe { core::slice::from_raw_parts_mut(buf_ptr, count) };

    let nonblocking = abi
        .get_file_status_flags(fd)
        .map(|f| ((f as i32) & O_NONBLOCK) != 0)
        .unwrap_or(false);

    match file.read_at(position as u64, &mut buffer) {
        Ok(n) => {
            trapframe.increment_pc_next(task);
            n
        }
        Err(StreamError::EndOfStream) => {
            trapframe.increment_pc_next(task);
            0
        }
        Err(StreamError::WouldBlock) => {
            if nonblocking {
                trapframe.increment_pc_next(task);
                errno::to_result(errno::EAGAIN)
            } else {
                schedule(trapframe);
                usize::MAX
            }
        }
        Err(err) => {
            trapframe.increment_pc_next(task);
            errno::to_result(stream_error_to_errno(err))
        }
    }
}

pub fn sys_pwrite64(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let buf_addr = trapframe.get_arg(1);
    let count = trapframe.get_arg(2) as usize;
    let position = trapframe.get_arg(3) as i64;

    if position < 0 {
        trapframe.increment_pc_next(task);
        return errno::to_result(errno::EINVAL);
    }

    if count == 0 {
        trapframe.increment_pc_next(task);
        return 0;
    }

    let buf_ptr = match task.vm_manager.translate_to_kva(buf_addr) {
        Some(ptr) => ptr as *const u8,
        None => {
            trapframe.increment_pc_next(task);
            return errno::to_result(errno::EFAULT);
        }
    };

    if buf_ptr.is_null() {
        trapframe.increment_pc_next(task);
        return errno::to_result(errno::EFAULT);
    }

    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.increment_pc_next(task);
            return errno::to_result(errno::EBADF);
        }
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => {
            trapframe.increment_pc_next(task);
            return errno::to_result(errno::EBADF);
        }
    };

    let file = match kernel_obj.as_file() {
        Some(file) => file,
        None => {
            trapframe.increment_pc_next(task);
            if kernel_obj.as_stream().is_some() {
                return errno::to_result(errno::ESPIPE);
            }
            return errno::to_result(errno::EBADF);
        }
    };

    let buffer = unsafe { core::slice::from_raw_parts(buf_ptr, count) };

    let nonblocking = abi
        .get_file_status_flags(fd)
        .map(|f| ((f as i32) & O_NONBLOCK) != 0)
        .unwrap_or(false);

    match file.write_at(position as u64, buffer) {
        Ok(n) => {
            trapframe.increment_pc_next(task);
            n
        }
        Err(StreamError::WouldBlock) => {
            if nonblocking {
                trapframe.increment_pc_next(task);
                errno::to_result(errno::EAGAIN)
            } else {
                schedule(trapframe);
                usize::MAX
            }
        }
        Err(err) => {
            trapframe.increment_pc_next(task);
            errno::to_result(stream_error_to_errno(err))
        }
    }
}

fn stream_error_to_errno(err: StreamError) -> usize {
    match err {
        StreamError::EndOfStream => errno::SUCCESS,
        StreamError::WouldBlock => errno::EAGAIN,
        StreamError::IoError => errno::EIO,
        StreamError::Closed => errno::EBADF,
        StreamError::InvalidArgument => errno::EINVAL,
        StreamError::Interrupted => errno::EINTR,
        StreamError::PermissionDenied => errno::EACCES,
        StreamError::DeviceError => errno::EIO,
        StreamError::NotSupported | StreamError::SeekError => errno::ESPIPE,
        StreamError::NoSpace => errno::ENOSPC,
        StreamError::BrokenPipe => errno::EPIPE,
        StreamError::FileSystemError(fs_err) => errno::from_fs_error(&fs_err),
        StreamError::Other(_) => errno::EIO,
    }
}

/// Linux writev system call implementation
///
/// This system call writes data from multiple buffers (I/O vectors) to a file descriptor.
/// It provides scatter-gather I/O functionality, allowing efficient writes from multiple
/// non-contiguous memory regions in a single system call.
///
/// # Arguments
/// - fd: File descriptor
/// - iovec: Array of iovec structures describing the buffers
/// - iovcnt: Number of iovec structures in the array
///
/// # Returns
/// - Number of bytes written on success
/// - usize::MAX on error
pub fn sys_writev(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let iovec_ptr = trapframe.get_arg(1);
    let iovcnt = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if writev fails
    trapframe.increment_pc_next(task);

    // Validate parameters
    if iovcnt == 0 {
        return 0; // Nothing to write
    }

    // Linux typically limits iovcnt to prevent resource exhaustion
    const IOV_MAX: usize = 1024;
    if iovcnt > IOV_MAX {
        return usize::MAX; // Too many vectors
    }

    // Get handle from Linux fd
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return usize::MAX, // Invalid file descriptor
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid file descriptor
    };

    let stream = match kernel_obj.as_stream() {
        Some(stream) => stream,
        None => return usize::MAX, // Not a stream object
    };

    let nonblocking = abi
        .get_file_status_flags(fd)
        .map(|f| ((f as i32) & O_NONBLOCK) != 0)
        .unwrap_or(false);

    // Translate and validate iovec array pointer
    let iovec_vaddr = match task.vm_manager.translate_to_kva(iovec_ptr) {
        Some(addr) => addr as *const IoVec,
        None => return usize::MAX, // Invalid address
    };

    if iovec_vaddr.is_null() {
        return usize::MAX; // NULL pointer
    }

    // Read iovec structures from user space
    let iovecs = unsafe { core::slice::from_raw_parts(iovec_vaddr, iovcnt) };

    let mut total_written = 0usize;

    // Process each iovec
    for iovec in iovecs {
        if iovec.iov_len == 0 {
            continue; // Skip empty buffers
        }

        // Translate buffer address
        let buf_vaddr = match task.vm_manager.translate_to_kva(iovec.iov_base as usize) {
            Some(addr) => addr as *const u8,
            None => return usize::MAX, // Invalid buffer address
        };

        if buf_vaddr.is_null() {
            return usize::MAX; // NULL buffer pointer
        }

        // Create a slice from the user buffer
        let buffer = unsafe { core::slice::from_raw_parts(buf_vaddr, iovec.iov_len) };

        // Write data from this buffer
        match stream.write(buffer) {
            Ok(n) => {
                total_written = total_written.saturating_add(n);

                // If partial write occurred, stop processing remaining vectors
                // This matches Linux behavior for writev
                if n < iovec.iov_len {
                    break;
                }
            }
            Err(StreamError::WouldBlock) => {
                if nonblocking {
                    // If some bytes were written, return them; otherwise, EAGAIN
                    if total_written == 0 {
                        return errno::to_result(errno::EAGAIN);
                    } else {
                        break;
                    }
                } else {
                    schedule(trapframe);
                    return usize::MAX;
                }
            }
            Err(_) => {
                // If no bytes were written at all, return error
                // If some bytes were written, return the count
                if total_written == 0 {
                    return usize::MAX;
                } else {
                    break;
                }
            }
        }
    }

    total_written
}

pub fn sys_lseek(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let offset = trapframe.get_arg(1) as i64;
    let whence = trapframe.get_arg(2) as i32;

    // Increment PC to avoid infinite loop if lseek fails
    trapframe.increment_pc_next(task);

    // Get handle from Linux fd
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return usize::MAX, // Invalid file descriptor
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid file descriptor
    };

    let file = match kernel_obj.as_file() {
        Some(file) => file,
        None => return usize::MAX, // Not a file object
    };

    let whence = match whence {
        0 => SeekFrom::Start(offset as u64),
        1 => SeekFrom::Current(offset),
        2 => SeekFrom::End(offset),
        _ => return usize::MAX, // Invalid whence
    };

    match file.seek(whence) {
        Ok(pos) => pos as usize,
        Err(e) => {
            crate::println!("sys_lseek: seek error: {:?}", e);
            usize::MAX // Seek error
        }
    }
}

// // Create device file
// pub fn sys_mknod(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
//     let task = mytask().unwrap();
//     trapframe.increment_pc_next(task);
//     let name_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
//     let name = get_path_str_v2(name_ptr).unwrap();
//     let path = to_absolute_path_v2(&task, &name).unwrap();

//     let major = trapframe.get_arg(1) as u32;
//     let minor = trapframe.get_arg(2) as u32;

//     match (major, minor) {
//         (1, 0) => {
//             // Create a console device
//             let console_dev = Some(DeviceManager::get_manager().register_device(Arc::new(
//                 crate::abi::Linux::drivers::console::ConsoleDevice::new(0, "console")
//             )));

//             let vfs = task.vfs.read().clone().unwrap();
//             let _res = vfs.create_file(&path, FileType::CharDevice(
//                 DeviceFileInfo {
//                     device_id: console_dev.unwrap(),
//                     device_type: crate::device::DeviceType::Char,
//                 }
//             ));
//             // crate::println!("Created console device at {}", path);
//         },
//         _ => {},
//     }
//     0
// }

// pub fn sys_fstat(abi: &mut LinuxRiscv64Abi, trapframe: &mut crate::arch::Trapframe) -> usize {
//     let fd = trapframe.get_arg(0) as usize;

//     let task = mytask()
//         .expect("sys_fstat: No current task found");
//     trapframe.increment_pc_next(task); // Increment the program counter

//     let stat_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1) as usize)
//         .expect("sys_fstat: Failed to translate stat pointer") as *mut Stat;

//     // Get handle from Linux fd
//     let handle = match abi.get_handle(fd) {
//         Some(h) => h,
//         None => return usize::MAX, // Invalid file descriptor
//     };

//     let kernel_obj = match task.handle_table.get(handle) {
//         Some(obj) => obj,
//         None => return usize::MAX, // Return -1 on error
//     };

//     let file = match kernel_obj.as_file() {
//         Some(file) => file,
//         None => return usize::MAX, // Not a file object
//     };

//     let metadata = file.metadata()
//         .expect("sys_fstat: Failed to get file metadata");

//     if stat_ptr.is_null() {
//         return usize::MAX; // Return -1 if stat pointer is null
//     }

//     let stat = unsafe { &mut *stat_ptr };

//     *stat = Stat {
//         dev: 0,
//         ino: metadata.file_id as u32,
//         file_type: match metadata.file_type {
//             FileType::Directory => 1, // T_DIR
//             FileType::RegularFile => 2,      // T_FILE
//             FileType::CharDevice(_) => 3, // T_DEVICE
//             FileType::BlockDevice(_) => 3, // T_DEVICE
//             _ => 0, // Unknown type
//         },
//         nlink: 1,
//         size: metadata.size as u64,
//     };

//     0
// }

/// Linux sys_newfstatat implementation for Scarlet VFS v2
///
/// Gets file status relative to a directory file descriptor (dirfd) and path.
/// If dirfd == AT_FDCWD, uses the current working directory as the base.
/// Otherwise, resolves the base directory from the file descriptor.
/// Uses VfsManager::resolve_path_from for safe and efficient path resolution.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) on error
pub fn sys_newfstatat(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let dirfd = trapframe.get_arg(0) as i32;
    let path_ptr = task
        .vm_manager
        .translate_to_kva(trapframe.get_arg(1))
        .unwrap() as *const u8;
    let stat_ptr = task
        .vm_manager
        .translate_to_kva(trapframe.get_arg(2))
        .unwrap() as *mut u8;
    let flags = trapframe.get_arg(3) as i32;

    // Increment PC to avoid infinite loop if fstatat fails
    trapframe.increment_pc_next(task);

    // Parse path from user space
    let path_str = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((path, _)) => path,
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    let path_str = remap_shm_path(&path_str);

    // crate::println!(
    //     "sys_newfstatat: dirfd={}, path='{}', flags={:#o}",
    //     dirfd,
    //     path_str,
    //     flags
    // );

    let vfs_guard = task.vfs.read();
    let vfs = vfs_guard.as_deref().unwrap();

    // Determine base directory (entry and mount) for path resolution
    use crate::fs::vfs_v2::core::VfsFileObject;

    const AT_FDCWD: i32 = -100;
    const AT_SYMLINK_NOFOLLOW: i32 = 0x100;

    // TODO: Handle AT_SYMLINK_NOFOLLOW flag properly
    // For now, we always follow symbolic links
    let _follow_symlinks = (flags & AT_SYMLINK_NOFOLLOW) == 0;

    let (base_entry, base_mount) = if dirfd == AT_FDCWD {
        // Use current working directory as base
        vfs.get_cwd().unwrap_or_else(|| {
            let root_mount = vfs.mount_tree.root_mount.read().clone();
            (root_mount.root.clone(), root_mount)
        })
    } else {
        // Use directory file descriptor as base
        let handle = match abi.get_handle(dirfd as usize) {
            Some(h) => h,
            None => return usize::MAX,
        };
        let kernel_obj = match task.handle_table.get(handle) {
            Some(obj) => obj,
            None => return usize::MAX,
        };
        let file_obj = match kernel_obj.as_file() {
            Some(f) => f,
            None => return usize::MAX,
        };
        let vfs_file_obj = file_obj
            .as_any()
            .downcast_ref::<VfsFileObject>()
            .ok_or(())
            .unwrap();
        (
            vfs_file_obj.get_vfs_entry().clone(),
            vfs_file_obj.get_mount_point().clone(),
        )
    };

    // Resolve the path from the base directory
    match vfs.resolve_path_from(&base_entry, &base_mount, &path_str) {
        Ok((entry, _mount_point)) => {
            // Get metadata from the resolved VfsEntry
            let node = entry.node();
            match node.metadata() {
                Ok(metadata) => {
                    if stat_ptr.is_null() {
                        return usize::MAX; // Return -1 if stat pointer is null
                    }

                    let stat = unsafe { &mut *(stat_ptr as *mut LinuxStat) };
                    *stat = LinuxStat::from_metadata(&metadata);
                    // crate::println!(
                    //     "sys_newfstatat: path='{}' size={}",
                    //     path_str,
                    //     metadata.size
                    // );
                    0 // Success
                }
                Err(_) => usize::MAX, // Error getting metadata
            }
        }
        Err(_) => usize::MAX, // Error resolving path
    }
}

/// Linux sys_statx implementation for Scarlet VFS v2
///
/// Gets extended file status relative to a directory file descriptor (dirfd) and path.
/// If dirfd == AT_FDCWD, uses the current working directory as the base.
/// Supports AT_EMPTY_PATH for file-descriptor based queries.
pub fn sys_statx(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::EIO),
    };

    let dirfd = trapframe.get_arg(0) as i32;
    let pathname_ptr = trapframe.get_arg(1);
    let flags = trapframe.get_arg(2) as u32;
    let mask = trapframe.get_arg(3) as u32;
    let statx_ptr = match task.vm_manager.translate_to_kva(trapframe.get_arg(4)) {
        Some(ptr) => ptr as *mut LinuxStatx,
        None => return errno::to_result(errno::EFAULT),
    };

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    if statx_ptr.is_null() {
        return errno::to_result(errno::EFAULT);
    }

    const AT_FDCWD: i32 = -100;
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const AT_EMPTY_PATH: u32 = 0x1000;

    let path_str = match parse_c_string_from_userspace(task, pathname_ptr, MAX_PATH_LENGTH) {
        Ok(s) => s,
        Err(_) => return errno::to_result(errno::EFAULT),
    };
    let path_str = remap_shm_path(&path_str);
    // crate::println!(
    //     "sys_statx: dirfd={} path='{}' flags={:#x} mask={:#x}",
    //     dirfd,
    //     path_str,
    //     flags,
    //     mask
    // );

    let vfs = match task.vfs.read().clone() {
        Some(v) => v,
        None => return errno::to_result(errno::EIO),
    };

    // Support AT_EMPTY_PATH for stat-by-fd
    if path_str.is_empty() && (flags & AT_EMPTY_PATH) != 0 {
        let handle = match abi.get_handle(dirfd as usize) {
            Some(h) => h,
            None => return errno::to_result(errno::EBADF),
        };
        let kernel_obj = match task.handle_table.get(handle) {
            Some(obj) => obj,
            None => return errno::to_result(errno::EBADF),
        };
        let file_obj = match kernel_obj.as_file() {
            Some(f) => f,
            None => return errno::to_result(errno::EBADF),
        };

        use crate::fs::vfs_v2::core::VfsFileObject;
        if let Some(vfs_file_obj) = file_obj.as_any().downcast_ref::<VfsFileObject>() {
            let entry = vfs_file_obj.get_vfs_entry();
            let node = entry.node();
            let metadata = match node.metadata() {
                Ok(m) => m,
                Err(e) => return errno::to_result(errno::from_fs_error(&e)),
            };
            let stat = LinuxStat::from_metadata(&metadata);
            let statx_ref = unsafe { &mut *statx_ptr };
            fill_statx_from_stat(statx_ref, &stat, metadata.created_time, mask);
            // crate::println!(
            //     "sys_statx: empty_path name='{}' size={}",
            //     entry.name(),
            //     metadata.size
            // );
            return 0;
        }

        let stat = LinuxStat {
            st_dev: 0,
            st_ino: handle as u64,
            st_mode: S_IFCHR | 0o666,
            st_nlink: 1,
            st_uid: 0,
            st_gid: 0,
            st_rdev: handle as u64,
            __pad1: 0,
            st_size: 0,
            st_blksize: 4096,
            __pad2: 0,
            st_blocks: 0,
            st_atime: 0,
            st_atime_nsec: 0,
            st_mtime: 0,
            st_mtime_nsec: 0,
            st_ctime: 0,
            st_ctime_nsec: 0,
            __unused4: 0,
            __unused5: 0,
        };
        let statx_ref = unsafe { &mut *statx_ptr };
        fill_statx_from_stat(statx_ref, &stat, 0, mask);
        return 0;
    }

    // Determine base directory (entry and mount) for path resolution
    use crate::fs::vfs_v2::core::VfsFileObject;

    let (base_entry, base_mount) = if dirfd == AT_FDCWD {
        vfs.get_cwd().unwrap_or_else(|| {
            let root_mount = vfs.mount_tree.root_mount.read().clone();
            (root_mount.root.clone(), root_mount)
        })
    } else {
        let handle = match abi.get_handle(dirfd as usize) {
            Some(h) => h,
            None => return errno::to_result(errno::EBADF),
        };
        let kernel_obj = match task.handle_table.get(handle) {
            Some(obj) => obj,
            None => return errno::to_result(errno::EBADF),
        };
        let file_obj = match kernel_obj.as_file() {
            Some(f) => f,
            None => return errno::to_result(errno::ENOTDIR),
        };
        let vfs_file_obj = match file_obj.as_any().downcast_ref::<VfsFileObject>() {
            Some(vfs_obj) => vfs_obj,
            None => return errno::to_result(errno::ENOTDIR),
        };
        (
            vfs_file_obj.get_vfs_entry().clone(),
            vfs_file_obj.get_mount_point().clone(),
        )
    };

    // TODO: Handle AT_SYMLINK_NOFOLLOW flag properly; for now, always follow.
    let _follow_symlinks = (flags & AT_SYMLINK_NOFOLLOW) == 0;

    let (entry, _mount_point) = match vfs.resolve_path_from(&base_entry, &base_mount, &path_str) {
        Ok(v) => v,
        Err(e) => return errno::to_result(errno::from_fs_error(&e)),
    };

    let node = entry.node();
    let metadata = match node.metadata() {
        Ok(m) => m,
        Err(e) => return errno::to_result(errno::from_fs_error(&e)),
    };

    let stat = LinuxStat::from_metadata(&metadata);
    let statx_ref = unsafe { &mut *statx_ptr };
    fill_statx_from_stat(statx_ref, &stat, metadata.created_time, mask);
    // crate::println!(
    //     "sys_statx: path='{}' size={}",
    //     path_str,
    //     metadata.size
    // );
    0
}

#[allow(dead_code)]
pub fn sys_mkdir(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let path_ptr = task
        .vm_manager
        .translate_to_kva(trapframe.get_arg(0))
        .unwrap() as *const u8;
    let path = match get_path_str_v2(path_ptr) {
        Ok(p) => to_absolute_path_v2(&task, &p).unwrap(),
        Err(_) => return usize::MAX, // Invalid path
    };

    // Try to create the directory
    let vfs = task.vfs.read().clone().unwrap();
    match vfs.create_dir(&path) {
        Ok(_) => 0,           // Success
        Err(_) => usize::MAX, // Error
    }
}

#[allow(dead_code)]
pub fn sys_unlink(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let path_ptr = task
        .vm_manager
        .translate_to_kva(trapframe.get_arg(0))
        .unwrap() as *const u8;
    let path = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((p, _)) => to_absolute_path_v2(&task, &p).unwrap(),
        Err(_) => return usize::MAX, // Invalid path
    };

    // Try to remove the file or directory
    let vfs = task.vfs.read().clone().unwrap();
    match vfs.remove(&path) {
        Ok(_) => 0,           // Success
        Err(_) => usize::MAX, // Error
    }
}

#[allow(dead_code)]
pub fn sys_link(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let src_path_ptr = task
        .vm_manager
        .translate_to_kva(trapframe.get_arg(0))
        .unwrap() as *const u8;
    let dst_path_ptr = task
        .vm_manager
        .translate_to_kva(trapframe.get_arg(1))
        .unwrap() as *const u8;

    let src_path = match cstring_to_string(src_path_ptr, MAX_PATH_LENGTH) {
        Ok((p, _)) => to_absolute_path_v2(&task, &p).unwrap(),
        Err(_) => return usize::MAX, // Invalid path
    };

    let dst_path = match cstring_to_string(dst_path_ptr, MAX_PATH_LENGTH) {
        Ok((p, _)) => to_absolute_path_v2(&task, &p).unwrap(),
        Err(_) => return usize::MAX, // Invalid path
    };

    let vfs_guard = task.vfs.read();
    let vfs = vfs_guard.as_deref().unwrap();
    match vfs.create_hardlink(&src_path, &dst_path) {
        Ok(_) => 0, // Success
        Err(err) => {
            // Map VFS errors to appropriate errno values for Linux
            errno::to_result(errno::from_fs_error(&err))
        }
    }
}

/// Linux sys_linkat implementation for Scarlet VFS v2
///
/// Creates a hard link to an existing file. Both oldpath and newpath
/// can be relative to their respective directory file descriptors.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: olddirfd (old directory file descriptor)
///   - arg1: oldpath_ptr (pointer to source path string)
///   - arg2: newdirfd (new directory file descriptor)
///   - arg3: newpath_ptr (pointer to destination path string)
///   - arg4: flags (link flags)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) on error
pub fn sys_linkat(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let olddirfd = trapframe.get_arg(0) as i32;
    let oldpath_ptr = match task.vm_manager.translate_to_kva(trapframe.get_arg(1)) {
        Some(ptr) => ptr as *const u8,
        None => return usize::MAX,
    };
    let newdirfd = trapframe.get_arg(2) as i32;
    let newpath_ptr = match task.vm_manager.translate_to_kva(trapframe.get_arg(3)) {
        Some(ptr) => ptr as *const u8,
        None => return usize::MAX,
    };
    let flags = trapframe.get_arg(4) as i32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Parse paths from user space
    let oldpath_str = match cstring_to_string(oldpath_ptr, MAX_PATH_LENGTH) {
        Ok((path, _)) => path,
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    let newpath_str = match cstring_to_string(newpath_ptr, MAX_PATH_LENGTH) {
        Ok((path, _)) => path,
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    // Linux constants for linkat
    const AT_FDCWD: i32 = -100;
    const AT_SYMLINK_FOLLOW: i32 = 0x400;
    const AT_EMPTY_PATH: i32 = 0x1000;

    let vfs = match task.vfs.read().clone() {
        Some(v) => v,
        None => return usize::MAX,
    };

    // Determine base directory for old path resolution
    use crate::fs::vfs_v2::core::VfsFileObject;

    let (old_base_entry, old_base_mount) = if olddirfd == AT_FDCWD {
        // Use current working directory as base
        vfs.get_cwd().unwrap_or_else(|| {
            let root_mount = vfs.mount_tree.root_mount.read().clone();
            (root_mount.root.clone(), root_mount)
        })
    } else {
        // Use directory file descriptor as base
        let handle = match abi.get_handle(olddirfd as usize) {
            Some(h) => h,
            None => return usize::MAX,
        };
        let kernel_obj = match task.handle_table.get(handle) {
            Some(obj) => obj,
            None => return usize::MAX,
        };
        let file_obj = match kernel_obj.as_file() {
            Some(f) => f,
            None => return usize::MAX,
        };
        let vfs_file_obj = file_obj
            .as_any()
            .downcast_ref::<VfsFileObject>()
            .ok_or(())
            .unwrap();
        (
            vfs_file_obj.get_vfs_entry().clone(),
            vfs_file_obj.get_mount_point().clone(),
        )
    };

    // Determine base directory for new path resolution
    let (_new_base_entry, _new_base_mount) = if newdirfd == AT_FDCWD {
        // Use current working directory as base
        vfs.get_cwd().unwrap_or_else(|| {
            let root_mount = vfs.mount_tree.root_mount.read().clone();
            (root_mount.root.clone(), root_mount)
        })
    } else {
        // Use directory file descriptor as base
        let handle = match abi.get_handle(newdirfd as usize) {
            Some(h) => h,
            None => return usize::MAX,
        };
        let kernel_obj = match task.handle_table.get(handle) {
            Some(obj) => obj,
            None => return usize::MAX,
        };
        let file_obj = match kernel_obj.as_file() {
            Some(f) => f,
            None => return usize::MAX,
        };
        let vfs_file_obj = file_obj
            .as_any()
            .downcast_ref::<VfsFileObject>()
            .ok_or(())
            .unwrap();
        (
            vfs_file_obj.get_vfs_entry().clone(),
            vfs_file_obj.get_mount_point().clone(),
        )
    };

    // Resolve the source path to verify it exists
    let _source_entry = match vfs.resolve_path_from(&old_base_entry, &old_base_mount, &oldpath_str)
    {
        Ok((entry, _mount_point)) => entry,
        Err(_) => return usize::MAX, // Source file doesn't exist
    };

    // For now, we'll implement a simplified version using absolute paths
    // since VFS v2 may not have direct hard link support yet

    // Convert paths to absolute paths
    let _old_absolute_path = if oldpath_str.starts_with('/') {
        oldpath_str.to_string()
    } else {
        match to_absolute_path_v2(&task, &oldpath_str) {
            Ok(p) => p,
            Err(_) => return usize::MAX,
        }
    };

    let _new_absolute_path = if newpath_str.starts_with('/') {
        newpath_str.to_string()
    } else {
        match to_absolute_path_v2(&task, &newpath_str) {
            Ok(p) => p,
            Err(_) => return usize::MAX,
        }
    };

    // Get mutable VFS reference for link creation
    let _vfs_mut = match task.vfs.write().clone() {
        Some(v) => v,
        None => return usize::MAX,
    };

    // TODO: Handle flags properly
    // AT_SYMLINK_FOLLOW: follow symbolic links in oldpath
    // AT_EMPTY_PATH: allow empty oldpath if olddirfd refers to a file
    let _follow_symlinks = (flags & AT_SYMLINK_FOLLOW) != 0;
    let _empty_path = (flags & AT_EMPTY_PATH) != 0;

    // Try to create the hard link
    // Note: This is a simplified implementation. A full implementation would:
    // 1. Check if source and destination are on the same filesystem
    // 2. Verify the source is not a directory (unless allowed)
    // 3. Handle proper hard link semantics
    // 4. Update inode reference counts

    // For now, we'll return success as a stub implementation
    // since VFS v2 might not support true hard links yet.
    // Real hard link functionality would require:
    // - Filesystem-level support for hard links
    // - Inode reference counting
    // - Cross-filesystem link prevention

    // Stub implementation: just return success
    // This prevents applications from crashing when they use linkat
    // but doesn't provide true hard link semantics
    0 // Success (stub implementation)
}

/// VFS v2 helper function for path absolutization using VfsManager
fn to_absolute_path_v2(task: &crate::task::Task, path: &str) -> Result<String, ()> {
    if path.starts_with('/') {
        Ok(path.to_string())
    } else {
        let vfs_guard = task.vfs.read();
        let vfs = vfs_guard.as_ref().ok_or(())?;
        Ok(vfs.resolve_path_to_absolute(path))
    }
}

/// Helper function to replace the missing get_path_str function
/// TODO: This should be moved to a shared helper when VFS v2 provides public API
fn get_path_str_v2(ptr: *const u8) -> Result<String, ()> {
    const MAX_PATH_LENGTH: usize = 1024; // Match the global constant
    cstring_to_string(ptr, MAX_PATH_LENGTH)
        .map(|(s, _)| s)
        .map_err(|_| ())
}

/// Linux ioctl system call implementation
///
/// This system call performs device-specific control operations on file descriptors,
/// similar to the POSIX ioctl system call. It acts as a bridge between Linux ABI
/// and Scarlet's native HandleControl functionality.
///
/// # Arguments
/// - fd: File descriptor
/// - request: Control operation command
/// - arg: Argument for the control operation (often a pointer)
///
/// # Returns
/// - 0 or positive value on success
/// - usize::MAX on error (-1 in Linux)
pub fn sys_ioctl(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let request = trapframe.get_arg(1) as u32;
    let arg = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Get handle from Linux fd
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return usize::MAX, // Invalid file descriptor
    };

    // Get the kernel object from the handle table
    let kernel_object = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid handle
    };

    // Dispatch hypervisor (KVM) ioctls before capability-based routing
    #[cfg(feature = "hypervisor")]
    match &kernel_object {
        crate::object::KernelObject::HypervisorVm(vm) => {
            let result = crate::abi::linux::device::kvm::handle_vm_ioctl(request, arg, vm, abi);
            return match result {
                Ok(Some(ret)) => ret,
                Ok(None) => errno::to_result(errno::ENOTTY),
                Err(_) => errno::to_result(errno::EINVAL),
            };
        }
        crate::object::KernelObject::HypervisorVcpu(vcpu) => {
            let result =
                crate::abi::linux::device::kvm::handle_vcpu_ioctl(request, arg, vcpu, trapframe);
            return match result {
                Ok(Some(ret)) => ret,
                Ok(None) => errno::to_result(errno::ENOTTY),
                Err(_) => errno::to_result(errno::EINVAL),
            };
        }
        _ => {}
    }

    // Determine device capabilities for per-device translation
    let mut caps: Option<&'static [DeviceCapability]> = None;
    if let Some(file_obj) = kernel_object.as_file() {
        if let Ok(metadata) = file_obj.metadata() {
            if let FileType::CharDevice(info) = metadata.file_type {
                if let Some(dev) = DeviceManager::get_manager().get_device(info.device_id) {
                    #[cfg(feature = "hypervisor")]
                    if dev.name() == crate::abi::linux::device::kvm::KVM_DEVICE_NAME {
                        let result =
                            crate::abi::linux::device::kvm::handle_system_ioctl(request, arg, abi);
                        return match result {
                            Ok(Some(ret)) => ret,
                            Ok(None) => errno::to_result(errno::ENOTTY),
                            Err(_) => errno::to_result(errno::EINVAL),
                        };
                    }
                    caps = Some(dev.capabilities());
                }
            }
        }
    }

    if let Some(caps) = caps {
        // TTY translation
        if caps.iter().any(|c| *c == DeviceCapability::Tty) {
            match crate::abi::linux::device::tty::handle_ioctl(request, arg, &kernel_object) {
                Ok(Some(ret)) => return ret,
                Ok(None) => {
                    // Do NOT pass through unknown TTY ioctls to device-specific control.
                    // Return ENOTTY to match Linux behavior and avoid accidental derefs.
                    return errno::to_result(errno::ENOTTY);
                }
                Err(_) => return errno::to_result(errno::ENOTTY),
            }
        }
        // Future: match on other capabilities here
    }

    // Default path: pass-through to ControlOps if available
    let result = match kernel_object.as_control() {
        Some(control_ops) => control_ops.control(request, arg),
        None => Err("Inappropriate ioctl for device"),
    };

    match result {
        Ok(value) => {
            if value >= 0 {
                value as usize
            } else {
                errno::to_result(errno::EINVAL)
            }
        }
        Err(_) => errno::to_result(errno::ENOTTY),
    }
}

/// Linux execve system call implementation
///
/// This system call executes a program specified by the given path, replacing the
/// current process image with a new one. It also allows passing arguments and
/// environment variables to the new program.
///
/// # Arguments
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///
/// # Returns
/// - 0 on success
/// - usize::MAX (Linux -1) on error
pub fn sys_execve(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();

    // Increment PC to avoid infinite loop if execve fails
    trapframe.increment_pc_next(task);

    // Get arguments from trapframe
    let path_ptr = trapframe.get_arg(0);
    let argv_ptr = trapframe.get_arg(1);
    let envp_ptr = trapframe.get_arg(2);

    // Parse path
    let path_str = match parse_c_string_from_userspace(task, path_ptr, MAX_PATH_LENGTH) {
        Ok(path) => match to_absolute_path_v2(&task, &path) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX, // Path error
        },
        Err(_) => return usize::MAX, // Path parsing error
    };

    // Parse argv
    let argv_strings =
        match parse_string_array_from_userspace(task, argv_ptr, MAX_ARG_COUNT, MAX_PATH_LENGTH) {
            Ok(args) => args,
            Err(_) => return usize::MAX, // argv parsing error
        };

    // Parse envp (optional)
    let envp_strings =
        match parse_string_array_from_userspace(task, envp_ptr, MAX_ARG_COUNT, MAX_PATH_LENGTH) {
            Ok(envs) => envs,
            Err(_) => return usize::MAX, // envp parsing error
        };

    crate::println!(
        "sys_execve: path: {}, argv: {:?}, envp: {:?}",
        path_str,
        argv_strings,
        envp_strings
    );

    // Debug: Print each argv element individually
    for (i, arg) in argv_strings.iter().enumerate() {
        crate::println!("  argv[{}]: \"{}\" (len={})", i, arg, arg.len());
        for (j, byte) in arg.bytes().enumerate() {
            if byte < 32 || byte > 126 {
                crate::println!("    byte[{}]: 0x{:02x} (non-printable)", j, byte);
            }
        }
    }

    // Convert Vec<String> to Vec<&str> for TransparentExecutor
    let argv_refs: Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();
    let envp_refs: Vec<&str> = envp_strings.iter().map(|s| s.as_str()).collect();

    // Use TransparentExecutor for cross-ABI execution
    match TransparentExecutor::execute_binary(
        &path_str, &argv_refs, &envp_refs, task, trapframe, false,
    ) {
        Ok(_) => {
            // execve normally should not return on success - the process is replaced
            // However, if ABI module sets trapframe return value and returns here,
            // we should respect that value instead of hardcoding 0
            trapframe.get_return_value()
        }
        Err(_) => {
            // Execution failed - return error code
            // The trap handler will automatically set trapframe return value from our return
            usize::MAX // Error return value
        }
    }
}

/// Linux iovec structure for vectored I/O operations
/// This structure matches the Linux kernel's definition for struct iovec
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IoVec {
    /// Base address of the buffer
    pub iov_base: *mut u8,
    /// Length of the buffer
    pub iov_len: usize,
}

/// Linux sys_fcntl implementation for Scarlet VFS v2
/// Currently provides basic logging of commands to understand usage patterns
///
/// This is a minimal implementation that logs the fcntl commands being used
/// to help understand what functionality needs to be implemented.
const LOG_FCNTL: bool = false;
pub fn sys_fcntl(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let cmd = trapframe.get_arg(1) as u32;
    let arg = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Log the fcntl command to understand usage patterns
    match cmd {
        F_DUPFD => {
            if LOG_FCNTL {
                crate::println!(
                    "[sys_fcntl] F_DUPFD: fd={}, arg={} - NOT IMPLEMENTED",
                    fd,
                    arg
                );
            }
            if fd >= super::MAX_FDS {
                return errno::to_result(errno::EBADF);
            }
            let min_fd = arg as usize;
            if min_fd >= super::MAX_FDS {
                return errno::to_result(errno::EMFILE);
            }
            let old_handle = match abi.get_handle(fd) {
                Some(h) => h,
                None => return errno::to_result(errno::EBADF),
            };
            let kernel_obj = match task.handle_table.clone_for_dup(old_handle) {
                Some(obj) => obj,
                None => return errno::to_result(errno::EBADF),
            };
            let mut new_fd = None;
            for candidate in min_fd..super::MAX_FDS {
                if abi.get_handle(candidate).is_none() {
                    new_fd = Some(candidate);
                    break;
                }
            }
            let new_fd = match new_fd {
                Some(fd) => fd,
                None => return errno::to_result(errno::EMFILE),
            };
            let new_handle = match task.handle_table.insert(kernel_obj) {
                Ok(handle) => handle,
                Err(_) => return errno::to_result(errno::ENFILE),
            };
            if abi.allocate_specific_fd(new_fd, new_handle as u32).is_err() {
                let _ = task.handle_table.remove(new_handle as u32);
                return errno::to_result(errno::EMFILE);
            }
            if let Some(flags) = abi.get_file_status_flags(fd) {
                let _ = abi.set_file_status_flags(new_fd, flags);
            }
            let _ = abi.set_fd_flags(new_fd, 0);
            return new_fd;
        }
        F_GETFD => {
            // Get file descriptor flags (IMPLEMENTED)
            if let Some(_handle) = abi.get_handle(fd) {
                if let Some(flags) = abi.get_fd_flags(fd) {
                    return flags as usize; // Return the flags
                } else {
                    return usize::MAX; // Invalid file descriptor
                }
            } else {
                return usize::MAX; // Invalid file descriptor
            }
        }
        F_SETFD => {
            // Set file descriptor flags (IMPLEMENTED)
            if let Some(_handle) = abi.get_handle(fd) {
                match abi.set_fd_flags(fd, arg as u32) {
                    Ok(()) => return 0,          // Success
                    Err(_) => return usize::MAX, // Error
                }
            } else {
                return usize::MAX; // Invalid file descriptor
            }
        }
        F_GETFL => {
            if let Some(_handle) = abi.get_handle(fd) {
                if let Some(flags) = abi.get_file_status_flags(fd) {
                    return flags as usize;
                } else {
                    return usize::MAX;
                }
            } else {
                return usize::MAX;
            }
        }
        F_SETFL => {
            // Only honor a subset (currently O_NONBLOCK). Preserve other bits as-is.
            if let Some(_handle) = abi.get_handle(fd) {
                // Get current status flags and update O_NONBLOCK bit only
                let curr = abi.get_file_status_flags(fd).unwrap_or(0);
                let mut new_flags = curr;
                const O_NONBLOCK_U32: u32 = O_NONBLOCK as u32;
                if (arg as u32) & O_NONBLOCK_U32 != 0 {
                    new_flags |= O_NONBLOCK_U32;
                } else {
                    new_flags &= !O_NONBLOCK_U32;
                }
                if abi.set_file_status_flags(fd, new_flags).is_err() {
                    return usize::MAX;
                }
                // Also propagate O_NONBLOCK to the object-level Selectable if available
                if let Some(handle) = abi.get_handle(fd) {
                    if let Some(obj) = task.handle_table.get(handle) {
                        if let Some(sel) = obj.as_selectable() {
                            sel.set_nonblocking(((new_flags as i32) & O_NONBLOCK) != 0);
                        }
                    }
                }

                return 0;
            } else {
                return usize::MAX;
            }
        }
        F_GETLK => {
            if LOG_FCNTL {
                crate::println!(
                    "[sys_fcntl] F_GETLK: fd={}, lock_ptr={:#x} - NOT IMPLEMENTED",
                    fd,
                    arg
                );
            }
            // TODO: Implement file locking
        }
        F_SETLK => {
            if LOG_FCNTL {
                crate::println!(
                    "[sys_fcntl] F_SETLK: fd={}, lock_ptr={:#x} - NOT IMPLEMENTED",
                    fd,
                    arg
                );
            }
            // TODO: Implement file locking
        }
        F_SETLKW => {
            if LOG_FCNTL {
                crate::println!(
                    "[sys_fcntl] F_SETLKW: fd={}, lock_ptr={:#x} - NOT IMPLEMENTED",
                    fd,
                    arg
                );
            }
            // TODO: Implement file locking
        }
        F_SETOWN => {
            if LOG_FCNTL {
                crate::println!(
                    "[sys_fcntl] F_SETOWN: fd={}, owner={} - NOT IMPLEMENTED",
                    fd,
                    arg
                );
            }
            // TODO: Implement F_SETOWN
        }
        F_GETOWN => {
            if LOG_FCNTL {
                crate::println!("[sys_fcntl] F_GETOWN: fd={} - NOT IMPLEMENTED", fd);
            }
            // TODO: Implement F_GETOWN
        }
        F_SETSIG => {
            if LOG_FCNTL {
                crate::println!(
                    "[sys_fcntl] F_SETSIG: fd={}, sig={} - NOT IMPLEMENTED",
                    fd,
                    arg
                );
            }
            // TODO: Implement F_SETSIG
        }
        F_GETSIG => {
            if LOG_FCNTL {
                crate::println!("[sys_fcntl] F_GETSIG: fd={} - NOT IMPLEMENTED", fd);
            }
            // TODO: Implement F_GETSIG
        }
        F_SETLEASE => {
            if LOG_FCNTL {
                crate::println!(
                    "[sys_fcntl] F_SETLEASE: fd={}, lease_type={} - NOT IMPLEMENTED",
                    fd,
                    arg
                );
            }
            // TODO: Implement F_SETLEASE
        }
        F_GETLEASE => {
            if LOG_FCNTL {
                crate::println!("[sys_fcntl] F_GETLEASE: fd={} - NOT IMPLEMENTED", fd);
            }
            // TODO: Implement F_GETLEASE
        }
        F_NOTIFY => {
            if LOG_FCNTL {
                crate::println!(
                    "[sys_fcntl] F_NOTIFY: fd={}, events={:#x} - NOT IMPLEMENTED",
                    fd,
                    arg
                );
            }
            // TODO: Implement F_NOTIFY
        }
        F_DUPFD_CLOEXEC => {
            if LOG_FCNTL {
                crate::println!(
                    "[sys_fcntl] F_DUPFD_CLOEXEC: fd={}, arg={} - NOT IMPLEMENTED",
                    fd,
                    arg
                );
            }
            if fd >= super::MAX_FDS {
                return errno::to_result(errno::EBADF);
            }
            let min_fd = arg as usize;
            if min_fd >= super::MAX_FDS {
                return errno::to_result(errno::EMFILE);
            }
            let old_handle = match abi.get_handle(fd) {
                Some(h) => h,
                None => return errno::to_result(errno::EBADF),
            };
            let kernel_obj = match task.handle_table.clone_for_dup(old_handle) {
                Some(obj) => obj,
                None => return errno::to_result(errno::EBADF),
            };
            let mut new_fd = None;
            for candidate in min_fd..super::MAX_FDS {
                if abi.get_handle(candidate).is_none() {
                    new_fd = Some(candidate);
                    break;
                }
            }
            let new_fd = match new_fd {
                Some(fd) => fd,
                None => return errno::to_result(errno::EMFILE),
            };
            let new_handle = match task.handle_table.insert(kernel_obj) {
                Ok(handle) => handle,
                Err(_) => return errno::to_result(errno::ENFILE),
            };
            if abi.allocate_specific_fd(new_fd, new_handle as u32).is_err() {
                let _ = task.handle_table.remove(new_handle as u32);
                return errno::to_result(errno::EMFILE);
            }
            if let Some(flags) = abi.get_file_status_flags(fd) {
                let _ = abi.set_file_status_flags(new_fd, flags);
            }
            let _ = abi.set_fd_flags(new_fd, FD_CLOEXEC);
            return new_fd;
        }
        _ => {
            if LOG_FCNTL {
                crate::println!(
                    "[sys_fcntl] UNKNOWN_CMD: fd={}, cmd={}, arg={:#x} - NOT IMPLEMENTED",
                    fd,
                    cmd,
                    arg
                );
            }
        }
    }

    // All unimplemented commands return ENOSYS (already logged above)
    errno::to_result(errno::ENOSYS)
}

/// Linux sys_flock - Apply or remove an advisory lock on an open file
///
/// Apply or remove an advisory lock on the open file specified by fd.
/// This is a simplified implementation that always succeeds.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: fd (file descriptor)
///   - arg1: operation (LOCK_SH, LOCK_EX, LOCK_UN, etc.)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) on error
pub fn sys_flock(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let fd = trapframe.get_arg(0) as i32;
    let _operation = trapframe.get_arg(1) as i32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Verify fd is valid
    if abi.get_handle(fd as usize).is_none() {
        return usize::MAX;
    }

    // Simplified implementation: always succeed
    // Real flock would require managing lock state per file descriptor
    // For Wayland SHM operations, advisory locks aren't critical
    0
}

/// Linux sys_fallocate - Manipulate file space
///
/// This is used by glib/Wayland to extend shared memory file size.
/// When mode is 0 (default), it extends the file to at least offset + len.
///
/// Arguments:
/// - fd: File descriptor
/// - mode: Operation mode (0 = allocate, FALLOC_FL_KEEP_SIZE, etc.)
/// - offset: Starting offset
/// - len: Length of the allocation
///
/// Returns:
/// - 0 on success
/// - negative errno on failure
pub fn sys_fallocate(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    use super::errno;

    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::EFAULT),
    };

    let fd = trapframe.get_arg(0) as usize;
    let mode = trapframe.get_arg(1) as i32;
    let offset = trapframe.get_arg(2) as i64;
    let len = trapframe.get_arg(3) as i64;

    trapframe.increment_pc_next(task);

    if offset < 0 || len <= 0 {
        return errno::to_result(errno::EINVAL);
    }

    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return errno::to_result(errno::EBADF),
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return errno::to_result(errno::EBADF),
    };

    // Handle SharedMemory (memfd)
    if let Some(shared_memory) = kernel_obj.as_shared_memory() {
        // FALLOC_FL_KEEP_SIZE = 0x01 - don't change file size
        const FALLOC_FL_KEEP_SIZE: i32 = 0x01;

        let new_size = (offset + len) as usize;
        let current_size = shared_memory.size();

        // If mode doesn't have KEEP_SIZE, extend the file
        if (mode & FALLOC_FL_KEEP_SIZE) == 0 && new_size > current_size {
            if let Err(_e) = shared_memory.resize(new_size) {
                return errno::to_result(errno::ENOSPC);
            }
        }

        return 0;
    }

    // For regular files, just succeed (no-op for now)
    // Real implementation would preallocate disk space
    0
}

/// Linux struct linux_dirent64 (for getdents64 syscall)
#[repr(C)]
pub struct LinuxDirent64 {
    pub d_ino: u64,
    pub d_off: i64,
    pub d_reclen: u16,
    pub d_type: u8,
    pub d_name: [u8; 256], // Linux allows up to 255 + null
}

impl LinuxDirent64 {
    pub fn new(entry: &DirectoryEntry, d_off: i64) -> Self {
        let mut d_name = [0u8; 256];
        let name_len = entry.name_len as usize;
        d_name[..name_len].copy_from_slice(&entry.name[..name_len]);
        d_name[name_len] = 0; // null-terminated
        Self {
            d_ino: entry.file_id,
            d_off,
            d_reclen: (core::mem::size_of::<u64>()
                + core::mem::size_of::<i64>()
                + core::mem::size_of::<u16>()
                + core::mem::size_of::<u8>()
                + name_len
                + 1) as u16,
            d_type: entry.file_type,
            d_name,
        }
    }
    pub fn as_bytes(&self) -> &[u8] {
        let len = self.d_reclen as usize;
        unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, len) }
    }
}

/// getdents64 syscall implementation
pub fn sys_getdents64(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let buf_ptr = task
        .vm_manager
        .translate_to_kva(trapframe.get_arg(1))
        .unwrap() as *mut u8;
    let buf_size = trapframe.get_arg(2) as usize;
    trapframe.increment_pc_next(task);

    // Get handle from Linux fd
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return usize::MAX,
    };
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX,
    };
    let stream = match kernel_obj.as_stream() {
        Some(s) => s,
        None => return usize::MAX,
    };

    let mut dir_buffer = vec![0u8; core::mem::size_of::<DirectoryEntry>()];
    let mut written = 0usize;
    let mut d_off = 0i64;
    while written + core::mem::size_of::<LinuxDirent64>() <= buf_size {
        match stream.read(&mut dir_buffer) {
            Ok(n) if n == dir_buffer.len() => {
                if let Some(entry) = DirectoryEntry::parse(&dir_buffer) {
                    let dirent = LinuxDirent64::new(&entry, d_off);
                    let dirent_bytes = dirent.as_bytes();
                    if written + dirent_bytes.len() > buf_size {
                        break;
                    }
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            dirent_bytes.as_ptr(),
                            buf_ptr.add(written),
                            dirent_bytes.len(),
                        );
                    }
                    written += dirent_bytes.len();
                    d_off += 1;
                } else {
                    break;
                }
            }
            Ok(0) => break, // EOF
            Ok(_) => break, // partial read, treat as error/EOF
            Err(StreamError::EndOfStream) => break,
            Err(StreamError::WouldBlock) => {
                schedule(trapframe);
                return usize::MAX;
            }
            Err(_) => break,
        }
    }
    written
}

/// Linux readv system call implementation
///
/// This system call reads data into multiple buffers (iovec) in a single call.
///
/// # Arguments
/// - fd: File descriptor
/// - iovec: Array of iovec structures
/// - iovcnt: Number of elements in the array
///
/// # Returns
/// - On success: number of bytes read
/// - On error: usize::MAX
pub fn sys_readv(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let iovec_ptr = trapframe.get_arg(1);
    let iovcnt = trapframe.get_arg(2) as usize;
    trapframe.increment_pc_next(task);

    if iovcnt == 0 {
        return 0;
    }
    const IOV_MAX: usize = 1024;
    if iovcnt > IOV_MAX {
        return usize::MAX;
    }
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return usize::MAX,
    };
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX,
    };
    let stream = match kernel_obj.as_stream() {
        Some(s) => s,
        None => return usize::MAX, // Not a stream object
    };

    let nonblocking = abi
        .get_file_status_flags(fd)
        .map(|f| ((f as i32) & O_NONBLOCK) != 0)
        .unwrap_or(false);
    let iovec_vaddr = match task.vm_manager.translate_to_kva(iovec_ptr) {
        Some(addr) => addr as *mut IoVec,
        None => return usize::MAX,
    };
    if iovec_vaddr.is_null() {
        return usize::MAX;
    }
    let iovecs = unsafe { core::slice::from_raw_parts_mut(iovec_vaddr, iovcnt) };
    let mut total_read = 0usize;
    for iovec in iovecs.iter_mut() {
        if iovec.iov_len == 0 {
            continue;
        }
        let buf_vaddr = match task.vm_manager.translate_to_kva(iovec.iov_base as usize) {
            Some(addr) => addr as *mut u8,
            None => return usize::MAX,
        };
        if buf_vaddr.is_null() {
            return usize::MAX;
        }
        let buffer = unsafe { core::slice::from_raw_parts_mut(buf_vaddr, iovec.iov_len) };
        match stream.read(buffer) {
            Ok(n) => {
                total_read = total_read.saturating_add(n);
                // If partial read occurred, stop processing remaining vectors
                // This matches Linux behavior for readv
                if n < iovec.iov_len {
                    break;
                }
            }
            Err(StreamError::EndOfStream) => break,
            Err(StreamError::WouldBlock) => {
                if nonblocking {
                    if total_read == 0 {
                        return errno::to_result(errno::EAGAIN);
                    } else {
                        break;
                    }
                } else {
                    schedule(trapframe);
                    return usize::MAX;
                }
            }
            Err(_) => {
                if total_read == 0 {
                    return usize::MAX;
                } else {
                    break;
                }
            }
        }
    }
    total_read
}

/// Linux sys_fsync system call implementation (stub)
/// Synchronize a file's in-core state with storage device
///
/// Arguments:
/// - fd: File descriptor to synchronize
///
/// Returns:
/// - 0 on success
/// - usize::MAX on error
pub fn sys_fsync(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let _fd = trapframe.get_arg(0);
    trapframe.increment_pc_next(task);

    // TODO: Implement actual file synchronization
    // For now, return success as a stub implementation
    0
}

/// Linux sys_ftruncate implementation
///
/// Truncate a file to a specified length using a file descriptor.
///
/// Arguments:
/// - fd: File descriptor to truncate
/// - length: New file length
///
/// Returns:
/// - 0 on success
/// - negative errno on failure
pub fn sys_ftruncate(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::EFAULT),
    };

    let fd = trapframe.get_arg(0) as usize;
    let length = trapframe.get_arg(1) as i64;

    trapframe.increment_pc_next(task);

    if length < 0 {
        return errno::to_result(errno::EINVAL);
    }

    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return errno::to_result(errno::EBADF),
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return errno::to_result(errno::EBADF),
    };

    if let Some(shared_memory) = kernel_obj.as_shared_memory() {
        if let Err(_err) = shared_memory.resize(length as usize) {
            return errno::to_result(errno::EINVAL);
        }
        return 0;
    }

    let file_obj = match kernel_obj.as_file() {
        Some(f) => f,
        None => return errno::to_result(errno::EINVAL),
    };

    let mut is_shm = false;
    let mut shm_path = None;
    if let Some(vfs_obj) = file_obj
        .as_any()
        .downcast_ref::<crate::fs::vfs_v2::core::VfsFileObject>()
    {
        let path = vfs_obj.get_original_path();
        if path.contains("wl_shm-") {
            is_shm = true;
            shm_path = Some(path);
            crate::println!("sys_ftruncate: shm path='{}' len={}", path, length);
        }
    }

    match file_obj.truncate(length as u64) {
        Ok(()) => 0,
        Err(err) => {
            if is_shm {
                if let Ok(meta) = file_obj.metadata() {
                    crate::println!(
                        "sys_ftruncate: shm truncate failed len={} file_type={:?} size={}",
                        length,
                        meta.file_type,
                        meta.size
                    );
                }
                crate::println!("sys_ftruncate: shm truncate error={:?}", err);
            } else if length > 0 {
                let kind = match kernel_obj {
                    crate::object::KernelObject::File(_) => "File",
                    crate::object::KernelObject::Pipe(_) => "Pipe",
                    crate::object::KernelObject::Counter(_) => "Counter",
                    crate::object::KernelObject::EventChannel(_) => "EventChannel",
                    crate::object::KernelObject::EventSubscription(_) => "EventSubscription",
                    crate::object::KernelObject::SharedMemory(_) => "SharedMemory",
                    #[cfg(feature = "network")]
                    crate::object::KernelObject::Socket(_) => "Socket",
                    #[cfg(feature = "hypervisor")]
                    crate::object::KernelObject::HypervisorVm(_) => "HypervisorVm",
                    #[cfg(feature = "hypervisor")]
                    crate::object::KernelObject::HypervisorVcpu(_) => "HypervisorVcpu",
                };
                crate::println!(
                    "sys_ftruncate: fd={} kind={} path={:?} len={} err={:?}",
                    fd,
                    kind,
                    shm_path,
                    length,
                    err
                );
            }
            errno::to_result(errno::EIO)
        }
    }
}

/// Linux sys_faccessat implementation (dummy: always returns 0)
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///
/// Returns:
/// - 0 (success)
pub fn sys_faccessat(_abi: &mut LinuxRiscv64Abi, trapframe: &mut crate::arch::Trapframe) -> usize {
    let task = crate::task::mytask().unwrap();
    trapframe.increment_pc_next(task);

    let dirfd = trapframe.get_arg(0) as i32;
    let path_ptr = match task.vm_manager.translate_to_kva(trapframe.get_arg(1)) {
        Some(ptr) => ptr as *const u8,
        None => return usize::MAX,
    };
    let mode = trapframe.get_arg(2) as i32;
    let path_str = match get_path_str_v2(path_ptr) {
        Ok(p) => p,
        Err(_) => return usize::MAX,
    };

    // crate::println!(
    //     "sys_faccessat: epc={:#x}, dirfd={}, path='{}', mode={:#o}",
    //     trapframe.epc,
    //     dirfd,
    //     path_str,
    //     mode
    // );

    0
}

/// Linux faccessat2 system call (syscall 439)
///
/// Checks user's permissions for a file. Similar to faccessat but with
/// additional flag support (AT_EACCESS, AT_SYMLINK_NOFOLLOW).
///
/// Signature: int faccessat2(int dirfd, const char *pathname, int mode, int flags);
///
/// Returns:
/// - 0 (success)
pub fn sys_faccessat2(_abi: &mut LinuxRiscv64Abi, trapframe: &mut crate::arch::Trapframe) -> usize {
    let task = crate::task::mytask().unwrap();
    trapframe.increment_pc_next(task);

    let dirfd = trapframe.get_arg(0) as i32;
    let path_ptr = match task.vm_manager.translate_to_kva(trapframe.get_arg(1)) {
        Some(ptr) => ptr as *const u8,
        None => return usize::MAX,
    };
    let mode = trapframe.get_arg(2) as i32;
    let flags = trapframe.get_arg(3) as i32;
    let path_str = match get_path_str_v2(path_ptr) {
        Ok(p) => p,
        Err(_) => return usize::MAX,
    };

    // crate::println!(
    //     "sys_faccessat2: epc={:#x}, dirfd={}, path='{}', mode={:#o}, flags={:#x}",
    //     trapframe.epc,
    //     dirfd,
    //     path_str,
    //     mode,
    //     flags
    // );

    0
}

/// Linux sys_mkdirat implementation
///
/// Currently supports only AT_FDCWD (current working directory) as dirfd.
///
pub fn sys_mkdirat(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };
    trapframe.increment_pc_next(task);
    let dirfd = trapframe.get_arg(0) as i32;
    let path_ptr = match task.vm_manager.translate_to_kva(trapframe.get_arg(1)) {
        Some(ptr) => ptr as *const u8,
        None => return usize::MAX,
    };
    let path = match cstring_to_string(path_ptr, 128) {
        Ok((p, _)) => p,
        Err(_) => return usize::MAX,
    };
    // NOTE: Currently only AT_FDCWD is supported
    if dirfd != -100 {
        // AT_FDCWD
        return usize::MAX;
    }

    let abs_path = match to_absolute_path_v2(&task, &path) {
        Ok(p) => p,
        Err(_) => return usize::MAX,
    };
    let vfs = match task.vfs.write().clone() {
        Some(v) => v,
        None => return usize::MAX,
    };
    match vfs.create_dir(&abs_path) {
        Ok(_) => 0,
        Err(_) => usize::MAX,
    }
}

/// Linux sys_newfstat implementation for Scarlet VFS v2
///
/// Gets file status information from a file descriptor.
/// This is equivalent to stat() but uses a file descriptor instead of a path.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: fd (file descriptor)
///   - arg1: stat_ptr (pointer to LinuxStat structure)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) on error
pub fn sys_newfstat(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let fd = trapframe.get_arg(0) as i32;
    let stat_ptr = match task.vm_manager.translate_to_kva(trapframe.get_arg(1)) {
        Some(ptr) => ptr as *mut u8,
        None => return usize::MAX,
    };

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Validate arguments
    if stat_ptr.is_null() {
        return usize::MAX; // Return -1 if stat pointer is null
    }

    // Get handle from file descriptor
    let handle = match abi.get_handle(fd as usize) {
        Some(h) => h,
        None => return usize::MAX, // Invalid file descriptor
    };

    // Get kernel object from handle
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Handle not found
    };

    // Get file object
    let file_obj = match kernel_obj.as_file() {
        Some(f) => f,
        None => return usize::MAX, // Not a file object
    };

    // Get VFS file object to access metadata
    use crate::fs::vfs_v2::core::VfsFileObject;
    let vfs_file_obj = match file_obj.as_any().downcast_ref::<VfsFileObject>() {
        Some(vfs_obj) => vfs_obj,
        None => {
            // For non-VFS files (like devices), create a basic stat with minimal info
            let stat = unsafe { &mut *(stat_ptr as *mut LinuxStat) };
            *stat = LinuxStat {
                st_dev: 0,
                st_ino: handle as u64,    // Use handle as inode
                st_mode: S_IFCHR | 0o666, // Character device with rw-rw-rw- permissions
                st_nlink: 1,
                st_uid: 0,
                st_gid: 0,
                st_rdev: handle as u64,
                __pad1: 0,
                st_size: 0,
                st_blksize: 4096,
                __pad2: 0,
                st_blocks: 0,
                st_atime: 0,
                st_atime_nsec: 0,
                st_mtime: 0,
                st_mtime_nsec: 0,
                st_ctime: 0,
                st_ctime_nsec: 0,
                __unused4: 0,
                __unused5: 0,
            };
            return 0; // Success
        }
    };

    // Get VFS entry and metadata
    let entry = vfs_file_obj.get_vfs_entry();
    let node = entry.node();

    match node.metadata() {
        Ok(metadata) => {
            let stat = unsafe { &mut *(stat_ptr as *mut LinuxStat) };
            *stat = LinuxStat::from_metadata(&metadata);
            // crate::println!(
            //     "sys_newfstat: fd={} name='{}' size={}",
            //     fd,
            //     entry.name(),
            //     metadata.size
            // );
            0 // Success
        }
        Err(_) => usize::MAX, // Error getting metadata
    }
}

/// Linux sys_unlinkat implementation for Scarlet VFS v2
///
/// Removes a file or directory relative to a directory file descriptor.
/// If dirfd == AT_FDCWD, uses the current working directory as the base.
/// Otherwise, resolves the base directory from the file descriptor.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: dirfd (directory file descriptor)
///   - arg1: path_ptr (pointer to path string)
///   - arg2: flags (unlink flags)
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) on error
pub fn sys_unlinkat(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let dirfd = trapframe.get_arg(0) as i32;
    let path_ptr = match task.vm_manager.translate_to_kva(trapframe.get_arg(1)) {
        Some(ptr) => ptr as *const u8,
        None => return usize::MAX,
    };
    let flags = trapframe.get_arg(2) as i32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Parse path from user space
    let path_str = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((path, _)) => path,
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    let path_str = remap_shm_path(&path_str);
    if is_wl_shm_path(&path_str) {
        crate::println!("sys_unlinkat: shm ignore path='{}'", path_str);
        return 0;
    }

    // Linux constants for unlinkat
    const AT_FDCWD: i32 = -100;
    const AT_REMOVEDIR: i32 = 0x200;

    let vfs = match task.vfs.read().clone() {
        Some(v) => v,
        None => return usize::MAX,
    };

    // Determine base directory for path resolution
    use crate::fs::vfs_v2::core::VfsFileObject;

    let (base_entry, base_mount) = if dirfd == AT_FDCWD {
        // Use current working directory as base
        vfs.get_cwd().unwrap_or_else(|| {
            let root_mount = vfs.mount_tree.root_mount.read().clone();
            (root_mount.root.clone(), root_mount)
        })
    } else {
        // Use directory file descriptor as base
        let handle = match abi.get_handle(dirfd as usize) {
            Some(h) => h,
            None => return usize::MAX,
        };
        let kernel_obj = match task.handle_table.get(handle) {
            Some(obj) => obj,
            None => return usize::MAX,
        };
        let file_obj = match kernel_obj.as_file() {
            Some(f) => f,
            None => return usize::MAX,
        };
        let vfs_file_obj = match file_obj.as_any().downcast_ref::<VfsFileObject>() {
            Some(vfs_obj) => vfs_obj,
            None => return usize::MAX,
        };
        (
            vfs_file_obj.get_vfs_entry().clone(),
            vfs_file_obj.get_mount_point().clone(),
        )
    };

    // Resolve the target path and perform the removal operation
    match vfs.resolve_path_from(&base_entry, &base_mount, &path_str) {
        Ok((entry, _mount_point)) => {
            // Prepare absolute path before getting mutable VFS reference
            let absolute_path = if path_str.starts_with('/') {
                path_str.to_string()
            } else {
                // Construct absolute path by resolving relative to current working directory
                match to_absolute_path_v2(&task, &path_str) {
                    Ok(p) => p,
                    Err(_) => return usize::MAX,
                }
            };

            // Get mutable reference to VFS for removal operations
            let vfs_mut = match task.vfs.write().clone() {
                Some(v) => v,
                None => return usize::MAX,
            };

            // Check if AT_REMOVEDIR flag is set
            if flags & AT_REMOVEDIR != 0 {
                // Remove directory - check if it's actually a directory
                let node = entry.node();
                match node.metadata() {
                    Ok(metadata) => {
                        if metadata.file_type == FileType::Directory {
                            // Try to remove the directory using VFS remove operation
                            match vfs_mut.remove(&absolute_path) {
                                Ok(_) => 0,           // Success
                                Err(_) => usize::MAX, // Error removing directory
                            }
                        } else {
                            usize::MAX // Not a directory, cannot use AT_REMOVEDIR
                        }
                    }
                    Err(_) => usize::MAX, // Cannot get metadata
                }
            } else {
                // Remove file or directory (standard removal)
                match vfs_mut.remove(&absolute_path) {
                    Ok(_) => 0,           // Success
                    Err(_) => usize::MAX, // Error removing file
                }
            }
        }
        Err(_) => usize::MAX, // Path resolution failed
    }
}

pub fn sys_epoll_create1(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    const EPOLL_CLOEXEC: i32 = 0o2000000;
    let flags = trapframe.get_arg(0) as i32;
    if flags & !EPOLL_CLOEXEC != 0 {
        trapframe.increment_pc_next(task);
        return usize::MAX;
    }

    trapframe.increment_pc_next(task);

    let id = NEXT_EPOLL_HANDLE_ID.fetch_add(1, Ordering::Relaxed);
    let handle = EPOLL_HANDLE_BASE | (id & 0x0fff_ffff);
    match abi.allocate_fd(handle) {
        Ok(fd) => fd,
        Err(_) => usize::MAX,
    }
}

pub fn sys_epoll_ctl(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let epfd = trapframe.get_arg(0) as usize;
    let op = trapframe.get_arg(1) as i32;
    let fd = trapframe.get_arg(2) as i32;
    let event_ptr = trapframe.get_arg(3);

    trapframe.increment_pc_next(task);

    let Some(epoll_handle) = abi.get_handle(epfd) else {
        return usize::MAX;
    };
    if !is_epoll_handle(epoll_handle) || fd < 0 {
        return usize::MAX;
    }

    match op {
        EPOLL_CTL_ADD | EPOLL_CTL_MOD => {
            let Some((events, data)) = read_linux_epoll_event(task, event_ptr) else {
                return usize::MAX;
            };
            if abi.get_handle(fd as usize).is_none() {
                return usize::MAX;
            }

            let mut interests = epoll_interests().write();
            if let Some(existing) = interests
                .iter_mut()
                .find(|interest| interest.epoll_handle == epoll_handle && interest.fd == fd)
            {
                if op == EPOLL_CTL_ADD {
                    return usize::MAX;
                }
                existing.events = events;
                existing.data = data;
            } else {
                if op == EPOLL_CTL_MOD {
                    return usize::MAX;
                }
                interests.push(EpollInterest {
                    epoll_handle,
                    fd,
                    events,
                    data,
                });
            }
            0
        }
        EPOLL_CTL_DEL => {
            let mut interests = epoll_interests().write();
            let before = interests.len();
            interests
                .retain(|interest| !(interest.epoll_handle == epoll_handle && interest.fd == fd));
            if interests.len() == before {
                usize::MAX
            } else {
                0
            }
        }
        _ => usize::MAX,
    }
}

pub fn sys_epoll_wait(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let epfd = trapframe.get_arg(0) as usize;
    let events_ptr = trapframe.get_arg(1);
    let maxevents = trapframe.get_arg(2) as usize;
    let timeout_ms = trapframe.get_arg(3) as i32;

    trapframe.increment_pc_next(task);

    let Some(epoll_handle) = abi.get_handle(epfd) else {
        return usize::MAX;
    };
    if !is_epoll_handle(epoll_handle) || events_ptr == 0 || maxevents == 0 {
        return usize::MAX;
    }

    let timeout_ticks = if timeout_ms < 0 {
        None
    } else {
        Some(crate::timer::ns_to_ticks(timeout_ms as u64 * 1_000_000))
    };
    let deadline = timeout_ticks.map(|ticks| crate::timer::get_tick().saturating_add(ticks));

    loop {
        let interests: Vec<EpollInterest> = epoll_interests()
            .read()
            .iter()
            .filter(|interest| interest.epoll_handle == epoll_handle)
            .copied()
            .collect();

        let mut ready_count = 0usize;
        for interest in interests {
            if ready_count >= maxevents {
                break;
            }
            let ready = epoll_ready_events(abi, task, interest);
            if ready == 0 {
                continue;
            }
            if !write_linux_epoll_event(task, events_ptr, ready_count, ready, interest.data) {
                return usize::MAX;
            }
            ready_count += 1;
        }
        if ready_count != 0 {
            return ready_count;
        }

        if matches!(timeout_ticks, Some(0)) {
            return 0;
        }
        if let Some(deadline) = deadline
            && crate::timer::get_tick() >= deadline
        {
            return 0;
        }
        task.sleep(trapframe, 1);
    }
}

/// Linux epoll_pwait implementation (stub)
///
/// Like epoll_wait but with signal mask. This is a stub implementation
/// that immediately returns 0 (no events ready).
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: epfd (epoll file descriptor)  
///   - arg1: events (pointer to epoll_event array)
///   - arg2: maxevents (maximum number of events)
///   - arg3: timeout (timeout in milliseconds)
///   - arg4: sigmask (signal mask)
///
/// Returns:
/// - number of ready events
/// - usize::MAX (Linux -1) on error
pub fn sys_epoll_pwait(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let _sigmask_ptr = trapframe.get_arg(4);

    sys_epoll_wait(abi, trapframe)
}

/// Minimal Linux pselect6 implementation (stub)
///
/// Temporary reset: always returns immediately with 0 (no fds ready) and
/// does not block. This avoids complex readiness/timeout semantics until
/// the final design is in place.
///
/// Arguments (RISC-V register usage):
///   arg0: nfds (number of file descriptors to check)
///   arg1: readfds pointer (fd_set*)
///   arg2: writefds pointer (fd_set*)
///   arg3: exceptfds pointer (fd_set*)
///   arg4: timeout pointer (timespec*) or NULL
///   arg5: sigmask pointer (ignored)
///
/// Returns: number of ready descriptors, or -1 (usize::MAX) on error.
pub fn sys_pselect6(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    use crate::object::capability::selectable::{ReadyInterest, ReadySet};
    use crate::timer::ns_to_ticks;

    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let nfds = trapframe.get_arg(0) as usize;
    let readfds_ptr = trapframe.get_arg(1);
    let writefds_ptr = trapframe.get_arg(2);
    let exceptfds_ptr = trapframe.get_arg(3);
    let timeout_ptr = trapframe.get_arg(4);
    let _sigmask_ptr = trapframe.get_arg(5);

    // Only support up to 64 fds in this minimal implementation
    let max_fds = core::cmp::min(nfds, 64);

    // Translate fd_set user pointers (treat as u64 bitmask)
    let mut in_read: u64 = 0;
    let mut in_write: u64 = 0;
    let mut in_except: u64 = 0;
    if readfds_ptr != 0 {
        let kptr = task.vm_manager.translate_to_kva(readfds_ptr).unwrap() as *const u64;
        unsafe { in_read = core::ptr::read_unaligned(kptr) };
    }
    if writefds_ptr != 0 {
        let kptr = task.vm_manager.translate_to_kva(writefds_ptr).unwrap() as *const u64;
        unsafe { in_write = core::ptr::read_unaligned(kptr) };
    }
    if exceptfds_ptr != 0 {
        let kptr = task.vm_manager.translate_to_kva(exceptfds_ptr).unwrap() as *const u64;
        unsafe { in_except = core::ptr::read_unaligned(kptr) };
    }

    // Parse timeout (timespec)
    #[repr(C)]
    struct LinuxTimespec {
        tv_sec: i64,
        tv_nsec: i64,
    }
    let mut timeout_ticks: Option<u64> = None;
    if timeout_ptr != 0 {
        let kptr = task.vm_manager.translate_to_kva(timeout_ptr).unwrap() as *const LinuxTimespec;
        let ts = unsafe { core::ptr::read_unaligned(kptr) };
        // Zero timeout behaves as poll
        if ts.tv_sec == 0 && ts.tv_nsec == 0 {
            timeout_ticks = Some(0);
        } else {
            let ns = (ts.tv_sec as i128) * 1_000_000_000i128 + (ts.tv_nsec as i128);
            let ns_u = if ns <= 0 { 0 } else { (ns as u128) as u64 };
            timeout_ticks = Some(ns_to_ticks(ns_u));
        }
    }

    // First pass: compute immediate readiness; default-ready for non-selectables
    let mut out_read: u64 = 0;
    let mut out_write: u64 = 0;
    let mut out_except: u64 = 0;
    let mut any_ready = false;
    let mut first_selectable_fd: Option<usize> = None;

    for fd in 0..max_fds {
        let bit = 1u64 << fd;
        let want_read = (in_read & bit) != 0;
        let want_write = (in_write & bit) != 0;
        let want_except = (in_except & bit) != 0;
        if !(want_read || want_write || want_except) {
            continue;
        }

        // Resolve handle → KernelObject
        let Some(handle) = abi.get_handle(fd) else {
            continue;
        };
        let Some(kobj) = task.handle_table.get(handle) else {
            continue;
        };

        // Use generic Selectable if available; otherwise default policy
        if let Some(sel) = kobj.as_selectable() {
            // Remember first selectable for potential blocking
            if first_selectable_fd.is_none() {
                first_selectable_fd = Some(fd);
            }

            let interest = ReadyInterest {
                read: want_read,
                write: want_write,
                except: want_except,
            };
            let rs: ReadySet = sel.current_ready(interest);
            if rs.read {
                out_read |= bit;
                any_ready = true;
            }
            if rs.write {
                out_write |= bit;
                any_ready = true;
            }
            if rs.except {
                out_except |= bit; /* any_ready unchanged */
            }
        } else {
            // Default: treat as immediately ready (non-selectable path)
            if want_read {
                out_read |= bit;
                any_ready = true;
            }
            if want_write {
                out_write |= bit;
                any_ready = true;
            }
            // except always false in this minimal implementation
        }
    }

    // If nothing is ready and a non-zero timeout is provided, attempt to block
    if !any_ready {
        let zero_poll = matches!(timeout_ticks, Some(t) if t == 0);
        if !zero_poll {
            // Best-effort: wait on the first selectable fd's primary interest
            if let Some(fd_wait) = first_selectable_fd {
                let bit = 1u64 << fd_wait;
                let want_read = (in_read & bit) != 0;
                let want_write = (in_write & bit) != 0;
                let want_except = (in_except & bit) != 0;
                if let Some(handlew) = abi.get_handle(fd_wait) {
                    if let Some(kobjw) = task.handle_table.get(handlew) {
                        if let Some(sel) = kobjw.as_selectable() {
                            let _ = sel.wait_until_ready(
                                ReadyInterest {
                                    read: want_read,
                                    write: want_write,
                                    except: want_except,
                                },
                                trapframe,
                                timeout_ticks,
                                0,
                            );
                            // After wake or timeout, recompute readiness for all fds properly
                            out_read = 0;
                            out_write = 0;
                            out_except = 0;
                            for fd2 in 0..max_fds {
                                let bit2 = 1u64 << fd2;
                                let want_r = (in_read & bit2) != 0;
                                let want_w = (in_write & bit2) != 0;
                                let want_x = (in_except & bit2) != 0;
                                if !(want_r || want_w || want_x) {
                                    continue;
                                }
                                if let Some(handle2) = abi.get_handle(fd2) {
                                    if let Some(kobj2) = task.handle_table.get(handle2) {
                                        if let Some(sel2) = kobj2.as_selectable() {
                                            let rs2: ReadySet = sel2.current_ready(ReadyInterest {
                                                read: want_r,
                                                write: want_w,
                                                except: want_x,
                                            });
                                            if rs2.read {
                                                out_read |= bit2;
                                            }
                                            if rs2.write {
                                                out_write |= bit2;
                                            }
                                            if rs2.except {
                                                out_except |= bit2;
                                            }
                                        } else {
                                            if want_r {
                                                out_read |= bit2;
                                            }
                                            if want_w {
                                                out_write |= bit2;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Write back fd_sets with results
    if readfds_ptr != 0 {
        let kptr = task.vm_manager.translate_to_kva(readfds_ptr).unwrap() as *mut u64;
        unsafe { core::ptr::write_unaligned(kptr, out_read) };
    }
    if writefds_ptr != 0 {
        let kptr = task.vm_manager.translate_to_kva(writefds_ptr).unwrap() as *mut u64;
        unsafe { core::ptr::write_unaligned(kptr, out_write) };
    }
    if exceptfds_ptr != 0 {
        let kptr = task.vm_manager.translate_to_kva(exceptfds_ptr).unwrap() as *mut u64;
        unsafe { core::ptr::write_unaligned(kptr, out_except) };
    }

    // Return count of ready fds
    let ready_count = out_read.count_ones() as usize
        + out_write.count_ones() as usize
        + out_except.count_ones() as usize;

    trapframe.increment_pc_next(task);
    ready_count
}

/// Minimal Linux ppoll implementation
///
/// Arguments (RISC-V):
///   arg0: fds_ptr (struct pollfd*)
///   arg1: nfds (usize)
///   arg2: timeout_ptr (timespec*) or NULL
///   arg3: sigmask (ignored)
///   arg4: sigsetsize (ignored)
///
/// Returns: number of fds with non-zero revents, or -1 (usize::MAX) on error.
pub fn sys_ppoll(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    use crate::object::capability::selectable::{ReadyInterest, ReadySet};
    use crate::timer::ns_to_ticks;

    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }

    const POLLIN: i16 = 0x0001;
    const POLLPRI: i16 = 0x0002;
    const POLLOUT: i16 = 0x0004;
    const POLLERR: i16 = 0x0008;
    const POLLHUP: i16 = 0x0010;
    const POLLNVAL: i16 = 0x0020;

    let fds_ptr = trapframe.get_arg(0);
    let nfds = trapframe.get_arg(1) as usize;
    let timeout_ptr = trapframe.get_arg(2);
    let _sigmask = trapframe.get_arg(3);
    let _sigsetsize = trapframe.get_arg(4);

    trapframe.increment_pc_next(task);

    if fds_ptr == 0 {
        return usize::MAX;
    }
    let kptr = match task.vm_manager.translate_to_kva(fds_ptr) {
        Some(p) => p as *mut PollFd,
        None => return usize::MAX,
    };
    if kptr.is_null() {
        return usize::MAX;
    }
    let fds: &mut [PollFd] = unsafe { core::slice::from_raw_parts_mut(kptr, nfds) };

    #[repr(C)]
    struct LinuxTimespec {
        tv_sec: i64,
        tv_nsec: i64,
    }
    let mut timeout_ticks: Option<u64> = None;
    if timeout_ptr != 0 {
        let tsp = match task.vm_manager.translate_to_kva(timeout_ptr) {
            Some(p) => p as *const LinuxTimespec,
            None => return usize::MAX,
        };
        let ts = unsafe { core::ptr::read_unaligned(tsp) };
        if ts.tv_sec == 0 && ts.tv_nsec == 0 {
            timeout_ticks = Some(0);
        } else {
            let ns = (ts.tv_sec as i128) * 1_000_000_000i128 + (ts.tv_nsec as i128);
            let ns_u = if ns <= 0 { 0 } else { (ns as u128) as u64 };
            timeout_ticks = Some(ns_to_ticks(ns_u));
            // crate::println!(
            //     "[sys_ppoll] timeout ts={}s {}ns -> ns={} ticks={:?}",
            //     ts.tv_sec,
            //     ts.tv_nsec,
            //     ns_u,
            //     timeout_ticks
            // );
        }
    }

    struct EvalResult {
        ready: bool,
        selectable: bool,
    }

    fn eval_pfd(
        pfd: &mut PollFd,
        abi_ref: &LinuxRiscv64Abi,
        task_ref: &crate::task::Task,
    ) -> EvalResult {
        pfd.revents = 0;
        if pfd.fd < 0 {
            pfd.revents |= POLLNVAL;
            return EvalResult {
                ready: true,
                selectable: false,
            };
        }
        let fd_usize = pfd.fd as usize;
        let Some(handle) = abi_ref.get_handle(fd_usize) else {
            pfd.revents |= POLLNVAL;
            return EvalResult {
                ready: true,
                selectable: false,
            };
        };
        let Some(kobj) = task_ref.handle_table.get(handle) else {
            pfd.revents |= POLLNVAL;
            return EvalResult {
                ready: true,
                selectable: false,
            };
        };

        let want_read = (pfd.events & POLLIN) != 0;
        let want_write = (pfd.events & POLLOUT) != 0;
        let want_except = (pfd.events & POLLPRI) != 0;

        let mut selectable = false;

        if let Some(sel) = kobj.as_selectable() {
            selectable = true;
            let rs: ReadySet = sel.current_ready(ReadyInterest {
                read: want_read,
                write: want_write,
                except: want_except,
            });
            if rs.read && want_read {
                pfd.revents |= POLLIN;
            }
            if rs.write && want_write {
                pfd.revents |= POLLOUT;
            }
            if rs.except && want_except {
                pfd.revents |= POLLPRI;
            }
        } else {
            if want_read {
                pfd.revents |= POLLIN;
            }
            if want_write {
                pfd.revents |= POLLOUT;
            }
        }

        if let Some(pipe) = kobj.as_pipe() {
            if pipe.is_readable() && !pipe.has_writers() {
                pfd.revents |= POLLHUP;
                if want_read && (pfd.revents & POLLIN) == 0 {
                    pfd.revents |= POLLIN;
                }
            }
            if pipe.is_writable() && !pipe.has_readers() {
                pfd.revents |= POLLERR | POLLHUP;
            }
        }

        EvalResult {
            ready: pfd.revents != 0,
            selectable,
        }
    }

    let mut any_ready = false;
    let mut first_selectable_index: Option<usize> = None;
    let mut selectable_count = 0usize;
    let mut ready_count = 0usize;
    {
        let abi_ref = &*abi;
        let task_ref: &crate::task::Task = &*task;
        for (idx, pfd) in fds.iter_mut().enumerate() {
            let eval = eval_pfd(pfd, abi_ref, task_ref);
            if eval.ready {
                any_ready = true;
                ready_count += 1;
            }
            if eval.selectable {
                selectable_count += 1;
            }
            if first_selectable_index.is_none() && eval.selectable {
                first_selectable_index = Some(idx);
            }

            let mut kind = "unknown";
            let mut socket_state: Option<u32> = None;
            let fd_usize = pfd.fd as usize;
            if let Some(handle) = abi_ref.get_handle(fd_usize) {
                if let Some(kobj) = task_ref.handle_table.get(handle) {
                    match kobj {
                        crate::object::KernelObject::File(_) => {
                            kind = "file";
                        }
                        crate::object::KernelObject::Pipe(_) => {
                            kind = "pipe";
                        }
                        crate::object::KernelObject::Counter(_) => {
                            kind = "counter";
                        }
                        crate::object::KernelObject::EventChannel(_) => {
                            kind = "event_channel";
                        }
                        crate::object::KernelObject::EventSubscription(_) => {
                            kind = "event_sub";
                        }
                        #[cfg(feature = "network")]
                        crate::object::KernelObject::Socket(socket) => {
                            kind = "socket";
                            socket_state = Some(socket.state() as u32);
                        }
                        crate::object::KernelObject::SharedMemory(_) => {
                            kind = "shmem";
                        }
                        #[cfg(feature = "hypervisor")]
                        crate::object::KernelObject::HypervisorVm(_) => {
                            kind = "hypervisor_vm";
                        }
                        #[cfg(feature = "hypervisor")]
                        crate::object::KernelObject::HypervisorVcpu(_) => {
                            kind = "hypervisor_vcpu";
                        }
                    }
                }
            }

            // crate::println!(
            //     "[sys_ppoll] fd={} events=0x{:x} revents=0x{:x} selectable={} kind={} socket_state={:?}",
            //     pfd.fd,
            //     pfd.events as u16,
            //     pfd.revents as u16,
            //     eval.selectable,
            //     kind,
            //     socket_state
            // );
        }
    }

    {
        let zero_poll = matches!(timeout_ticks, Some(t) if t == 0);
        // crate::println!(
        //     "[sys_ppoll] task={} nfds={} selectable={} ready={} any_ready={} zero_poll={} timeout_ticks={:?} first_selectable={:?}",
        //     task.name,
        //     nfds,
        //     selectable_count,
        //     ready_count,
        //     any_ready,
        //     zero_poll,
        //     timeout_ticks,
        //     first_selectable_index
        // );
    }

    if !any_ready {
        let zero_poll = matches!(timeout_ticks, Some(t) if t == 0);
        if !zero_poll {
            if selectable_count > 1 {
                use crate::timer::get_tick;

                let deadline = timeout_ticks.map(|ticks| get_tick().saturating_add(ticks));
                loop {
                    if let Some(deadline) = deadline {
                        let now = get_tick();
                        if now >= deadline {
                            break;
                        }
                        let remaining = deadline.saturating_sub(now);
                        if remaining == 0 {
                            break;
                        }
                    }

                    task.sleep(trapframe, 1);

                    any_ready = false;
                    ready_count = 0;
                    {
                        let abi_ref = &*abi;
                        let task_ref: &crate::task::Task = &*task;
                        for pfd in fds.iter_mut() {
                            let eval = eval_pfd(pfd, abi_ref, task_ref);
                            if eval.ready {
                                any_ready = true;
                                ready_count += 1;
                            }
                        }
                    }

                    if any_ready {
                        break;
                    }
                }
            } else if let Some(wait_idx) = first_selectable_index {
                let pfd = &fds[wait_idx];
                if pfd.fd >= 0 {
                    let fd_usize = pfd.fd as usize;
                    let abi_ref = &*abi;
                    let task_ref: &crate::task::Task = &*task;
                    if let Some(handle) = abi_ref.get_handle(fd_usize) {
                        if let Some(kobj) = task_ref.handle_table.get(handle) {
                            if let Some(sel) = kobj.as_selectable() {
                                let want_read = (pfd.events & POLLIN) != 0;
                                let want_write = (pfd.events & POLLOUT) != 0;
                                let want_except = (pfd.events & POLLPRI) != 0;
                                let _ = sel.wait_until_ready(
                                    ReadyInterest {
                                        read: want_read,
                                        write: want_write,
                                        except: want_except,
                                    },
                                    trapframe,
                                    timeout_ticks,
                                    0,
                                );
                            }
                        }
                    }
                }

                let abi_ref = &*abi;
                let task_ref: &crate::task::Task = &*task;
                for pfd in fds.iter_mut() {
                    let _ = eval_pfd(pfd, abi_ref, task_ref);
                }
            }
        }
    }

    let mut count = 0usize;
    for pfd in fds.iter() {
        if pfd.revents != 0 {
            count += 1;
        }
    }
    count
}

/// Linux sys_fchmod implementation (stub)
///
/// Changes the permissions of a file using its file descriptor.
/// This is a stub implementation that simply validates the file descriptor
/// and returns success without actually changing permissions.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: fd (file descriptor)
///   - arg1: mode (new file permissions)
///
/// Returns:
/// - 0 on success (if fd is valid)
/// - usize::MAX (Linux -1) on error (if fd is invalid)
pub fn sys_fchmod(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let fd = trapframe.get_arg(0) as i32;
    let _mode = trapframe.get_arg(1) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Validate the file descriptor
    let handle = match abi.get_handle(fd as usize) {
        Some(h) => h,
        None => return usize::MAX, // Invalid file descriptor
    };

    // Check if the handle exists in the handle table
    match task.handle_table.get(handle) {
        Some(_) => {
            // File descriptor is valid, return success
            // In a real implementation, we would change the file permissions here
            0 // Success
        }
        None => usize::MAX, // Handle not found
    }
}

/// Linux sys_umask implementation (stub)
///
/// Sets the file mode creation mask (umask) and returns the previous value.
/// This is a stub implementation that simply returns the provided mask
/// without actually storing or using it for file creation permissions.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: mask (new file creation mask)
///
/// Returns:
/// - The provided mask value (simulating the previous umask)
pub fn sys_umask(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let mask = trapframe.get_arg(0) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // In a real implementation, we would:
    // 1. Store the current umask value to return it
    // 2. Set the new umask value for future file creation operations
    // 3. Return the previous umask value
    //
    // For this stub implementation, we simply return the provided mask
    // This satisfies most applications that just want to set a umask
    mask as usize // Return the provided mask as if it was the previous value
}

/// Linux sys_readlinkat implementation
///
/// Reads the target of a symbolic link relative to a directory file descriptor.
/// Properly queries the VFS and does not append a null terminator.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context  
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: dirfd (directory file descriptor or AT_FDCWD)
///   - arg1: pathname (pointer to path string)
///   - arg2: buf (buffer to store link contents)
///   - arg3: bufsiz (size of buffer)
///
/// Returns:
/// - Number of bytes placed in buf on success
/// - usize::MAX (Linux -1) on error
pub fn sys_readlinkat(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::EIO),
    };

    let dirfd = trapframe.get_arg(0) as i32;
    let pathname_ptr = trapframe.get_arg(1);
    let buf_ptr = trapframe.get_arg(2);
    let bufsiz = trapframe.get_arg(3) as usize;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Fast path: zero-sized buffer
    if bufsiz == 0 {
        return 0;
    }

    // Parse the pathname
    let path_str = match parse_c_string_from_userspace(task, pathname_ptr, MAX_PATH_LENGTH) {
        Ok(s) => s,
        Err(_) => return errno::to_result(errno::EFAULT),
    };

    // Acquire VFS
    let vfs = match task.vfs.read().clone() {
        Some(v) => v,
        None => return errno::to_result(errno::EIO),
    };

    // Determine base directory (entry and mount) for path resolution
    use crate::fs::vfs_v2::core::VfsFileObject;
    const AT_FDCWD: i32 = -100;

    let (base_entry, base_mount) = if dirfd == AT_FDCWD {
        vfs.get_cwd().unwrap_or_else(|| {
            let root_mount = vfs.mount_tree.root_mount.read().clone();
            (root_mount.root.clone(), root_mount)
        })
    } else {
        // Resolve base from dirfd
        let handle = match abi.get_handle(dirfd as usize) {
            Some(h) => h,
            None => return errno::to_result(errno::EBADF),
        };
        let kernel_obj = match task.handle_table.get(handle) {
            Some(obj) => obj,
            None => return errno::to_result(errno::EBADF),
        };
        let file_obj = match kernel_obj.as_file() {
            Some(f) => f,
            None => return errno::to_result(errno::ENOTDIR),
        };
        let vfs_file_obj = match file_obj.as_any().downcast_ref::<VfsFileObject>() {
            Some(vfs_obj) => vfs_obj,
            None => return errno::to_result(errno::ENOTDIR),
        };
        (
            vfs_file_obj.get_vfs_entry().clone(),
            vfs_file_obj.get_mount_point().clone(),
        )
    };

    // Resolve the path from the base (do not follow the final link)
    let (entry, _mp) = match vfs.resolve_path_from(&base_entry, &base_mount, &path_str) {
        Ok(v) => v,
        Err(e) => return errno::to_result(errno::from_fs_error(&e)),
    };

    // Ensure the target is a symlink and obtain its target
    let node = entry.node();
    let metadata = match node.metadata() {
        Ok(m) => m,
        Err(e) => return errno::to_result(errno::from_fs_error(&e)),
    };

    let target = match metadata.file_type {
        FileType::SymbolicLink(ref t) => t.as_str(),
        _ => return errno::to_result(errno::EINVAL), // Not a symlink
    };

    // Copy to user buffer (no null terminator), truncated if needed
    let target_bytes = target.as_bytes();
    let copy_len = core::cmp::min(target_bytes.len(), bufsiz);

    let user_buf = match task.vm_manager.translate_to_kva(buf_ptr) {
        Some(addr) => addr as *mut u8,
        None => return errno::to_result(errno::EFAULT),
    };

    if copy_len > 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(target_bytes.as_ptr(), user_buf, copy_len);
        }
    }

    copy_len
}

/// Linux sys_getrandom implementation
///
/// Fills the user buffer with random bytes from the kernel pool.
pub fn sys_getrandom(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::EIO),
    };

    let buf_ptr = trapframe.get_arg(0);
    let buflen = trapframe.get_arg(1) as usize;
    let flags = trapframe.get_arg(2) as u32;

    trapframe.increment_pc_next(task);

    if buflen == 0 {
        return 0;
    }

    let user_buf = match task.vm_manager.translate_to_kva(buf_ptr) {
        Some(addr) => addr as *mut u8,
        None => return errno::to_result(errno::EFAULT),
    };

    if user_buf.is_null() {
        return errno::to_result(errno::EFAULT);
    }

    let buffer = unsafe { core::slice::from_raw_parts_mut(user_buf, buflen) };
    let bytes_read = crate::random::RandomManager::get_random_bytes(buffer);

    if bytes_read == 0 {
        if (flags & GRND_NONBLOCK) != 0 {
            return errno::to_result(errno::EAGAIN);
        }
        fill_pseudo_random(buffer);
        return buflen;
    }

    if bytes_read < buflen && (flags & GRND_NONBLOCK) == 0 {
        fill_pseudo_random(&mut buffer[bytes_read..]);
        return buflen;
    }

    bytes_read
}

/// Linux sys_getcwd system call implementation
/// Get current working directory
///
/// Arguments:
/// - buf: Buffer to store the current working directory path
/// - size: Size of the buffer
///
/// Returns:
/// - Number of bytes written to buffer on success
/// - usize::MAX on error
pub fn sys_getcwd(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let buf_ptr = trapframe.get_arg(0);
    let size = trapframe.get_arg(1);
    trapframe.increment_pc_next(task);

    // Check for invalid arguments
    if buf_ptr == 0 || size == 0 {
        return usize::MAX; // EFAULT or EINVAL
    }

    // Get current working directory from task context
    let cwd = if let Some(vfs) = task.vfs.read().clone() {
        vfs.get_cwd_path()
    } else {
        "/".to_string() // Default to root if no VFS manager
    };
    let cwd_bytes = cwd.as_bytes();

    // Check if buffer is large enough (including null terminator)
    if cwd_bytes.len() + 1 > size {
        return usize::MAX; // ERANGE - buffer too small
    }

    // Translate user buffer address
    let user_buf = match task.vm_manager.translate_to_kva(buf_ptr) {
        Some(addr) => addr as *mut u8,
        None => return usize::MAX, // EFAULT - invalid buffer address
    };

    // Copy current working directory to user buffer
    unsafe {
        core::ptr::copy_nonoverlapping(cwd_bytes.as_ptr(), user_buf, cwd_bytes.len());
        // Add null terminator
        *user_buf.add(cwd_bytes.len()) = 0;
    }

    // Return the number of bytes written (including null terminator)
    cwd_bytes.len() + 1
}

/// Linux sys_chdir system call implementation (syscall 49)
/// Change current working directory
///
/// Arguments:
/// - path: Path to the new working directory
///
/// Returns:
/// - 0 on success
/// - usize::MAX on error (path not found, not a directory, permission denied, etc.)
pub fn sys_chdir(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let path_ptr = match task.vm_manager.translate_to_kva(trapframe.get_arg(0)) {
        Some(ptr) => ptr as *const u8,
        None => return usize::MAX,
    };

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Parse path from user space
    let path_str = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((path, _)) => path,
        Err(_) => return usize::MAX, // Invalid UTF-8 or path too long
    };

    crate::println!("sys_chdir: Changing directory to '{}'", path_str);

    // Convert to absolute path
    let absolute_path = if path_str.starts_with('/') {
        path_str
    } else {
        match to_absolute_path_v2(&task, &path_str) {
            Ok(p) => p,
            Err(_) => return usize::MAX,
        }
    };

    let vfs = match task.vfs.read().clone() {
        Some(v) => v,
        None => return usize::MAX,
    };

    // Check if the path exists and is a directory
    match vfs.resolve_path(&absolute_path) {
        Ok((entry, _mount_point)) => {
            match entry.node().file_type() {
                Ok(file_type) => {
                    if file_type == FileType::Directory {
                        // Update the current working directory via VfsManager
                        match vfs.set_cwd_by_path(&absolute_path) {
                            Ok(()) => {
                                crate::println!(
                                    "sys_chdir: Successfully changed directory to '{}'",
                                    absolute_path
                                );
                                0 // Success
                            }
                            Err(_) => {
                                crate::println!(
                                    "sys_chdir: Failed to set working directory to '{}'",
                                    absolute_path
                                );
                                usize::MAX // Failed to set cwd
                            }
                        }
                    } else {
                        crate::println!("sys_chdir: '{}' is not a directory", absolute_path);
                        usize::MAX // Not a directory (ENOTDIR)
                    }
                }
                Err(_) => {
                    crate::println!("sys_chdir: Failed to get file type for '{}'", absolute_path);
                    usize::MAX // Failed to get file type
                }
            }
        }
        Err(_) => {
            crate::println!("sys_chdir: Path '{}' not found", absolute_path);
            usize::MAX // Path not found (ENOENT)
        }
    }
}

// renameat2 flags
const RENAME_NOREPLACE: u32 = 1 << 0; // Don't overwrite target
const RENAME_EXCHANGE: u32 = 1 << 1; // Exchange source and target
#[allow(dead_code)]
const RENAME_WHITEOUT: u32 = 1 << 2; // Create whiteout object

/// Linux sys_renameat2 system call implementation (syscall 276)
/// Rename/move a file or directory with additional flags
///
/// Arguments:
/// - olddirfd: Old directory file descriptor (or AT_FDCWD)
/// - oldpath: Pointer to old path string
/// - newdirfd: New directory file descriptor (or AT_FDCWD)  
/// - newpath: Pointer to new path string
/// - flags: Rename operation flags
///
/// Returns:
/// - 0 on success
/// - usize::MAX on error
pub fn sys_renameat2(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let olddirfd = trapframe.get_arg(0) as i32;
    let oldpath_ptr = match task.vm_manager.translate_to_kva(trapframe.get_arg(1)) {
        Some(ptr) => ptr as *const u8,
        None => return usize::MAX,
    };
    let newdirfd = trapframe.get_arg(2) as i32;
    let newpath_ptr = match task.vm_manager.translate_to_kva(trapframe.get_arg(3)) {
        Some(ptr) => ptr as *const u8,
        None => return usize::MAX,
    };
    let flags = trapframe.get_arg(4) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Parse old path from user space
    let oldpath_str = match cstring_to_string(oldpath_ptr, MAX_PATH_LENGTH) {
        Ok((path, _)) => path,
        Err(_) => return usize::MAX, // Invalid UTF-8 or path too long
    };

    // Parse new path from user space
    let newpath_str = match cstring_to_string(newpath_ptr, MAX_PATH_LENGTH) {
        Ok((path, _)) => path,
        Err(_) => return usize::MAX, // Invalid UTF-8 or path too long
    };

    crate::println!(
        "sys_renameat2: olddirfd={}, oldpath='{}', newdirfd={}, newpath='{}', flags={:#x}",
        olddirfd,
        oldpath_str,
        newdirfd,
        newpath_str,
        flags
    );

    // Check for unsupported flags
    const SUPPORTED_FLAGS: u32 = RENAME_NOREPLACE | RENAME_EXCHANGE;
    if (flags & !SUPPORTED_FLAGS) != 0 {
        crate::println!(
            "sys_renameat2: Unsupported flags: {:#x}",
            flags & !SUPPORTED_FLAGS
        );
        return usize::MAX; // EINVAL - unsupported flags
    }

    // RENAME_EXCHANGE and RENAME_NOREPLACE are mutually exclusive
    if (flags & RENAME_EXCHANGE) != 0 && (flags & RENAME_NOREPLACE) != 0 {
        crate::println!(
            "sys_renameat2: RENAME_EXCHANGE and RENAME_NOREPLACE are mutually exclusive"
        );
        return usize::MAX; // EINVAL
    }

    let vfs = match task.vfs.read().clone() {
        Some(v) => v,
        None => return usize::MAX,
    };

    // Note: Current implementation ignores dirfd and only uses absolute path resolution
    // TODO: Implement proper *at support for relative paths from directory file descriptors

    // Resolve absolute paths using basic path resolution
    let old_absolute_path = if oldpath_str.starts_with('/') {
        oldpath_str
    } else {
        match to_absolute_path_v2(&task, &oldpath_str) {
            Ok(p) => p,
            Err(_) => return usize::MAX,
        }
    };

    let new_absolute_path = if newpath_str.starts_with('/') {
        newpath_str
    } else {
        match to_absolute_path_v2(&task, &newpath_str) {
            Ok(p) => p,
            Err(_) => return usize::MAX,
        }
    };

    crate::println!(
        "sys_renameat2: Resolved paths: '{}' -> '{}'",
        old_absolute_path,
        new_absolute_path
    );

    // Handle different rename operations based on flags
    if (flags & RENAME_EXCHANGE) != 0 {
        // Exchange operation: swap the two files/directories
        crate::println!("sys_renameat2: Exchange operation not yet implemented");
        return usize::MAX; // ENOSYS - not implemented
    } else {
        // Standard rename/move operation
        let no_replace = (flags & RENAME_NOREPLACE) != 0;

        // Check if target exists when RENAME_NOREPLACE is set
        if no_replace {
            match vfs.resolve_path(&new_absolute_path) {
                Ok(_) => {
                    crate::println!(
                        "sys_renameat2: Target exists and RENAME_NOREPLACE flag is set"
                    );
                    return usize::MAX; // EEXIST - target exists
                }
                Err(_) => {
                    // Target doesn't exist, which is what we want for RENAME_NOREPLACE
                }
            }
        }

        match vfs.rename(&old_absolute_path, &new_absolute_path) {
            Ok(()) => 0,
            Err(e) => {
                crate::println!("sys_renameat2: rename failed: {:?}", e);
                errno::to_result(errno::from_fs_error(&e))
            }
        }
    }
}

/// eventfd2 - create file descriptor for event notification
///
/// # Arguments
/// * `initval` - Initial value of the event counter (unsigned int)
/// * `flags` - Flags (bitwise OR of EFD_CLOEXEC, EFD_NONBLOCK, EFD_SEMAPHORE)
///
/// # Returns
/// * New file descriptor on success
/// * Error code on failure
pub fn sys_eventfd2(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::EIO),
    };

    let initval = trapframe.get_arg(0) as u32;
    let flags = trapframe.get_arg(1) as u32;

    trapframe.increment_pc_next(task);

    // eventfd2 flags
    const EFD_CLOEXEC: u32 = 0o02000000;
    const EFD_NONBLOCK: u32 = 0o00004000;
    const EFD_SEMAPHORE: u32 = 0x00000001;

    // Validate flags (only allow EFD_CLOEXEC, EFD_NONBLOCK, EFD_SEMAPHORE)
    let valid_flags = EFD_CLOEXEC | EFD_NONBLOCK | EFD_SEMAPHORE;
    if flags & !valid_flags != 0 {
        crate::println!("[sys_eventfd2] Invalid flags: 0x{:x}", flags);
        return errno::to_result(errno::EINVAL);
    }

    // Create Counter object
    let counter_obj = crate::ipc::counter::Counter::create_kernel_object(initval, flags);

    // Insert into handle table
    let handle = match task.handle_table.insert(counter_obj) {
        Ok(h) => h,
        Err(_) => return errno::to_result(errno::EMFILE),
    };

    // Allocate file descriptor for the handle
    let fd = match abi.allocate_fd(handle as u32) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = task.handle_table.remove(handle);
            return errno::to_result(errno::EMFILE);
        }
    };

    // Set FD_CLOEXEC if requested
    if (flags & EFD_CLOEXEC) != 0 {
        let _ = abi.set_fd_flags(fd, FD_CLOEXEC);
    }

    // Set O_NONBLOCK if requested (already set in Counter, but also in ABI for consistency)
    if (flags & EFD_NONBLOCK) != 0 {
        let _ = abi.set_file_status_flags(fd, O_NONBLOCK as u32);
    }

    fd
}
