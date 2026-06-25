#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};

use scarlet::{
    arch::mmio,
    device::{
        DeviceInfo,
        clk::ClkHandle,
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
        watchdog::{Watchdog, WatchdogError},
    },
    driver_initcall, early_println,
};

const APPLE_WDT_WD1_CUR_TIME: usize = 0x10;
const APPLE_WDT_WD1_BITE_TIME: usize = 0x14;
const APPLE_WDT_WD1_CTRL: usize = 0x1c;
const APPLE_WDT_CTRL_RESET_EN: u32 = 1 << 2;
const APPLE_WDT_DEFAULT_TIMEOUT_MS: u32 = 30_000;
const APPLE_WDT_DEFAULT_CLK_HZ: u64 = 24_000_000;

struct AppleWdt {
    base_addr: usize,
    clk_rate_hz: u64,
    timeout_ms: AtomicU32,
    _clk: Option<ClkHandle>,
}

impl AppleWdt {
    const fn new(base_addr: usize, clk_rate_hz: u64, clk: Option<ClkHandle>) -> Self {
        Self {
            base_addr,
            clk_rate_hz,
            timeout_ms: AtomicU32::new(0),
            _clk: clk,
        }
    }

    #[inline]
    fn cur_time_addr(&self) -> usize {
        self.base_addr + APPLE_WDT_WD1_CUR_TIME
    }

    #[inline]
    fn bite_time_addr(&self) -> usize {
        self.base_addr + APPLE_WDT_WD1_BITE_TIME
    }

    #[inline]
    fn ctrl_addr(&self) -> usize {
        self.base_addr + APPLE_WDT_WD1_CTRL
    }

    fn timeout_to_ticks(&self, timeout_ms: u32) -> Result<u32, WatchdogError> {
        if timeout_ms == 0 {
            return Err(WatchdogError::InvalidTimeout);
        }

        let ticks = u64::from(timeout_ms)
            .checked_mul(self.clk_rate_hz)
            .ok_or(WatchdogError::InvalidTimeout)?
            / 1000;
        if ticks == 0 || ticks > u64::from(u32::MAX) {
            return Err(WatchdogError::InvalidTimeout);
        }

        Ok(ticks as u32)
    }

    fn ticks_to_timeout_ms(&self, ticks: u32) -> u32 {
        ((u64::from(ticks) * 1000) / self.clk_rate_hz) as u32
    }

    fn disable(&self) {
        // SAFETY: `base_addr` comes from an MMIO mapping of the watchdog register block.
        // Writing 0 to WD1 CTRL is the documented disable sequence for Apple Silicon.
        unsafe {
            mmio::write32(self.ctrl_addr(), 0);
        }

        // SAFETY: Same mapped register block as above. Readback confirms the posted write.
        let ctrl = unsafe { mmio::read32(self.ctrl_addr()) };
        early_println!("[apple-wdt] WD1 control after disable = {:#x}", ctrl);
    }
}

impl Watchdog for AppleWdt {
    fn name(&self) -> &'static str {
        "apple-wdt"
    }

    fn start(&self, timeout_ms: u32) -> Result<(), WatchdogError> {
        if self.is_running() {
            return Err(WatchdogError::AlreadyRunning);
        }

        if timeout_ms != 0 {
            self.set_timeout(timeout_ms)?;
        } else if self.get_timeout().is_none() {
            self.set_timeout(APPLE_WDT_DEFAULT_TIMEOUT_MS)?;
        }

        self.ping()?;
        // SAFETY: `base_addr` is the live MMIO mapping retained by this registered watchdog.
        unsafe {
            mmio::write32(self.ctrl_addr(), APPLE_WDT_CTRL_RESET_EN);
        }
        Ok(())
    }

    fn stop(&self) -> Result<(), WatchdogError> {
        self.disable();
        Ok(())
    }

    fn ping(&self) -> Result<(), WatchdogError> {
        // SAFETY: `base_addr` is the live MMIO mapping retained by this registered watchdog.
        unsafe {
            mmio::write32(self.cur_time_addr(), 0);
        }
        Ok(())
    }

    fn is_running(&self) -> bool {
        // SAFETY: `base_addr` is the live MMIO mapping retained by this registered watchdog.
        let ctrl = unsafe { mmio::read32(self.ctrl_addr()) };
        (ctrl & APPLE_WDT_CTRL_RESET_EN) != 0
    }

    fn set_timeout(&self, timeout_ms: u32) -> Result<u32, WatchdogError> {
        let ticks = self.timeout_to_ticks(timeout_ms)?;
        // SAFETY: `base_addr` is the live MMIO mapping retained by this registered watchdog.
        unsafe {
            mmio::write32(self.bite_time_addr(), ticks);
        }

        let actual_ms = self.ticks_to_timeout_ms(ticks);
        self.timeout_ms.store(actual_ms, Ordering::SeqCst);
        Ok(actual_ms)
    }

    fn get_timeout(&self) -> Option<u32> {
        match self.timeout_ms.load(Ordering::SeqCst) {
            0 => None,
            timeout_ms => Some(timeout_ms),
        }
    }

    fn max_timeout(&self) -> u32 {
        let timeout_ms = (u64::from(u32::MAX) * 1000) / self.clk_rate_hz;
        timeout_ms.min(u64::from(u32::MAX)) as u32
    }
}

fn resolve_watchdog_clk(device: &PlatformDeviceInfo) -> Option<ClkHandle> {
    let manager = DeviceManager::get_manager();
    for name in ["wdt", "ref", "bus"] {
        if let Ok(clk) = manager.resolve_clk(device, name) {
            if let Err(error) = clk.prepare_enable() {
                early_println!(
                    "[apple-wdt] warning: failed to enable {} clock: {:?}",
                    name,
                    error
                );
                return None;
            }
            return Some(clk);
        }
    }

    None
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resource = device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("Apple watchdog: no memory resource found")?;

    let paddr = mem_resource.start;
    let size = mem_resource.end - mem_resource.start + 1;
    early_println!(
        "[apple-wdt] probe {} at paddr={:#x}, size={:#x}",
        device.name(),
        paddr,
        size
    );

    let base_addr = scarlet::vm::ioremap(paddr, size).map_err(|e| {
        early_println!("[apple-wdt] ioremap failed: {}", e);
        e
    })?;

    let clk = resolve_watchdog_clk(device);
    let clk_rate_hz = match clk.as_ref().map(ClkHandle::rate) {
        Some(rate) if rate != 0 => rate,
        _ => APPLE_WDT_DEFAULT_CLK_HZ,
    };
    let watchdog = Arc::new(AppleWdt::new(base_addr, clk_rate_hz, clk));
    watchdog.disable();
    DeviceManager::get_manager().register_watchdog(watchdog);

    early_println!("[apple-wdt] watchdog disabled and registered");
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-wdt",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,wdt",
            "apple,t8103-wdt",
            "apple,t8112-wdt",
            "apple,t6000-wdt",
            "apple,t6020-wdt",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical);
}

driver_initcall!(register_driver);

#[used]
static SCARLET_DRIVER_APPLE_WDT_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
