//! Small proc/sys snapshots in the Linux ABI view.

use alloc::{format, string::String, sync::Arc, vec::Vec};
use core::{any::Any, arch::asm};

use scarlet_abi::network::{NetworkLinkInfoV1, NetworkReadLinkStatisticsV1};

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
const NETWORK_DIRECTORY: &str = "/sys/class/net";
const NETWORK_STATISTICS_DIRECTORY: &str = "statistics";
const NETWORK_STATISTICS_FILES: &[&str] = &["rx_bytes", "rx_packets", "tx_bytes", "tx_packets"];

/// Report a native address space, including ELF loader mappings.
/// Box64 uses these ranges before retrying a hinted mmap with MAP_FIXED.
fn task_maps(task: &Task) -> String {
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
    content
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
    fields[20 - 4] = crate::sched::scheduler::get_task_pool().task_ids_snapshot()
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

fn proc_task(caller: &Task, path: &str, field: &str) -> Option<Arc<Task>> {
    let path = path.trim_end_matches('/');
    let directory = path.strip_suffix(&format!("/{field}"))?;
    if let Some(target) = super::proc_fd::thread_directory_target(caller, directory) {
        return Some(target);
    }
    let self_path = format!("/proc/self/{field}");
    let thread_self_path = format!("/proc/thread-self/{field}");
    let id = if path == self_path {
        caller.get_thread_group_id()
    } else if path == thread_self_path {
        caller.get_id()
    } else {
        let local_pid = path
            .strip_prefix("/proc/")?
            .strip_suffix(&format!("/{field}"))?
            .parse::<usize>()
            .ok()?;
        caller.get_namespace().resolve_global_id(local_pid)?
    };
    crate::sched::scheduler::get_task_by_id(id)
}

fn maps_target(caller: &Task, path: &str) -> Option<Result<Arc<Task>, usize>> {
    let target = proc_task(caller, path, "maps")?;
    // Match process_vm_readv's existing address-space boundary. Namespace
    // visibility alone is insufficient for cross-process memory metadata.
    if !caller.vm_manager.shares_address_space(&target.vm_manager) {
        return Some(Err(errno::EPERM));
    }
    Some(Ok(target))
}

/// Android checks process identity by parsing the `Uid:` line from procfs.
/// Keep the fields that libcore and process-group users rely on, while deriving
/// every value from Scarlet's native task and credential state.
fn task_status(caller: &Task, task: &Task) -> String {
    use crate::task::TaskState;

    let namespace = caller.get_namespace();
    let local = |id| namespace.resolve_local_id(id).unwrap_or(0);
    let name = {
        let name = task.name.read();
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
    let credentials = task.credentials.read();
    let uids = credentials.principals();
    let gids = credentials.groups();
    let threads = crate::sched::scheduler::get_task_pool()
        .task_ids_snapshot()
        .into_iter()
        .filter(|id| {
            crate::sched::scheduler::get_task_by_id(*id).is_some_and(|member| {
                member.get_thread_group_id() == task.get_thread_group_id()
                    && !matches!(member.get_state(), TaskState::Zombie | TaskState::Terminated)
            })
        })
        .count()
        .max(1);

    format!(
        "Name:\t{name}\nState:\t{state}\nTgid:\t{}\nPid:\t{}\nPPid:\t{}\nTracerPid:\t0\nUid:\t{}\t{}\t{}\t{}\nGid:\t{}\t{}\t{}\t{}\nThreads:\t{threads}\n",
        local(task.get_thread_group_id()),
        local(task.get_id()),
        task.get_parent_id().map_or(0, local),
        uids.base.0,
        uids.active.0,
        uids.retained.0,
        uids.active.0,
        gids.base.0,
        gids.active.0,
        gids.retained.0,
        gids.active.0,
    )
}

fn task_syscall(task: &Task) -> String {
    let state = task.execution_debug_snapshot();
    if !state.syscall_active || state.syscall_number == u64::MAX {
        return String::from("running\n");
    }
    // Linux report_layout is syscall, six arguments, stack pointer, program
    // counter. Scarlet records the number and entry PC; it does not retain the
    // scalar argument registers after dispatch, so those fields stay zero.
    format!(
        "{} 0 0 0 0 0 0 0 0x{:x}\n",
        state.syscall_number, state.syscall_pc
    )
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

fn layout_field(path:&str)->Option<bool> {
    match path {"/proc/sys/vm/mmap_rnd_bits"=>Some(false),"/proc/sys/vm/mmap_rnd_compat_bits"=>Some(true),_=>None}
}
fn layout_text(path:&str)->Option<String> {
    let low=layout_field(path)?;let task=crate::task::mytask()?;let policy=task.vm_manager.layout_policy();
    Some(format!("{}\n",if low {policy.low_bits}else {policy.native_bits}))
}
fn text_content(path: &str) -> Option<String> {
    if let Some(content) = super::oom::text_content(path) {
        return Some(content);
    }
    if path == "/proc/version" {
        return Some(super::proc::linux_version_text());
    }
    if path == "/proc/meminfo" {
        let (total, free) = crate::mem::pmm::stats();
        let kib_per_page = crate::environment::PAGE_SIZE / 1024;
        // Expose existing PMM counters. Scarlet has no swap; unknown cache,
        // reclaimability and kernel-category statistics remain absent.
        return Some(format!("MemTotal: {} kB\nMemFree: {} kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n",
            total.saturating_mul(kib_per_page), free.saturating_mul(kib_per_page)));
    }
    if let Some(text)=layout_text(path) {return Some(text);}
    if path == "/proc/sys/kernel/kptr_restrict" {
        let caller = crate::task::mytask()?;
        return Some(String::from(if caller.execution_restrictions.read().kernel_addresses_hidden() { "2\n" } else { "0\n" }));
    }
    if matches!(path, "/proc/mounts" | "/proc/self/mounts" | "/proc/thread-self/mounts") {
        let fs = crate::task::mytask()?.get_vfs()?;
        return Some(mounts_content(&fs));
    }
    if path.trim_end_matches('/').ends_with("/stat") {
        let caller = crate::task::mytask()?;
        let target = proc_task(&caller, path, "stat")?;
        return Some(task_stat(&target, target.get_id() != target.get_thread_group_id()));
    }
    if path.trim_end_matches('/').ends_with("/status") {
        let caller = crate::task::mytask()?;
        let target = proc_task(&caller, path, "status")?;
        return Some(task_status(&caller, &target));
    }
    if path.trim_end_matches('/').ends_with("/syscall") {
        let caller = crate::task::mytask()?;
        let target = proc_task(&caller, path, "syscall")?;
        return Some(task_syscall(&target));
    }
    if path.trim_end_matches('/').ends_with("/maps") {
        let caller = crate::task::mytask()?;
        let target = maps_target(&caller, path)?.ok()?;
        return Some(task_maps(&target));
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
    if !cfg!(target_arch = "aarch64") {
        return false;
    }
    let path = path.trim_end_matches('/');
    if path == CPU_DIRECTORY {
        return true;
    }
    path.strip_prefix(CPU_DIRECTORY)
        .and_then(|suffix| suffix.strip_prefix("/cpu"))
        .and_then(|id| id.parse::<usize>().ok())
        .is_some_and(|id| {
            path == format!("{CPU_DIRECTORY}/cpu{id}")
                && crate::sched::scheduler::is_cpu_online(id)
        })
}

fn cpu_directory_entry(path: &str, index: usize) -> Option<(String, bool)> {
    match index {
        0 => Some((String::from("."), true)),
        1 => Some((String::from(".."), true)),
        _ if path.trim_end_matches('/') != CPU_DIRECTORY => None,
        _ => {
            let index = index - 2;
            if let Some(name) = CPU_FILES.get(index) {
                return Some((String::from(*name), false));
            }
            let wanted = index - CPU_FILES.len();
            let mut found = None;
            let mut position = 0;
            crate::sched::scheduler::for_each_online_cpu(|id| {
                if position == wanted {
                    found = Some((format!("cpu{id}"), true));
                }
                position += 1;
            });
            found
        }
    }
}

type ProcDirectoryEntry = (String, bool);

#[derive(Clone, Copy)]
enum NetworkStatistic {
    RxBytes,
    RxPackets,
    TxBytes,
    TxPackets,
}

impl NetworkStatistic {
    fn value(self, statistics: &NetworkReadLinkStatisticsV1) -> u64 {
        match self {
            Self::RxBytes => statistics.rx_bytes,
            Self::RxPackets => statistics.rx_packets,
            Self::TxBytes => statistics.tx_bytes,
            Self::TxPackets => statistics.tx_packets,
        }
    }
}

enum NetworkPath {
    Root,
    Interface,
    Statistics,
    Statistic(NetworkLinkInfoV1, NetworkStatistic),
}

enum NetworkSnapshot {
    Directory(Vec<ProcDirectoryEntry>),
    File(Vec<u8>),
}

#[cfg(feature = "network")]
fn network_path(path: &str) -> Option<NetworkPath> {
    let path = path.trim_end_matches('/');
    if path == NETWORK_DIRECTORY {
        return Some(NetworkPath::Root);
    }
    let suffix = path.strip_prefix(NETWORK_DIRECTORY)?.strip_prefix('/')?;
    let (name, tail) = match suffix.split_once('/') {
        Some((name, tail)) if !tail.is_empty() => (name, Some(tail)),
        Some(_) => return None,
        None => (suffix, None),
    };
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return None;
    }
    let link = crate::network::NetworkManager::get_manager()
        .link_snapshot()
        .into_iter()
        .find(|link| link.interface_name() == Some(name))?;
    match tail {
        None => Some(NetworkPath::Interface),
        Some("statistics") => Some(NetworkPath::Statistics),
        Some("statistics/rx_bytes") => Some(NetworkPath::Statistic(link, NetworkStatistic::RxBytes)),
        Some("statistics/rx_packets") => Some(NetworkPath::Statistic(link, NetworkStatistic::RxPackets)),
        Some("statistics/tx_bytes") => Some(NetworkPath::Statistic(link, NetworkStatistic::TxBytes)),
        Some("statistics/tx_packets") => Some(NetworkPath::Statistic(link, NetworkStatistic::TxPackets)),
        Some(_) => None,
    }
}

fn base_directory_entries() -> Vec<ProcDirectoryEntry> {
    alloc::vec![(String::from("."), true), (String::from(".."), true)]
}

fn network_root_directory_entries(links: &[NetworkLinkInfoV1]) -> Vec<ProcDirectoryEntry> {
    let mut entries = base_directory_entries();
    for link in links {
        if let Some(name) = link.interface_name()
            && !name.is_empty()
            && name != "."
            && name != ".."
            && !name.contains('/')
        {
            entries.push((String::from(name), true));
        }
    }
    entries
}

#[cfg(feature = "network")]
fn network_directory_entries(path: &str) -> Option<Vec<ProcDirectoryEntry>> {
    match network_path(path)? {
        NetworkPath::Root => Some(network_root_directory_entries(
            &crate::network::NetworkManager::get_manager().link_snapshot(),
        )),
        NetworkPath::Interface => {
            let mut entries = base_directory_entries();
            entries.push((String::from(NETWORK_STATISTICS_DIRECTORY), true));
            Some(entries)
        }
        NetworkPath::Statistics => {
            let mut entries = base_directory_entries();
            entries.extend(NETWORK_STATISTICS_FILES.iter().map(|name| (String::from(*name), false)));
            Some(entries)
        }
        NetworkPath::Statistic(_, _) => None,
    }
}

#[cfg(feature = "network")]
fn network_statistic_content(link: NetworkLinkInfoV1, statistic: NetworkStatistic) -> Option<Vec<u8>> {
    let statistics = crate::network::NetworkManager::get_manager()
        .read_link_statistics(NetworkReadLinkStatisticsV1::query(link.id, link.generation))
        .ok()?;
    Some(format!("{}\n", statistic.value(&statistics)).into_bytes())
}

#[cfg(feature = "network")]
fn network_snapshot(path: &str) -> Option<NetworkSnapshot> {
    match network_path(path)? {
        NetworkPath::Statistic(link, statistic) => {
            Some(NetworkSnapshot::File(network_statistic_content(link, statistic)?))
        }
        _ => Some(NetworkSnapshot::Directory(network_directory_entries(path)?)),
    }
}

#[cfg(not(feature = "network"))]
fn network_snapshot(_path: &str) -> Option<NetworkSnapshot> { None }

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

fn snapshot_content(path: &str) -> Option<alloc::vec::Vec<u8>> {
    if path == "/proc/config.gz" {
        Some(super::config::GZIP.to_vec())
    } else {
        text_content(path).map(String::into_bytes)
    }
}

pub(super) fn path_metadata(path: &str) -> Option<FileMetadata> {
    if let Some(metadata) = super::oom::path_metadata(path) {
        return Some(metadata);
    }
    if let Some(snapshot) = network_snapshot(path) {
        return Some(match snapshot {
            NetworkSnapshot::Directory(_) => metadata(path, 0, true),
            NetworkSnapshot::File(content) => metadata(path, content.len(), false),
        });
    }
    if is_cpu_directory(path) {
        Some(metadata(path, 0, true))
    } else {
        snapshot_content(path).map(|content| metadata(path, content.len(), false))
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
    if let Some(fd) = super::oom::open(abi, task, path, flags) {
        return Some(fd);
    }
    if path.trim_end_matches('/').ends_with("/maps")
        && let Some(Err(error)) = maps_target(task, path)
    {
        return Some(errno::to_result(error));
    }
    let cpu_directory = is_cpu_directory(path);
    let (network_directory, network_content, network_entries) = match network_snapshot(path) {
        Some(NetworkSnapshot::Directory(entries)) => (true, None, Some(entries)),
        Some(NetworkSnapshot::File(content)) => (false, Some(content), None),
        None => (false, None, None),
    };
    let directory = cpu_directory || network_directory;
    let content = if directory {
        alloc::vec::Vec::new()
    } else if let Some(content) = network_content {
        content
    } else {
        snapshot_content(path)?
    };
    if flags & 3 == 3 || (flags & 3 != 0 && path != "/proc/sys/kernel/kptr_restrict" && layout_field(path).is_none()) {
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
        directory_entries: network_entries,
        access: flags & 3,
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
    content: alloc::vec::Vec<u8>,
    path: String,
    directory: bool,
    /// Network entries are captured at open; CPU directories retain their
    /// scheduler-backed live iteration.
    directory_entries: Option<Vec<ProcDirectoryEntry>>,
    access: i32,
}

impl StreamOps for ProcTextFile {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut position = self.position.write();
        if self.directory {
            let entry = self.directory_entries.as_ref()
                .and_then(|entries| entries.get(*position).cloned())
                .or_else(|| cpu_directory_entry(&self.path, *position));
            let Some((name, directory)) = entry else {
                return Ok(0);
            };
            let path = match name.as_str() {
                "." => self.path.clone(),
                ".." => self.path.rsplit_once('/').map(|(parent, _)| String::from(parent)).unwrap_or_default(),
                _ => format!("{}/{}", self.path, name),
            };
            let entry = DirectoryEntry::from_internal(&DirectoryEntryInternal {
                name,
                file_type: if directory {
                    FileType::Directory
                } else {
                    FileType::RegularFile
                },
                size: 0,
                file_id: metadata(&path, 0, directory).file_id,
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

    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        if self.access==0 {return Err(StreamError::PermissionDenied);}
        if let Some(low)=layout_field(&self.path) {
            let caller=crate::task::mytask().ok_or(StreamError::PermissionDenied)?;
            if !caller.credentials.read().permits(crate::shield::identity::MANAGE_LAYOUT) {return Err(StreamError::PermissionDenied);}
            let value=core::str::from_utf8(buffer).map_err(|_|StreamError::InvalidArgument)?.trim().parse::<u8>().map_err(|_|StreamError::InvalidArgument)?;
            let mut policy=caller.vm_manager.layout_policy();if low {policy.low_bits=value;}else {policy.native_bits=value;}
            policy.validate().map_err(|_|StreamError::InvalidArgument)?;
            caller.vm_manager.configure_layout(policy).map_err(|_|StreamError::NotSupported)?;return Ok(buffer.len());
        }
        if self.path!="/proc/sys/kernel/kptr_restrict" {return Err(StreamError::PermissionDenied);}
        let caller = crate::task::mytask().ok_or(StreamError::PermissionDenied)?;
        let value = core::str::from_utf8(buffer).map_err(|_| StreamError::InvalidArgument)?.trim();
        if !matches!(value, "0" | "1" | "2") { return Err(StreamError::InvalidArgument); }
        // The manager installs the inherited constraint before launching Android.
        // A proc write may confirm it, but cannot impersonate a global kernel
        // policy change or loosen a stronger native restriction.
        if value != "2" || !caller.execution_restrictions.read().kernel_addresses_hidden() { return Err(StreamError::PermissionDenied); }
        Ok(buffer.len())
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
                    if let Some(entries) = &self.directory_entries {
                        entries.len()
                    } else if self.path == CPU_DIRECTORY {
                        CPU_FILES.len() + 2 + crate::sched::scheduler::num_online_cpus()
                    } else {
                        2
                    }
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
        if self.access == 1 { return Err(StreamError::PermissionDenied); }
        if self.directory {
            return Err(StreamError::InvalidArgument);
        }
        let start = usize::try_from(offset).map_err(|_| StreamError::InvalidArgument)?;
        let layout=layout_text(&self.path);
        let bytes = if let Some(layout)=&layout {layout.as_bytes()}else if self.path == "/proc/sys/kernel/kptr_restrict" {
            if crate::task::mytask().is_some_and(|task| task.execution_restrictions.read().kernel_addresses_hidden()) { b"2\n".as_slice() }
            else { b"0\n".as_slice() }
        } else { self.content.as_slice() };
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
    use super::*;
    #[cfg(feature = "network")]
    use crate::{
        device::network::{DevicePacket, MacAddress},
        network::{InterfaceStats, NetworkInterface, NetworkManager},
    };

    #[cfg(feature = "network")]
    struct NetworkStatsTestInterface {
        name: &'static str,
        statistics: Arc<IrqRwSpinLock<InterfaceStats>>,
    }

    #[cfg(feature = "network")]
    impl NetworkInterface for NetworkStatsTestInterface {
        fn name(&self) -> &str { self.name }
        fn mac_address(&self) -> MacAddress { MacAddress::new([2, 0, 0, 0, 0, 1]) }
        fn link_state(&self) -> u8 { scarlet_abi::network::LINK_UP }
        fn ip_address(&self) -> Option<crate::network::ipv4::Ipv4Address> { None }
        fn set_ip_address(&self, _: crate::network::ipv4::Ipv4Address) {}
        fn clear_ip_address(&self) {}
        fn send(&self, _: DevicePacket) -> Result<(), &'static str> {
            Err("network statistics test interface")
        }
        fn poll(&self) -> Result<Vec<DevicePacket>, &'static str> { Ok(Vec::new()) }
        fn stats(&self) -> InterfaceStats { self.statistics.read().clone() }
    }

    fn file_contents(file: &dyn FileObject) -> Vec<u8> {
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut contents = Vec::new();
        let mut buffer = [0; 16];
        loop {
            let read = file.read(&mut buffer).unwrap();
            if read == 0 { return contents; }
            contents.extend_from_slice(&buffer[..read]);
        }
    }

    fn directory_names(file: &dyn FileObject) -> Vec<String> {
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut names = Vec::new();
        let mut buffer = [0; core::mem::size_of::<DirectoryEntry>()];
        loop {
            let read = file.read(&mut buffer).unwrap();
            if read == 0 { return names; }
            assert_eq!(read, buffer.len());
            names.push(String::from(DirectoryEntry::parse(&buffer).unwrap().name_str().unwrap()));
        }
    }

    fn open_directory(task: &Task, abi: &mut LinuxAbi, path: &str) -> usize {
        super::open_proc_text_file(abi, task, path, O_DIRECTORY).unwrap()
    }

    fn read_synthetic_file(task: &Task, abi: &mut LinuxAbi, path: &str) -> Vec<u8> {
        let fd = super::open_proc_text_file(abi, task, path, 0).unwrap();
        let object = task.handle_table.get(abi.get_handle(fd).unwrap()).unwrap();
        file_contents(object.as_file().unwrap())
    }

    #[test_case]
    fn config_gzip_has_consistent_binary_reads_metadata_and_read_only_access() {
        use super::*;
        let task = crate::task::new_user_task("proc-config".into(), 0);
        let mut abi = LinuxAbi::default();
        let fd = open_proc_text_file(&mut abi, &task, "/proc/config.gz", O_CLOEXEC).unwrap();
        assert_eq!(abi.get_fd_flags(fd), Some(FD_CLOEXEC));
        let object = task.handle_table.get(abi.get_handle(fd).unwrap()).unwrap();
        let file = object.as_file().unwrap();
        let expected = super::super::config::GZIP;
        assert_eq!(file.metadata().unwrap().size, expected.len());
        assert_eq!(path_metadata("/proc/config.gz").unwrap().size, expected.len());
        let mut actual = alloc::vec::Vec::new();
        let mut chunk = [0; 7];
        loop {
            let n = file.read(&mut chunk).unwrap();
            if n == 0 { break; }
            actual.extend_from_slice(&chunk[..n]);
        }
        assert_eq!(actual, expected);
        assert_eq!(file.seek(SeekFrom::Start(0)).unwrap(), 0);
        assert_eq!(file.read(&mut chunk).unwrap(), chunk.len());
        assert_eq!(&chunk, &expected[..7]);
        assert!(matches!(file.write(b"CONFIG_UNIMPLEMENTED=y\n"), Err(StreamError::PermissionDenied)));
        assert_eq!(open_proc_text_file(&mut abi, &task, "/proc/config.gz", 1), Some(errno::to_result(errno::EACCES)));
        assert_eq!(open_proc_text_file(&mut abi, &task, "/proc/config.gz", O_DIRECTORY), Some(errno::to_result(errno::ENOTDIR)));
    }

    #[test_case]
    fn proc_version_matches_uname_release() {
        let version = super::text_content("/proc/version").unwrap();
        assert!(version.starts_with(&alloc::format!("Linux version {} ", super::super::proc::LINUX_ABI_RELEASE)));
        assert!(version.ends_with('\n'));
    }

    #[test_case]
    fn stat_num_threads_counts_blocked_members_outside_run_queues() {
        use crate::sched::scheduler::{get_all_task_ids, get_task_by_id, register_task, reset};
        use crate::task::{BlockedType, Task, TaskState, TaskType};

        reset();
        let leader_id = register_task(Task::new("StatLeader".into(), 0, TaskType::Kernel));
        let leader = get_task_by_id(leader_id).unwrap();
        let mut members = alloc::vec::Vec::new();
        for state in [
            TaskState::Blocked(BlockedType::Interruptible),
            TaskState::Blocked(BlockedType::Uninterruptible),
            TaskState::Zombie,
            TaskState::Terminated,
        ] {
            let mut member = Task::new("StatThread".into(), 0, TaskType::Kernel);
            member.set_thread_group_id(leader_id);
            member.set_state(state);
            let id = register_task(member);
            members.push(get_task_by_id(id).unwrap());
        }
        register_task(Task::new("OtherProcess".into(), 0, TaskType::Kernel));
        assert!(!get_all_task_ids().contains(&members[0].get_id()));
        assert!(!get_all_task_ids().contains(&members[1].get_id()));

        for (task, thread) in [(&leader, false), (&members[0], true)] {
            let stat = super::task_stat(task, thread);
            let fields: alloc::vec::Vec<_> = stat[stat.rfind(')').unwrap() + 1..]
                .split_whitespace()
                .collect();
            assert_eq!(fields[20 - 3], "3");
            assert_eq!(fields[0], if thread { "S" } else { "R" });
        }

        drop(members);
        drop(leader);
        reset();
    }

    #[test_case]
    fn status_exposes_credentials_for_android_process_liveness_checks() {
        use crate::sched::scheduler::{get_task_by_id, register_task, reset};
        use crate::shield::identity::{Group, Identities, Principal};
        use crate::task::{BlockedType, Task, TaskState, TaskType};

        reset();
        let leader_id = register_task(Task::new("StatusLeader".into(), 0, TaskType::User));
        let leader = get_task_by_id(leader_id).unwrap();
        leader
            .credentials
            .write()
            .set_principals(Identities::all(Principal(1000)))
            .unwrap();
        leader
            .credentials
            .write()
            .set_groups(Identities::all(Group(1000)))
            .unwrap();
        let mut member = Task::new("BlockedStatus".into(), 0, TaskType::User);
        member.set_thread_group_id(leader_id);
        member.set_state(TaskState::Blocked(BlockedType::Interruptible));
        let member_id = register_task(member);
        assert_eq!(get_task_by_id(member_id).unwrap().get_thread_group_id(), leader_id);

        let local_pid = leader.get_namespace_id();
        crate::task::set_mock_current_task(leader.clone());
        let self_status = super::text_content("/proc/self/status").unwrap();
        let numeric_status = super::text_content(&alloc::format!("/proc/{local_pid}/status")).unwrap();
        let numeric_stat = super::text_content(&alloc::format!("/proc/{local_pid}/stat")).unwrap();
        assert_eq!(numeric_stat, super::text_content("/proc/self/stat").unwrap());
        assert_eq!(self_status, numeric_status);
        assert!(self_status.contains("Name:\tStatusLeader\n"));
        assert!(self_status.contains("Uid:\t1000\t1000\t1000\t1000\n"));
        assert!(self_status.contains("Gid:\t1000\t1000\t1000\t1000\n"));
        assert!(self_status.contains("Threads:\t2\n"));
        crate::task::clear_mock_current_task();
        drop(leader);
        reset();
    }

    #[test_case]
    fn numeric_maps_require_the_process_memory_address_space_authority() {
        use super::{LinuxAbi,errno,O_CLOEXEC};
        use crate::environment::{PAGE_SIZE, USER_STACK_END};
        use crate::sched::scheduler::{get_task_by_id, register_task, reset};
        use crate::task::{Task, TaskType};

        reset();
        let caller = Task::new("MapCaller".into(), 0, TaskType::User);
        let caller_id = register_task(caller);
        let caller = get_task_by_id(caller_id).unwrap();
        let target = Task::new("MapTarget".into(), 0, TaskType::User);
        let target_id = register_task(target);
        let target = get_task_by_id(target_id).unwrap();
        target.allocate_stack_pages(USER_STACK_END - PAGE_SIZE, 1).unwrap();
        let maps_path = alloc::format!("/proc/{}/maps", target.get_namespace_id());

        crate::task::set_mock_current_task(target.clone());
        let maps = super::text_content(&maps_path).unwrap();
        let start = USER_STACK_END - PAGE_SIZE;
        assert!(maps.contains(&alloc::format!("{start:08x}-{USER_STACK_END:08x} rw-p")));

        crate::task::set_mock_current_task(caller.clone());
        assert!(super::text_content(&maps_path).is_none());
        let mut abi = LinuxAbi::default();
        assert_eq!(
            super::open_proc_text_file(&mut abi, &caller, &maps_path, O_CLOEXEC),
            Some(errno::to_result(errno::EPERM))
        );

        crate::task::clear_mock_current_task();
        drop(target);
        drop(caller);
        reset();
    }

    #[cfg(target_arch="aarch64")]
    #[test_case]
    fn cpu_directory_exposes_real_sparse_cpu_ids_for_bionic_nprocs_conf() {
        use super::*;
        use crate::sched::scheduler::{reset,register_online_cpu};
        reset();register_online_cpu(0);register_online_cpu(2);
        assert_eq!(cpu_list(),"0,2\n");
        assert_eq!(cpu_directory_entry(CPU_DIRECTORY,5),Some((String::from("cpu0"),true)));
        assert_eq!(cpu_directory_entry(CPU_DIRECTORY,6),Some((String::from("cpu2"),true)));
        assert_eq!(cpu_directory_entry(CPU_DIRECTORY,7),None);
        assert!(is_cpu_directory("/sys/devices/system/cpu/cpu2"));
        assert!(!is_cpu_directory("/sys/devices/system/cpu/cpu1"));
        assert!(!is_cpu_directory("/sys/devices/system/cpu/cpu02"));
        assert_eq!(cpu_directory_entry("/sys/devices/system/cpu/cpu2",2),None);
        reset();
    }

    #[cfg(feature = "network")]
    #[test_case]
    fn network_directories_are_native_read_only_and_relative_openable() {
        let name = "sysfsdir0";
        let manager = NetworkManager::get_manager();
        manager.register_interface(name, Arc::new(NetworkStatsTestInterface {
            name,
            statistics: Arc::new(IrqRwSpinLock::new(InterfaceStats::default())),
        })).unwrap();
        let task = crate::task::new_user_task("network-directory".into(), 0);
        let mut abi = LinuxAbi::default();
        let root_fd = open_directory(&task, &mut abi, NETWORK_DIRECTORY);
        let interface_path = format!("{NETWORK_DIRECTORY}/{name}");
        let interface_fd = open_directory(&task, &mut abi, &interface_path);
        let statistics_path = format!("{interface_path}/{NETWORK_STATISTICS_DIRECTORY}");
        let statistics_fd = open_directory(&task, &mut abi, &statistics_path);
        let root = task.handle_table.get(abi.get_handle(root_fd).unwrap()).unwrap();
        let interface = task.handle_table.get(abi.get_handle(interface_fd).unwrap()).unwrap();
        let statistics = task.handle_table.get(abi.get_handle(statistics_fd).unwrap()).unwrap();
        let root_file = root.as_file().unwrap();
        let interface_file = interface.as_file().unwrap();
        let statistics_file = statistics.as_file().unwrap();

        // These are the exact paths consumed by ABI fs::openat for dirfd walks.
        assert_eq!(directory_path(root_file), Some(NETWORK_DIRECTORY));
        assert_eq!(directory_path(interface_file), Some(interface_path.as_str()));
        assert_eq!(directory_path(statistics_file), Some(statistics_path.as_str()));
        assert_eq!(directory_names(interface_file), alloc::vec![
            String::from("."), String::from(".."), String::from("statistics"),
        ]);
        assert_eq!(directory_names(statistics_file), alloc::vec![
            String::from("."), String::from(".."), String::from("rx_bytes"),
            String::from("rx_packets"), String::from("tx_bytes"), String::from("tx_packets"),
        ]);
        let root_entries = directory_names(root_file);
        assert!(root_entries.contains(&String::from(name)));
        assert!(matches!(root_file.write(b"x"), Err(StreamError::PermissionDenied)));

        manager.unregister_interface(name);
        assert_eq!(directory_names(root_file), root_entries);
        assert!(path_metadata(&interface_path).is_none());
        assert!(path_metadata(&statistics_path).is_none());
        let current_fd = open_directory(&task, &mut abi, NETWORK_DIRECTORY);
        let current = task.handle_table.get(abi.get_handle(current_fd).unwrap()).unwrap();
        assert!(!directory_names(current.as_file().unwrap()).contains(&String::from(name)));
        assert_eq!(network_root_directory_entries(&[]), base_directory_entries());
    }

    #[cfg(feature = "network")]
    #[test_case]
    fn network_statistics_use_real_counters_and_per_open_file_snapshots() {
        let name = "sysfsstats0";
        let statistics = Arc::new(IrqRwSpinLock::new(InterfaceStats {
            rx_packets: 23,
            rx_bytes: 1234,
            tx_packets: 17,
            tx_bytes: 0x1234,
            drops: 2,
            errors: 3,
        }));
        let manager = NetworkManager::get_manager();
        manager.register_interface(name, Arc::new(NetworkStatsTestInterface {
            name,
            statistics: Arc::clone(&statistics),
        })).unwrap();
        let task = crate::task::new_user_task("network-statistics".into(), 0);
        let mut abi = LinuxAbi::default();
        let base = format!("{NETWORK_DIRECTORY}/{name}/{NETWORK_STATISTICS_DIRECTORY}");
        assert_eq!(read_synthetic_file(&task, &mut abi, &format!("{base}/rx_bytes")).as_slice(), b"1234\n".as_slice());
        assert_eq!(read_synthetic_file(&task, &mut abi, &format!("{base}/rx_packets")).as_slice(), b"23\n".as_slice());
        assert_eq!(read_synthetic_file(&task, &mut abi, &format!("{base}/tx_bytes")).as_slice(), b"4660\n".as_slice());
        assert_eq!(read_synthetic_file(&task, &mut abi, &format!("{base}/tx_packets")).as_slice(), b"17\n".as_slice());

        let rx_bytes = format!("{base}/rx_bytes");
        let fd = super::open_proc_text_file(&mut abi, &task, &rx_bytes, 0).unwrap();
        let object = task.handle_table.get(abi.get_handle(fd).unwrap()).unwrap();
        let file = object.as_file().unwrap();
        assert_eq!(file_contents(file).as_slice(), b"1234\n".as_slice());
        statistics.write().rx_bytes = 9876;
        assert_eq!(file_contents(file).as_slice(), b"1234\n".as_slice());
        assert_eq!(read_synthetic_file(&task, &mut abi, &rx_bytes).as_slice(), b"9876\n".as_slice());
        assert_eq!(file.seek(SeekFrom::Start(0)).unwrap(), 0);
        let mut at = [0; 2];
        assert_eq!(file.read_at(1, &mut at).unwrap(), 2);
        assert_eq!(&at, b"23");
        let mut positioned = [0; 2];
        assert_eq!(file.read(&mut positioned).unwrap(), 2);
        assert_eq!(&positioned, b"12");
        assert_eq!(file.seek(SeekFrom::Start(1)).unwrap(), 1);
        let mut next = [0; 2];
        assert_eq!(file.read(&mut next).unwrap(), 2);
        assert_eq!(&next, b"23");
        assert!(matches!(file.write(b"0\n"), Err(StreamError::PermissionDenied)));
        assert_eq!(super::open_proc_text_file(&mut abi, &task, &rx_bytes, 1), Some(errno::to_result(errno::EACCES)));
        let unknown = format!("{base}/unknown");
        assert!(path_metadata(&unknown).is_none());
        assert_eq!(super::open_proc_text_file(&mut abi, &task, &unknown, 0), None);
        manager.unregister_interface(name);
        assert!(path_metadata(&rx_bytes).is_none());
    }

    #[test_case]
    fn kptr_control_confirms_a_real_inherited_constraint_and_never_loosens_it() {
        use super::*;
        let task = Arc::new(crate::task::new_user_task("kptr-control".into(), 0));
        crate::task::set_mock_current_task(task.clone());
        let file = ProcTextFile { position: IrqRwSpinLock::new(0), content: b"0\n".to_vec(),
            path: String::from("/proc/sys/kernel/kptr_restrict"), directory: false,
            directory_entries: None, access: 2 };
        assert!(file.write(b"2").is_err()); // Cannot fabricate a global policy setting.
        crate::shield::information::constrain(&task, 1).unwrap();
        assert_eq!(file.write(b"2\n").unwrap(), 2);
        assert!(file.write(b"0").is_err()); assert!(file.write(b"1").is_err());
        assert!(file.write(b"invalid").is_err());
        let mut value = [0; 2]; assert_eq!(file.read(&mut value).unwrap(), 2); assert_eq!(&value, b"2\n");
        crate::task::clear_mock_current_task();
    }

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
