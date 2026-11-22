//! Host-specific kernel platform setup.

pub(crate) mod cpu;
pub(crate) mod execution;
pub(crate) mod file_mapping;
pub(crate) mod interrupts;
mod memory;

pub use interrupts::raise_external as raise_external_interrupt;

use alloc::sync::Arc;
use core::ffi::c_void;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    environment::MAX_NUM_CPUS,
    random::{EntropySource, RandomManager},
};

pub const DEFAULT_VCPUS: usize = 4;

/// Minimum protection granule available to a hosted native executable.
pub(crate) fn executable_page_size() -> usize {
    scarlet_hosted_runtime::ram::host_page_size()
        .ok()
        .filter(|size| *size >= crate::environment::PAGE_SIZE)
        .expect("Invalid hosted executable page size")
}

/// The BSP thread's ownership of its logical CPU and the initialized kernel.
pub struct Platform {
    _bsp: cpu::CpuBinding,
    cpu_count: usize,
    aps_started: bool,
}

impl Platform {
    /// Enter the common kernel boot path with the BSP's initramfs and devices.
    pub fn boot(mut self, initramfs: &[u8], cmdline: &str, devices: crate::DeviceSource) -> ! {
        let area = crate::vm::vmem::MemoryArea::from_ptr(initramfs.as_ptr(), initramfs.len())
            .expect("Invalid initramfs range");
        crate::sched::scheduler::register_boot_cpu(0);
        crate::start_prepared_kernel(crate::PreparedBoot {
            cpu_id: 0,
            cpu_count: self.cpu_count,
            cmdline,
            device_source: devices,
            initramfs: Some(area),
            probe_buses: || {},
            seed_wall_clock: || {},
            probe_secondary_cpus: None,
            start_secondary_cpus: || {
                self.start_secondary_cpus()
                    .expect("Cannot start secondary vCPUs")
            },
        })
    }

    pub fn cpu_count(&self) -> usize {
        self.cpu_count
    }

    /// Bring up the secondary vCPUs after the BSP has claimed its first Task.
    /// No AP enters a Task until all configured CPUs have initialized their
    /// per-CPU interrupt controller, timer, and scheduler.
    pub fn start_secondary_cpus(&mut self) -> Result<(), &'static str> {
        use crate::sched::scheduler::{current_task_id, scheduler_ready, task_capacity_snapshot};

        if self.aps_started || !scheduler_ready(0) || current_task_id(0).is_none() {
            return Err("Secondary vCPUs require a claimed BSP Task and a single release");
        }

        let capacity = task_capacity_snapshot();
        let secondary_count = self.cpu_count - 1;
        if capacity.active_kernel + capacity.pending_kernel + secondary_count
            > capacity.limits.max_kernel
        {
            return Err("Task capacity cannot accommodate all secondary idle Tasks");
        }
        scarlet_hosted_runtime::smp::start_secondaries(
            1..self.cpu_count,
            cpu::CpuBinding::bind,
            prepare_secondary,
            |next| crate::enter_first_task(Some(next)),
        )?;
        self.aps_started = true;
        Ok(())
    }
}

fn prepare_secondary(cpu_id: usize) -> Result<usize, &'static str> {
    crate::interrupt::InterruptManager::global().init_controllers_for_cpu(cpu_id as u32);
    crate::timer::get_kernel_timer().init(cpu_id);
    crate::interrupt::enable_cpu_interrupts();
    crate::prepare_secondary_scheduler(cpu_id)
        .ok_or("Secondary scheduler did not claim an initial Task")
}

/// Initialize the host allocator boundary, vCPU identity, PMM, and initcalls.
pub fn prepare(cpu_count: usize, ram_bytes: usize) -> Result<Platform, &'static str> {
    if !(2..=MAX_NUM_CPUS).contains(&cpu_count) {
        return Err("Hosted vCPU count is outside Scarlet's supported range");
    }
    #[cfg(target_os = "linux")]
    if scarlet_hosted_runtime::ram::host_page_size().ok() != Some(crate::environment::PAGE_SIZE) {
        return Err("AArch64 Linux hosted Scarlet requires a 4 KiB host page size");
    }
    let bsp = cpu::CpuBinding::bind(0)?;
    let area = memory::initialize(ram_bytes)?;
    crate::mem::pmm::add_region(area)?;
    interrupts::initialize(cpu_count)?;
    seed_wall_clock()?;
    RandomManager::register_entropy_source(Arc::new(HostEntropy));
    crate::initcall::early::early_initcall_call();
    crate::initcall::driver::driver_initcall_call();
    Ok(Platform {
        _bsp: bsp,
        cpu_count,
        aps_started: false,
    })
}

fn seed_wall_clock() -> Result<(), &'static str> {
    let before = crate::time::current_time_ns();
    let utc = SystemTime::now();
    let after = crate::time::current_time_ns();
    let elapsed = utc
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Host clock predates the Unix epoch")?;
    let unix_ns =
        u64::try_from(elapsed.as_nanos()).map_err(|_| "Host clock exceeds the wall clock range")?;
    crate::time::initialize_wall_clock_from_rtc_sample(unix_ns, before, after)
}

struct HostEntropy;

unsafe extern "C" {
    fn arc4random_buf(buffer: *mut c_void, size: usize);
}

impl EntropySource for HostEntropy {
    fn name(&self) -> &'static str {
        "host system entropy"
    }

    fn read_entropy(&self, buffer: &mut [u8]) -> usize {
        if !buffer.is_empty() {
            // SAFETY: The caller supplies writable storage for the entire length.
            unsafe { arc4random_buf(buffer.as_mut_ptr().cast(), buffer.len()) };
        }
        buffer.len()
    }

    fn is_available(&self) -> bool {
        true
    }
}

pub(crate) use memory::{SHARED_OBJECT_LIMIT, SharedBacking, alias_for, logical_address, ram_area};

/// Load a guest executable through the existing ELF loader and fault its
/// entry page into the software mapping table. This leaves it parked.
pub fn inspect_executable(
    vfs: alloc::sync::Arc<crate::fs::vfs_v2::manager::VfsManager>,
    path: &str,
) -> Result<u64, alloc::string::String> {
    use crate::{
        environment::PAGE_SIZE,
        object::capability::memory_mapping::{AccessKind, AccessOp},
        vm::vmem::{MemoryAttribute, VirtualMemoryPermission},
    };
    use alloc::{format, string::ToString};

    let object = vfs
        .open(path, 0)
        .map_err(|error| format!("{path}: {error:?}"))?;
    let file = object
        .as_file()
        .ok_or_else(|| format!("{path}: expected a file"))?;
    let task = crate::task::new_user_task(path.to_string(), 0);
    task.init();
    *task.vfs.write() = Some(vfs);
    let entry = crate::task::elf_loader::load_elf_into_task(file, &task)
        .map_err(|error| format!("{path}: {error:?}"))?;
    let address = usize::try_from(entry).map_err(|_| format!("{path}: invalid entry address"))?;
    task.vm_manager
        .lazy_map_page_with(AccessKind {
            op: AccessOp::Instruction,
            vaddr: address,
            size: Some(4),
        })
        .map_err(|error| format!("{path}: {error}"))?;
    let mut root = task
        .vm_manager
        .get_root_page_table()
        .ok_or("Missing hosted page table")?;
    let page_base = address & !(PAGE_SIZE - 1);
    let entry_page = root
        .resident_pages()
        .into_iter()
        .find_map(|(vaddr, page)| (vaddr == page_base).then_some(page))
        .ok_or("Entry page was not published")?;
    if !VirtualMemoryPermission::Execute.contained_in(entry_page.permissions)
        || entry_page.memory_attribute != MemoryAttribute::Normal
        || alias_for(entry_page.paddr).is_none()
    {
        return Err("Entry page lacks executable shared backing".into());
    }
    Ok(entry)
}
