//! Small read-only proc/sys text files in the Linux ABI view.

use alloc::{format, string::String, sync::Arc};
use core::{any::Any, arch::asm};

use crate::{
    abi::linux::generic::{
        LinuxAbi, errno,
        fs::{FD_CLOEXEC, O_CLOEXEC, O_DIRECTORY},
    },
    arch::Trapframe,
    fs::{
        DirectoryEntry, DirectoryEntryInternal, FileMetadata, FilePermission, FileType, SeekFrom,
    },
    object::{
        KernelObject,
        capability::{
            ControlOps, FileObject, MemoryMappingInfo, MemoryMappingOps, ReadyInterest,
            SelectWaitOutcome, Selectable, StreamError, StreamOps,
        },
    },
    sync::IrqRwSpinLock,
    task::Task,
};

#[cfg(target_arch = "aarch64")]
fn cpu_list() -> String {
    let mut list = String::new();
    crate::sched::scheduler::for_each_online_cpu(|id| {
        if !list.is_empty() {
            list.push(',');
        }
        list.push_str(&format!("{id}"));
    });
    list.push('\n');
    list
}

#[cfg(target_arch = "aarch64")]
fn cpu_info() -> String {
    let midr: u64;
    // SAFETY: the kernel executes at EL1 and MIDR_EL1 is read-only.
    unsafe { asm!("mrs {value}, midr_el1", value = out(reg) midr, options(nomem, nostack)) };
    let hwcap = crate::arch::aarch64::cpu_features::userspace_capabilities().hwcap;
    let mut features = String::new();
    for (bit, name) in [
        (0, "fp"),
        (1, "asimd"),
        (3, "aes"),
        (4, "pmull"),
        (5, "sha1"),
        (6, "sha2"),
        (7, "crc32"),
        (8, "atomics"),
        (9, "fphp"),
        (10, "asimdhp"),
        (21, "sha512"),
    ] {
        if hwcap & (1u64 << bit) != 0 {
            if !features.is_empty() {
                features.push(' ');
            }
            features.push_str(name);
        }
    }
    let mut content = String::new();
    crate::sched::scheduler::for_each_online_cpu(|id| {
        content.push_str(&format!(
            "processor\t: {id}\nFeatures\t: {features}\nCPU implementer\t: 0x{:02x}\nCPU architecture: 8\nCPU variant\t: 0x{:x}\nCPU part\t: 0x{:03x}\nCPU revision\t: {}\n\n",
            (midr >> 24) & 0xff, (midr >> 20) & 0xf, (midr >> 4) & 0xfff, midr & 0xf,
        ));
    });
    content
}

const CPU_DIRECTORY: &str = "/sys/devices/system/cpu";
const CPU_FILES: &[&str] = &["online", "possible", "present"];

/// Report the calling address space, including native ELF loader mappings.
/// Box64 uses these ranges before retrying a hinted mmap with MAP_FIXED.
fn self_maps() -> Option<String> {
    let task = crate::task::mytask()?;
    let ranges = task.vm_manager.with_memmaps(|maps| {
        maps.values()
            .filter(|map| map.vmarea.start < crate::environment::USER_LOWER_CANONICAL_END)
            .map(|map| {
                (
                    map.vmarea.start,
                    map.vmarea.end,
                    map.permissions,
                    map.is_shared,
                )
            })
            .collect::<alloc::vec::Vec<_>>()
    });
    let mut content = String::new();
    for (start, end, permissions, shared) in ranges {
        // VMA ends are inclusive internally; Linux maps uses exclusive ends.
        // Backing file identities/offsets are not exposed by every map owner.
        content.push_str(&format!(
            "{start:08x}-{:08x} {}{}{}{} 00000000 00:00 0\n",
            end + 1,
            if permissions & 1 != 0 { 'r' } else { '-' },
            if permissions & 2 != 0 { 'w' } else { '-' },
            if permissions & 4 != 0 { 'x' } else { '-' },
            if shared { 's' } else { 'p' },
        ));
    }
    Some(content)
}

/// A fixed-shape Linux stat record. Identity, state, scheduling priority,
/// virtual size and entry stack come from native state. Accounting fields not
/// exposed by Scarlet's process interface are zero, not guessed from limits.
fn task_stat(task: &Task, thread: bool) -> String {
    use crate::task::TaskState;
    let ns = task.get_namespace();
    let pid = if thread {
        task.get_id()
    } else {
        task.get_thread_group_id()
    };
    let id = |global| ns.resolve_local_id(global).unwrap_or(0);
    let name = {
        let name = task.name.read();
        let name = name.rsplit('/').next().unwrap_or(&name);
        let mut length = name.len().min(15);
        while !name.is_char_boundary(length) {
            length -= 1;
        }
        String::from(&name[..length])
    };
    let state = match task.get_state() {
        TaskState::Ready | TaskState::Running | TaskState::NotInitialized => 'R',
        TaskState::Blocked(_) => 'S',
        TaskState::Zombie | TaskState::Terminated => 'Z',
    };
    let mut content = format!("{} ({name}) {state}", id(pid));
    // Fields 4..52, following proc_pid_stat(5). Signed fields are formatted as
    // signed values so a negative nice value does not wrap into an unsigned PID.
    let mut fields = [0i128; 49];
    fields[0] = task.get_parent_id().map_or(0, id) as i128;
    fields[1] = id(task.get_process_group_id()) as i128;
    fields[2] = id(task.get_session_id()) as i128;
    let nice = task.sched_nice.load(core::sync::atomic::Ordering::Relaxed);
    fields[18 - 4] = (20 + nice) as i128;
    fields[19 - 4] = nice as i128;
    fields[20 - 4] = crate::sched::scheduler::get_all_task_ids()
        .into_iter()
        .filter(|id| {
            crate::sched::scheduler::get_task_by_id(*id).is_some_and(|other| {
                other.get_thread_group_id() == task.get_thread_group_id()
                    && !matches!(other.get_state(), TaskState::Zombie | TaskState::Terminated)
            })
        })
        .count()
        .max(1) as i128;
    fields[23 - 4] = task.vm_manager.with_memmaps(|maps| {
        maps.values()
            .filter(|map| map.vmarea.end < crate::environment::USER_LOWER_CANONICAL_END)
            .map(|map| map.vmarea.size() as i128)
            .sum()
    });
    fields[25 - 4] = u64::MAX as i128;
    fields[28 - 4] = task
        .vm_manager
        .process_entry_stack()
        .map_or(0, |stack| stack.initial_sp) as i128;
    for field in fields {
        content.push_str(&format!(" {field}"));
    }
    content.push('\n');
    content
}

/// mntent escapes must preserve whitespace and backslashes in mount paths.
fn mount_field(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            ' ' => escaped.push_str("\\040"),
            '\t' => escaped.push_str("\\011"),
            '\n' => escaped.push_str("\\012"),
            '\\' => escaped.push_str("\\134"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn mounts_content(fs: &crate::fs::VfsManager) -> String {
    use crate::fs::vfs_v2::drivers::{tmpfs::TmpFS, overlayfs::OverlayFS, devfs::DevFS, cpiofs::CpioFS};
    let mut content = String::new();
    for (path, mount) in fs.mounted_paths() {
        let filesystem = mount.filesystem.clone().or_else(|| mount.root.node().filesystem().and_then(|fs| fs.upgrade()));
        let Some(filesystem) = filesystem else { continue };
        let kind = if filesystem.as_any().is::<TmpFS>() { "tmpfs" }
            else if filesystem.as_any().is::<OverlayFS>() { "overlay" }
            else if filesystem.as_any().is::<DevFS>() { "devfs" }
            else if filesystem.as_any().is::<CpioFS>() { "cpiofs" }
            else { filesystem.name() };
        let restrictions = mount.restrictions;
        let mut options = String::from("rw");
        if !restrictions.allows_privilege_gain() { options.push_str(",nosuid"); }
        if !restrictions.allows_devices() { options.push_str(",nodev"); }
        if !restrictions.allows_execution() { options.push_str(",noexec"); }
        content.push_str(&format!("none {} {} {options} 0 0\n", mount_field(&path), mount_field(kind)));
    }
    content
}

fn text_content(path: &str) -> Option<String> {
    if matches!(path, "/proc/mounts" | "/proc/self/mounts" | "/proc/thread-self/mounts") {
        let fs = crate::task::mytask()?.get_vfs()?;
        return Some(mounts_content(&fs));
    }
    if matches!(path, "/proc/self/stat" | "/proc/thread-self/stat") {
        let caller = crate::task::mytask()?;
        let thread = path == "/proc/thread-self/stat";
        let leader = (!thread)
            .then(|| crate::sched::scheduler::get_task_by_id(caller.get_thread_group_id()))
            .flatten();
        return Some(task_stat(leader.as_deref().unwrap_or(&caller), thread));
    }
    if matches!(path, "/proc/self/maps" | "/proc/thread-self/maps") {
        return self_maps();
    }
    #[cfg(target_arch = "aarch64")]
    return Some(match path {
        "/proc/cpuinfo" => cpu_info(),
        // Use scheduler state, not a fixed count or the caller's affinity.
        "/sys/devices/system/cpu/online"
        | "/sys/devices/system/cpu/possible"
        | "/sys/devices/system/cpu/present" => cpu_list(),
        "/proc/sys/fs/inotify/max_user_watches" => String::from("8192\n"),
        _ => return None,
    });
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = path;
        None
    }
}

fn is_cpu_directory(path: &str) -> bool {
    cfg!(target_arch = "aarch64") && path.trim_end_matches('/') == CPU_DIRECTORY
}

fn metadata(path: &str, size: usize, directory: bool) -> FileMetadata {
    // Stable across open/stat, distinct for each synthetic entry.
    let file_id = path
        .trim_end_matches('/')
        .bytes()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    FileMetadata {
        file_type: if directory {
            FileType::Directory
        } else {
            FileType::RegularFile
        },
        size,
        permissions: FilePermission {
            read: true,
            write: false,
            execute: directory,
        },
        created_time: 0,
        modified_time: 0,
        accessed_time: 0,
        file_id,
        link_count: if directory { 2 } else { 1 },
    }
}

pub(super) fn path_metadata(path: &str) -> Option<FileMetadata> {
    if is_cpu_directory(path) {
        Some(metadata(path, 0, true))
    } else {
        text_content(path).map(|content| metadata(path, content.len(), false))
    }
}

/// Resolve relative openat/stat/access on the directory used by util-linux.
pub(super) fn directory_path(file: &dyn FileObject) -> Option<&str> {
    let file = file.as_any().downcast_ref::<ProcTextFile>()?;
    file.directory.then_some(file.path.as_str())
}

pub(super) fn open_proc_text_file(
    abi: &mut LinuxAbi,
    task: &Task,
    path: &str,
    flags: i32,
) -> Option<usize> {
    let directory = is_cpu_directory(path);
    let content = if directory {
        String::new()
    } else {
        text_content(path)?
    };
    if flags & 3 != 0 {
        return Some(errno::to_result(errno::EACCES));
    }
    if flags & O_DIRECTORY != 0 && !directory {
        return Some(errno::to_result(errno::ENOTDIR));
    }
    let file = Arc::new(ProcTextFile {
        position: IrqRwSpinLock::new(0),
        content,
        path: String::from(path.trim_end_matches('/')),
        directory,
    });
    let handle = match task.handle_table.insert(KernelObject::File(file)) {
        Ok(handle) => handle,
        Err(_) => return Some(errno::to_result(errno::ENFILE)),
    };
    let fd = match abi.allocate_fd(handle) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = task.handle_table.remove(handle);
            return Some(errno::to_result(errno::EMFILE));
        }
    };
    if flags & O_CLOEXEC != 0 {
        let _ = abi.set_fd_flags(fd, FD_CLOEXEC);
    }
    Some(fd)
}

struct ProcTextFile {
    position: IrqRwSpinLock<usize>,
    content: String,
    path: String,
    directory: bool,
}

impl StreamOps for ProcTextFile {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut position = self.position.write();
        if self.directory {
            let name = match *position {
                0 => ".",
                1 => "..",
                n => match CPU_FILES.get(n - 2) {
                    Some(name) => name,
                    None => return Ok(0),
                },
            };
            let path = match name {
                "." => self.path.clone(),
                ".." => String::from("/sys/devices/system"),
                _ => format!("{}/{}", self.path, name),
            };
            let entry = DirectoryEntry::from_internal(&DirectoryEntryInternal {
                name: String::from(name),
                file_type: if *position < 2 {
                    FileType::Directory
                } else {
                    FileType::RegularFile
                },
                size: 0,
                file_id: metadata(&path, 0, *position < 2).file_id,
                metadata: None,
            });
            let size = core::mem::size_of::<DirectoryEntry>();
            if buffer.len() < size {
                return Err(StreamError::InvalidArgument);
            }
            // Same internal directory record consumed by read_linux_dirents_to_user.
            let bytes = unsafe {
                core::slice::from_raw_parts((&entry as *const DirectoryEntry).cast::<u8>(), size)
            };
            buffer[..size].copy_from_slice(bytes);
            *position += 1;
            return Ok(size);
        }
        let n = self.read_at(*position as u64, buffer)?;
        *position += n;
        Ok(n)
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::PermissionDenied)
    }
}

impl ControlOps for ProcTextFile {}

impl MemoryMappingOps for ProcTextFile {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        Err("cannot map proc text file")
    }
    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for ProcTextFile {
    fn wait_until_ready(
        &self,
        _interest: ReadyInterest,
        _trapframe: &mut Trapframe,
        _timeout_ns: Option<u64>,
        _min_wait_ns: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}

impl FileObject for ProcTextFile {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut position = self.position.write();
        let next = match whence {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::Current(offset) => *position as i128 + offset as i128,
            SeekFrom::End(offset) => {
                let end = if self.directory {
                    CPU_FILES.len() + 2
                } else {
                    self.content.len()
                };
                end as i128 + offset as i128
            }
        };
        if next < 0 || next > usize::MAX as i128 {
            return Err(StreamError::InvalidArgument);
        }
        *position = next as usize;
        Ok(*position as u64)
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if self.directory {
            return Err(StreamError::InvalidArgument);
        }
        let start = usize::try_from(offset).map_err(|_| StreamError::InvalidArgument)?;
        let bytes = self.content.as_bytes();
        if start >= bytes.len() {
            return Ok(0);
        }
        let n = buffer.len().min(bytes.len() - start);
        buffer[..n].copy_from_slice(&bytes[start..start + n]);
        Ok(n)
    }

    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(metadata(&self.path, self.content.len(), self.directory))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    #[test_case]
    fn proc_mounts_reports_only_selected_view_and_escapes_paths_with_real_restrictions() {
        use alloc::sync::Arc;
        use crate::fs::{VfsManager, drivers::tmpfs::TmpFS};
        let host = Arc::new(VfsManager::new());
        host.create_dir("/host-only").unwrap();
        host.mount(TmpFS::new(0), "/host-only", 8).unwrap();
        let private = VfsManager::view_rooted_at(&host, "/host-only").unwrap();
        private.create_dir("/space path\tback\\slash").unwrap();
        private.mount(TmpFS::new(0), "/space path\tback\\slash", 14).unwrap();
        let content = super::mounts_content(&private);
        assert!(content.contains("none / tmpfs rw,noexec 0 0\n"));
        assert!(content.contains("/space\\040path\\011back\\134slash tmpfs rw,nosuid,nodev,noexec 0 0\n"));
        assert!(!content.contains("/host-only"));
        assert_eq!(content.lines().count(), 2);
        private.unmount("/space path\tback\\slash").unwrap();
        assert_eq!(super::mounts_content(&private).lines().count(), 1);
    }

    #[test_case]
    fn stat_field_28_uses_initial_stack_pointer_and_preserves_comm_delimiters() {
        use crate::environment::{PAGE_SIZE, USER_STACK_END};
        let task = crate::task::new_user_task("a (b) c".into(), 0);
        task.allocate_stack_pages(USER_STACK_END - 2 * PAGE_SIZE, 2)
            .unwrap();
        let sp = USER_STACK_END - 256;
        task.vm_manager.record_process_entry_stack(sp).unwrap();
        let stat = super::task_stat(&task, false);
        assert!(stat.contains(" (a (b) c) R "));
        let end = stat.rfind(')').unwrap();
        let fields: alloc::vec::Vec<&str> = stat[end + 1..].split_whitespace().collect();
        assert_eq!(fields.len(), 50); // state (field 3) through exit_code (52)
        assert_eq!(fields[28 - 3].parse::<usize>().unwrap(), sp);
        assert_eq!(fields[23 - 3].parse::<usize>().unwrap(), 2 * PAGE_SIZE);
    }
}
