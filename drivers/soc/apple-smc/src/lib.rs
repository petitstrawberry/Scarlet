#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ptr;

use scarlet::device::DeviceInfo;
use scarlet::device::fdt::FdtManager;
use scarlet::device::manager::{DeviceManager, DriverPriority, PROBE_DEFER};
use scarlet::device::nvmem::NvmemCell;
use scarlet::device::platform::resource::PlatformDeviceResourceType;
use scarlet::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use scarlet::early_println;
use scarlet::sync::Mutex;
use scarlet::time;
use scarlet::vm;
use scarlet_driver_apple_asc::get_apple_asc_by_phandle;
use scarlet_driver_apple_rtkit::{AppleRtkit, RtkitMessage};

const SMC_ENDPOINT: u8 = 0x20;
const SMC_SHMEM_SIZE: usize = 0x1000;

const SMC_MSG_READ_KEY: u64 = 0x10;
const SMC_MSG_INITIALIZE: u64 = 0x17;
const SMC_MSG_NOTIFICATION: u64 = 0x18;

const SMC_TIMEOUT_US: u64 = 500_000;
const SMC_POLL_DELAY_US: u64 = 100;
const RTC_BYTES: usize = 6;
const RTC_BITS: u32 = 48;
const RTC_SEC_SHIFT: u32 = 15;
const NANOS_PER_SECOND: u64 = 1_000_000_000;

const SMC_KEY_CLKM: u32 = smc_key(*b"CLKM");

static SMC_REGISTRY: Mutex<Vec<Arc<AppleSmc>>> = Mutex::new(Vec::new());

const fn smc_key(bytes: [u8; 4]) -> u32 {
    ((bytes[0] as u32) << 24)
        | ((bytes[1] as u32) << 16)
        | ((bytes[2] as u32) << 8)
        | (bytes[3] as u32)
}

fn smc_msg_type(message: u64) -> u64 {
    message & 0xff
}

fn smc_msg_id(message: u64) -> u8 {
    ((message >> 12) & 0xf) as u8
}

fn smc_msg_size(message: u64) -> usize {
    ((message >> 16) & 0xff) as usize
}

fn smc_msg_data(message: u64) -> u32 {
    (message >> 32) as u32
}

/// Apple SMC endpoint client used for RTC wall-clock seeding.
pub struct AppleSmc {
    rtkit: AppleRtkit,
    sram_paddr: usize,
    sram_vaddr: usize,
    sram_size: usize,
    shmem_vaddr: Mutex<Option<usize>>,
    msg_id: Mutex<u8>,
}

impl AppleSmc {
    fn new(rtkit: AppleRtkit, sram_paddr: usize, sram_vaddr: usize, sram_size: usize) -> Self {
        Self {
            rtkit,
            sram_paddr,
            sram_vaddr,
            sram_size,
            shmem_vaddr: Mutex::new(None),
            msg_id: Mutex::new(0),
        }
    }

    fn boot(&self) -> Result<(), &'static str> {
        self.rtkit.wake()?;
        self.rtkit.start_ep(SMC_ENDPOINT)?;
        self.rtkit.send(&RtkitMessage {
            ep: SMC_ENDPOINT,
            msg: SMC_MSG_INITIALIZE,
        })?;

        let shmem_iova = self.wait_smc_message(SMC_TIMEOUT_US)?;
        self.set_shmem(shmem_iova)?;
        Ok(())
    }

    fn set_shmem(&self, iova: u64) -> Result<(), &'static str> {
        let sram_start = self.sram_paddr as u64;
        let sram_end = sram_start
            .checked_add(self.sram_size as u64)
            .and_then(|value| value.checked_sub(1))
            .ok_or("apple-smc: invalid SRAM resource")?;
        let shmem_end = iova
            .checked_add(SMC_SHMEM_SIZE as u64)
            .and_then(|value| value.checked_sub(1))
            .ok_or("apple-smc: invalid shared memory range")?;

        if iova < sram_start || shmem_end > sram_end {
            return Err("apple-smc: shared memory outside SRAM");
        }

        let offset = (iova - sram_start) as usize;
        *self.shmem_vaddr.lock() = Some(self.sram_vaddr + offset);
        Ok(())
    }

    fn wait_smc_message(&self, timeout_us: u64) -> Result<u64, &'static str> {
        let start = time::current_time();
        loop {
            let mut message = RtkitMessage { ep: 0, msg: 0 };
            if self.rtkit.recv(&mut message)? && message.ep == SMC_ENDPOINT {
                return Ok(message.msg);
            }

            if time::current_time().saturating_sub(start) >= timeout_us {
                return Err("apple-smc: timeout waiting for SMC message");
            }

            time::udelay(SMC_POLL_DELAY_US);
        }
    }

    fn read_key(&self, key: u32, buf: &mut [u8]) -> Result<usize, &'static str> {
        if buf.is_empty() || buf.len() > 255 {
            return Err("apple-smc: invalid read size");
        }

        let id = {
            let mut id = self.msg_id.lock();
            *id = (*id + 1) & 0xf;
            *id
        };
        let msg = SMC_MSG_READ_KEY
            | ((buf.len() as u64) << 16)
            | ((id as u64) << 12)
            | ((key as u64) << 32);

        self.rtkit.send(&RtkitMessage {
            ep: SMC_ENDPOINT,
            msg,
        })?;

        let reply = loop {
            let reply = self.wait_smc_message(SMC_TIMEOUT_US)?;
            if smc_msg_type(reply) != SMC_MSG_NOTIFICATION {
                break reply;
            }
        };

        if smc_msg_id(reply) != id {
            return Err("apple-smc: command sequence mismatch");
        }

        if smc_msg_type(reply) != 0 {
            return Err("apple-smc: command failed");
        }

        let returned = smc_msg_size(reply);
        if returned < buf.len() {
            return Err("apple-smc: short read");
        }

        if buf.len() <= 4 {
            let data = smc_msg_data(reply).to_le_bytes();
            buf.copy_from_slice(&data[..buf.len()]);
        } else {
            let shmem =
                (*self.shmem_vaddr.lock()).ok_or("apple-smc: shared memory not initialized")?;
            for (index, byte) in buf.iter_mut().enumerate() {
                // SAFETY: `shmem` was validated against the mapped SMC SRAM window.
                *byte = unsafe { ptr::read_volatile((shmem + index) as *const u8) };
            }
        }

        Ok(returned)
    }
}

fn read_le48(bytes: &[u8; RTC_BYTES]) -> u64 {
    let mut full = [0u8; 8];
    full[..RTC_BYTES].copy_from_slice(bytes);
    u64::from_le_bytes(full)
}

fn rtc_ticks_to_epoch_ns(
    counter: [u8; RTC_BYTES],
    offset: [u8; RTC_BYTES],
) -> Result<u64, &'static str> {
    let mask = (1u64 << RTC_BITS) - 1;
    let ticks = read_le48(&counter).wrapping_add(read_le48(&offset)) & mask;
    let signed = ((ticks << (64 - RTC_BITS)) as i64) >> (64 - RTC_BITS);
    let seconds = signed >> RTC_SEC_SHIFT;
    if seconds < 0 {
        return Err("apple-smc: RTC epoch is negative");
    }

    (seconds as u64)
        .checked_mul(NANOS_PER_SECOND)
        .ok_or("apple-smc: RTC epoch overflow")
}

fn read_be_u32(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 4 {
        return None;
    }
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_be_u32_cells(bytes: &[u8]) -> Option<Vec<u32>> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }

    let mut cells = Vec::new();
    for chunk in bytes.chunks_exact(4) {
        cells.push(u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Some(cells)
}

fn node_matches_platform_device(
    node: &fdt::node::FdtNode<'_, '_>,
    device: &PlatformDeviceInfo,
) -> bool {
    if node.name != device.name() {
        return false;
    }

    node.compatible().is_some_and(|compatibles| {
        compatibles
            .all()
            .any(|entry| device.compatible().contains(&entry))
    })
}

fn find_platform_node<'a>(
    fdt: &'a fdt::Fdt<'a>,
    device: &PlatformDeviceInfo,
) -> Option<fdt::node::FdtNode<'a, 'a>> {
    let mut stack = Vec::new();
    stack.push(fdt.find_node("/")?);

    while let Some(node) = stack.pop() {
        if node_matches_platform_device(&node, device) {
            return Some(node);
        }

        for child in node.children() {
            stack.push(child);
        }
    }

    None
}

fn string_list_index(bytes: &[u8], needle: &str) -> Option<usize> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .enumerate()
        .find_map(|(index, part)| {
            let entry = core::str::from_utf8(part).ok()?;
            (entry == needle).then_some(index)
        })
}

fn rtc_offset_cell(smc_node: &fdt::node::FdtNode<'_, '_>) -> Result<NvmemCell, &'static str> {
    let rtc_node = smc_node
        .children()
        .find(|child| child.name == "rtc")
        .ok_or("apple-smc: rtc child missing")?;
    let names = rtc_node
        .property("nvmem-cell-names")
        .ok_or("apple-smc: rtc nvmem-cell-names missing")?;
    let index =
        string_list_index(names.value, "rtc_offset").ok_or("apple-smc: rtc_offset missing")?;
    let cells = rtc_node
        .property("nvmem-cells")
        .ok_or("apple-smc: rtc nvmem-cells missing")?;
    let phandles = read_be_u32_cells(cells.value).ok_or("apple-smc: malformed nvmem-cells")?;
    let phandle = *phandles
        .get(index)
        .ok_or("apple-smc: rtc_offset phandle missing")?;

    DeviceManager::get_manager().resolve_nvmem_cell_by_phandle(phandle, "rtc_offset")
}

fn mailbox_phandle(device: &PlatformDeviceInfo) -> Result<u32, &'static str> {
    let mboxes = device
        .property("mboxes")
        .ok_or("apple-smc: mboxes missing")?;
    read_be_u32(mboxes.value()).ok_or("apple-smc: malformed mboxes")
}

fn seed_wall_clock(smc: &AppleSmc, rtc_offset: &NvmemCell) -> Result<(), &'static str> {
    let mono_before = time::current_time_ns();

    let mut counter = [0u8; RTC_BYTES];
    let read = smc.read_key(SMC_KEY_CLKM, &mut counter)?;
    if read != RTC_BYTES {
        return Err("apple-smc: CLKM returned unexpected size");
    }

    let mut offset = [0u8; RTC_BYTES];
    rtc_offset
        .read(&mut offset)
        .map_err(|_| "apple-smc: failed to read rtc_offset")?;

    let epoch_ns = rtc_ticks_to_epoch_ns(counter, offset)?;
    let mono_after = time::current_time_ns();

    match time::initialize_wall_clock_from_rtc_sample(epoch_ns, mono_before, mono_after) {
        Ok(()) => {
            early_println!("[apple-smc] seeded wall clock from SMC RTC");
            Ok(())
        }
        Err("wall clock already initialized") => {
            early_println!("[apple-smc] wall clock already initialized");
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mailbox_phandle = mailbox_phandle(device)?;
    let asc = get_apple_asc_by_phandle(mailbox_phandle).ok_or(PROBE_DEFER)?;

    let sram = device
        .get_resources()
        .iter()
        .filter(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .nth(1)
        .ok_or("apple-smc: no SRAM resource")?;
    let sram_paddr = sram.start;
    let sram_size = sram
        .end
        .checked_sub(sram.start)
        .and_then(|value| value.checked_add(1))
        .ok_or("apple-smc: invalid SRAM resource")?;
    let sram_vaddr = vm::ioremap(sram_paddr, sram_size).map_err(|_| "apple-smc: ioremap failed")?;

    let fdt = FdtManager::get_manager()
        .get_fdt()
        .ok_or("apple-smc: FDT unavailable")?;
    let smc_node = find_platform_node(fdt, device).ok_or("apple-smc: FDT node not found")?;
    let rtc_offset = rtc_offset_cell(&smc_node)?;

    let rtkit = AppleRtkit::new(asc);
    let smc = Arc::new(AppleSmc::new(rtkit, sram_paddr, sram_vaddr, sram_size));
    smc.boot()?;
    seed_wall_clock(&smc, &rtc_offset)?;
    SMC_REGISTRY.lock().push(smc);

    early_println!(
        "[apple-smc] probed {} SRAM={:#x}",
        device.name(),
        sram_paddr
    );
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_apple_smc_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-smc",
        probe_fn,
        remove_fn,
        alloc::vec!["apple,t8103-smc", "apple,smc"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_apple_smc_driver);

#[used]
static SCARLET_DRIVER_APPLE_SMC_ANCHOR: fn() = force_link;

/// Force the linker to keep the apple-smc driver object.
#[inline(never)]
pub fn force_link() {}
