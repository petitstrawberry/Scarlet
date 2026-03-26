#![no_std]
extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;

use scarlet::sync::Mutex;

use scarlet::device::DeviceInfo;
use scarlet::device::manager::{DeviceManager, DriverPriority};
use scarlet::device::platform::resource::PlatformDeviceResourceType;
use scarlet::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use scarlet::early_println;
use scarlet::time;
use scarlet::vm;

const FIFO_WR_DPTX_CLK_EN: usize = 0x000;
const FIFO_WR_N_CLK_EN: usize = 0x004;
const FIFO_WR_UNK_EN: usize = 0x008;
const FIFO_RD_PCLK1_EN: usize = 0x020;
const FIFO_RD_N_CLK_EN: usize = 0x028;
const FIFO_RD_UNK_EN: usize = 0x02c;

const OUT_PCLK1_EN: usize = 0x040;
const OUT_N_CLK_EN: usize = 0x048;
const OUT_UNK_EN: usize = 0x04c;

const CROSSBAR_DISPEXT_EN: usize = 0x050;
const CROSSBAR_MUX_CTRL: usize = 0x060;
const CROSSBAR_ATC_EN: usize = 0x070;

const FIFO_WR_DPTX_CLK_EN_STAT: usize = 0x800;
const FIFO_WR_N_CLK_EN_STAT: usize = 0x804;
const FIFO_RD_PCLK1_EN_STAT: usize = 0x820;
const FIFO_RD_PCLK2_EN_STAT: usize = 0x824;
const FIFO_RD_N_CLK_EN_STAT: usize = 0x828;
const OUT_PCLK1_EN_STAT: usize = 0x840;
const OUT_PCLK2_EN_STAT: usize = 0x844;
const OUT_N_CLK_EN_STAT: usize = 0x848;

const UNK_TUNABLE: usize = 0xc00;

const ATC_DPIN0: u32 = 1 << 0;
const ATC_DPIN1: u32 = 1 << 4;
const ATC_DPPHY: u32 = 1 << 8;

const MUX_DPPHY: usize = 0;
const MUX_DPIN0: usize = 1;
const MUX_DPIN1: usize = 2;
const MUX_MAX: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpxbarPort {
    DpPhy,
    Dpin0,
    Dpin1,
}

pub struct AppleDpxbar {
    base: usize,
    selected_dispext: [i32; MUX_MAX],
    n_ufp: u32,
}

impl AppleDpxbar {
    fn new(base: usize, n_ufp: u32) -> Self {
        Self {
            base,
            selected_dispext: [-1; MUX_MAX],
            n_ufp,
        }
    }

    fn port_index(port: DpxbarPort) -> usize {
        match port {
            DpxbarPort::DpPhy => MUX_DPPHY,
            DpxbarPort::Dpin0 => MUX_DPIN0,
            DpxbarPort::Dpin1 => MUX_DPIN1,
        }
    }

    pub fn set_port(
        &mut self,
        port: DpxbarPort,
        dispext_state: Option<u32>,
    ) -> Result<(), &'static str> {
        let state = match dispext_state {
            Some(value) => {
                if value >= self.n_ufp {
                    return Err("apple-dpxbar: invalid dispext state");
                }
                value as i32
            }
            None => -1,
        };

        self.set(Self::port_index(port), state)
    }

    pub fn disconnect_all(&mut self) {
        for port in [MUX_DPPHY, MUX_DPIN0, MUX_DPIN1] {
            if let Err(error) = self.set(port, -1) {
                early_println!(
                    "[apple-dpxbar] disconnect_all failed on port {}: {}",
                    port,
                    error
                );
            }
        }

        self.log_status();
    }

    fn log_status(&self) {
        early_println!(
            "[apple-dpxbar] stat wr_dptx={:#x} wr_n={:#x} rd_pclk1={:#x} rd_pclk2={:#x} rd_n={:#x} out_pclk1={:#x} out_pclk2={:#x} out_n={:#x}",
            read32(self.base, FIFO_WR_DPTX_CLK_EN_STAT),
            read32(self.base, FIFO_WR_N_CLK_EN_STAT),
            read32(self.base, FIFO_RD_PCLK1_EN_STAT),
            read32(self.base, FIFO_RD_PCLK2_EN_STAT),
            read32(self.base, FIFO_RD_N_CLK_EN_STAT),
            read32(self.base, OUT_PCLK1_EN_STAT),
            read32(self.base, OUT_PCLK2_EN_STAT),
            read32(self.base, OUT_N_CLK_EN_STAT)
        );
    }

    fn set(&mut self, port_index: usize, dispext_state: i32) -> Result<(), &'static str> {
        if port_index >= MUX_MAX {
            return Err("apple-dpxbar: invalid port index");
        }
        if dispext_state >= 0 && (dispext_state as u32) >= self.n_ufp {
            return Err("apple-dpxbar: dispext state out of range");
        }

        let (atc_bit, mux_mask, mux_set) = match port_index {
            MUX_DPPHY => (
                ATC_DPPHY,
                ((0xfu32) << 20) | ((0xfu32) << 8),
                field_prep(20, dispext_state as u32) | field_prep(8, dispext_state as u32),
            ),
            MUX_DPIN0 => (
                ATC_DPIN0,
                ((0xfu32) << 16) | ((0xfu32) << 0),
                field_prep(16, dispext_state as u32) | field_prep(0, dispext_state as u32),
            ),
            MUX_DPIN1 => (
                ATC_DPIN1,
                ((0xfu32) << 12) | ((0xfu32) << 4),
                field_prep(12, dispext_state as u32) | field_prep(4, dispext_state as u32),
            ),
            _ => return Err("apple-dpxbar: unknown port mapping"),
        };

        set32(self.base, OUT_N_CLK_EN, atc_bit);
        clear32(self.base, OUT_UNK_EN, atc_bit);
        clear32(self.base, OUT_PCLK1_EN, atc_bit);
        clear32(self.base, CROSSBAR_ATC_EN, atc_bit);

        let prev_state = self.selected_dispext[port_index];
        if prev_state >= 0 {
            let prev_dispext_bit = 1u32 << (prev_state as u32);
            let prev_dispext_bit_en = 1u32 << (2 * (prev_state as u32));

            set32(self.base, FIFO_WR_N_CLK_EN, prev_dispext_bit);
            set32(self.base, FIFO_RD_N_CLK_EN, prev_dispext_bit);

            clear32(self.base, FIFO_WR_UNK_EN, prev_dispext_bit);
            clear32(self.base, FIFO_RD_UNK_EN, prev_dispext_bit_en);
            clear32(self.base, FIFO_WR_DPTX_CLK_EN, prev_dispext_bit);

            clear32(self.base, FIFO_RD_PCLK1_EN, prev_dispext_bit);
            clear32(self.base, CROSSBAR_DISPEXT_EN, prev_dispext_bit);

            self.selected_dispext[port_index] = -1;
        }

        mask32(self.base, CROSSBAR_MUX_CTRL, mux_mask, mux_set);

        if dispext_state >= 0 {
            let dispext_bit = 1u32 << (dispext_state as u32);
            let dispext_bit_en = 1u32 << (2 * (dispext_state as u32));

            clear32(self.base, FIFO_WR_N_CLK_EN, dispext_bit);
            clear32(self.base, FIFO_RD_N_CLK_EN, dispext_bit);
            clear32(self.base, OUT_N_CLK_EN, atc_bit);

            set32(self.base, FIFO_WR_UNK_EN, dispext_bit);
            set32(self.base, FIFO_RD_UNK_EN, dispext_bit_en);
            set32(self.base, OUT_UNK_EN, atc_bit);

            set32(self.base, FIFO_WR_DPTX_CLK_EN, dispext_bit);
            set32(self.base, FIFO_RD_PCLK1_EN, dispext_bit);
            set32(self.base, OUT_PCLK1_EN, atc_bit);

            set32(self.base, CROSSBAR_ATC_EN, atc_bit);
            set32(self.base, CROSSBAR_DISPEXT_EN, dispext_bit);

            clear32(self.base, FIFO_RD_PCLK1_EN, dispext_bit);
            time::udelay(10);
            set32(self.base, FIFO_RD_PCLK1_EN, dispext_bit);

            self.selected_dispext[port_index] = dispext_state;
        }

        Ok(())
    }
}

fn field_prep(shift: u32, value: u32) -> u32 {
    (value & 0xf) << shift
}

fn read32(base: usize, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base + offset) as *const u32) }
}

fn write32(base: usize, offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile((base + offset) as *mut u32, value) }
}

fn mask32(base: usize, offset: usize, mask: u32, set: u32) {
    let value = read32(base, offset);
    write32(base, offset, (value & !mask) | set);
}

fn set32(base: usize, offset: usize, bits: u32) {
    mask32(base, offset, 0, bits);
}

fn clear32(base: usize, offset: usize, bits: u32) {
    mask32(base, offset, bits, 0);
}

fn dpxbar_resource(device: &PlatformDeviceInfo) -> Result<(usize, usize), &'static str> {
    let mem_resources: Vec<_> = device
        .get_resources()
        .iter()
        .filter(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .collect();

    let region = mem_resources
        .first()
        .ok_or("apple-dpxbar: missing memory resource")?;

    let paddr = region.start;
    let size = region
        .end
        .checked_sub(region.start)
        .and_then(|value| value.checked_add(1))
        .ok_or("apple-dpxbar: invalid memory resource")?;

    Ok((paddr, size))
}

fn is_t8103(device: &PlatformDeviceInfo) -> bool {
    device
        .compatible()
        .iter()
        .any(|entry| *entry == "apple,t8103-display-crossbar")
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let (paddr, size) = dpxbar_resource(device)?;
    let base = vm::ioremap(paddr, size).map_err(|_| "apple-dpxbar: failed to map MMIO")?;

    let mut dpxbar = AppleDpxbar::new(base, 2);

    if is_t8103(device) {
        let tunable = read32(base, UNK_TUNABLE);
        write32(base, UNK_TUNABLE, 0);
        let tunable_after = read32(base, UNK_TUNABLE);
        early_println!(
            "[apple-dpxbar] t8103 tunable: before={:#x} after={:#x}",
            tunable,
            tunable_after
        );
    }

    dpxbar.disconnect_all();

    early_println!(
        "[apple-dpxbar] probe: paddr={:#x} size={:#x} n_ufp={}",
        paddr,
        size,
        dpxbar.n_ufp
    );

    *DPXBAR.lock() = Some(dpxbar);
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    *DPXBAR.lock() = None;
    Ok(())
}

static DPXBAR: Mutex<Option<AppleDpxbar>> = Mutex::new(None);

pub fn with_dpxbar<R>(f: impl FnOnce(&mut AppleDpxbar) -> R) -> Option<R> {
    let mut guard = DPXBAR.lock();
    guard.as_mut().map(f)
}

fn register_dpxbar_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-dpxbar",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-display-crossbar",
            "apple,t8112-display-crossbar",
            "apple,t6000-display-crossbar",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Standard);
}

scarlet::driver_initcall!(register_dpxbar_driver);

#[used]
static SCARLET_DRIVER_DPXBAR_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
