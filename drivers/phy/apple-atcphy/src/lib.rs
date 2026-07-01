#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::{
    arch::mmio,
    device::{
        DeviceInfo,
        manager::{DeviceManager, DriverPriority},
        phy::{Phy, PhyError, PhyHandle, PhyMode, PhyProvider},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
        reset::ResetController,
    },
    early_println,
};

// =============================================================================
// =============================================================================

const ATCPHY_POWER_CTRL: usize = 0x20000;
const ATCPHY_POWER_STAT: usize = 0x20004;
const ATCPHY_MISC: usize = 0x20008;

const ATCPHY_POWER_SLEEP_SMALL: u32 = 1 << 0;
const ATCPHY_POWER_SLEEP_BIG: u32 = 1 << 1;
const ATCPHY_POWER_CLAMP_EN: u32 = 1 << 2;
const ATCPHY_POWER_APB_RESET_N: u32 = 1 << 3;
const ATCPHY_POWER_PHY_RESET_N: u32 = 1 << 4;

const ATCPHY_MISC_RESET_N: u32 = 1 << 0;
const ATCPHY_MISC_LANE_SWAP: u32 = 1 << 2;

const ACIOPHY_CFG0: usize = 0x08;
const ACIOPHY_CFG0_COMMON_BIG_OV: u32 = 1 << 1;
const ACIOPHY_CFG0_COMMON_SMALL_OV: u32 = 1 << 3;
const ACIOPHY_CFG0_COMMON_CLAMP_OV: u32 = 1 << 5;
const ACIOPHY_CFG0_RX_SMALL_OV: u32 = 0x3 << 8;
const ACIOPHY_CFG0_RX_BIG_OV: u32 = 0x3 << 12;
const ACIOPHY_CFG0_RX_CLAMP_OV: u32 = 0x3 << 16;

const ACIOPHY_SLEEP_CTRL: usize = 0x1b0;
const ACIOPHY_SLEEP_CTRL_TX_BIG_OV: u32 = 0x3 << 2;
const ACIOPHY_SLEEP_CTRL_TX_SMALL_OV: u32 = 0x3 << 6;
const ACIOPHY_SLEEP_CTRL_TX_CLAMP_OV: u32 = 0x3 << 10;

const AUSPLL_FSM_CTRL: usize = 0x1014;
const AUSPLL_APB_CMD_OVERRIDE: usize = 0x2000;
const AUSPLL_APB_CMD_OVERRIDE_UNK28: u32 = 1 << 28;

const CIO3PLL_CLK_CTRL: usize = 0x2a00;
const CIO3PLL_CLK_PCLK_EN: u32 = 1 << 1;
const CIO3PLL_CLK_REFCLK_EN: u32 = 1 << 5;

const ACIOPHY_LANE_MODE: usize = 0x48;
const ACIOPHY_CROSSBAR: usize = 0x4c;
const ACIOPHY_CROSSBAR_PROTOCOL_MASK: u32 = 0x1f;
const ACIOPHY_CROSSBAR_PROTOCOL_USB3_DP: u32 = 0x10;

const ACIOPHY_LANE_MODE_USB3: u32 = 0x3;
const ACIOPHY_LANE_MODE_DP: u32 = 0x5;

const PHY_TYPE_USB2: u32 = 3;
const PHY_TYPE_USB3: u32 = 4;

// =============================================================================
// =============================================================================

const USB2PHY_USBCTL: usize = 0x00;
const USB2PHY_CTL: usize = 0x04;
const USB2PHY_SIG: usize = 0x08;
const USB2PHY_MISCTUNE: usize = 0x1c;

const USB2PHY_USBCTL_RUN: u32 = 1 << 1;
const USB2PHY_USBCTL_ISOLATION: u32 = 1 << 2;

const USB2PHY_CTL_RESET: u32 = 1 << 0;
const USB2PHY_CTL_PORT_RESET: u32 = 1 << 1;
const USB2PHY_CTL_APB_RESET_N: u32 = 1 << 2;
const USB2PHY_CTL_SIDDQ: u32 = 1 << 3;

const USB2PHY_SIG_VBUSDET_FORCE_VAL: u32 = 1 << 0;
const USB2PHY_SIG_VBUSDET_FORCE_EN: u32 = 1 << 1;
const USB2PHY_SIG_VBUSVLDEXT_FORCE_VAL: u32 = 1 << 2;
const USB2PHY_SIG_VBUSVLDEXT_FORCE_EN: u32 = 1 << 3;
const USB2PHY_SIG_HOST: u32 = 7 << 12;

const USB2PHY_MISCTUNE_APBCLK_GATE_OFF: u32 = 1 << 29;
const USB2PHY_MISCTUNE_REFCLK_GATE_OFF: u32 = 1 << 30;

// =============================================================================
// =============================================================================

const PIPEHANDLER_OVERRIDE: usize = 0x00;
const PIPEHANDLER_OVERRIDE_VALUES: usize = 0x04;
const PIPEHANDLER_MUX_CTRL: usize = 0x0c;
const PIPEHANDLER_LOCK_REQ: usize = 0x10;
const PIPEHANDLER_LOCK_ACK: usize = 0x14;
const PIPEHANDLER_NONSELECTED_OVERRIDE: usize = 0x20;

const PIPEHANDLER_OVERRIDE_RXVALID: u32 = 1 << 0;
const PIPEHANDLER_OVERRIDE_RXDETECT: u32 = 1 << 2;

const PIPEHANDLER_OVERRIDE_VAL_RXDETECT0: u32 = 1 << 1;
const PIPEHANDLER_OVERRIDE_VAL_RXDETECT1: u32 = 1 << 2;

const PIPEHANDLER_MUX_CTRL_DATA_MASK: u32 = 0x7;
const PIPEHANDLER_MUX_CTRL_CLK_MASK: u32 = 0x7 << 3;
const PIPEHANDLER_MUX_CTRL_CLK_OFF: u32 = 0;
const PIPEHANDLER_MUX_CTRL_CLK_USB3: u32 = 1;
const PIPEHANDLER_MUX_CTRL_CLK_DUMMY: u32 = 4;
const PIPEHANDLER_MUX_CTRL_DATA_USB3: u32 = 0;
const PIPEHANDLER_MUX_CTRL_DATA_DUMMY: u32 = 2;

const PIPEHANDLER_LOCK_EN: u32 = 1 << 0;

const PIPEHANDLER_AON_GEN: usize = 0x1c;
const PIPEHANDLER_AON_GEN_DWC3_FORCE_CLAMP_EN: u32 = 1 << 4;
const PIPEHANDLER_AON_GEN_DWC3_RESET_N: u32 = 1 << 0;

const PIPEHANDLER_NATIVE_RESET: u32 = 1 << 12;
const PIPEHANDLER_DUMMY_PHY_EN: u32 = 1 << 15;
const PIPEHANDLER_NATIVE_POWER_DOWN_MASK: u32 = 0xf;

const PIPEHANDLER_MUX_CTRL_DATA_DP: u32 = 4;
const PIPEHANDLER_MUX_CTRL_CLK_DP: u32 = 4;

// =============================================================================
// Hardware Tunable
// =============================================================================

/// One hardware tunable entry: `[offset, mask, value]` applied to an MMIO region.
///
/// The bootloader (m1n1) pre-processes EFUSE calibration data into these
/// register-level tunables and injects them into the device tree.
#[derive(Debug, Clone)]
pub struct HardwareTunable {
    /// Register offset from the target MMIO base.
    pub offset: u32,
    /// Bit mask selecting the register fields controlled by this tunable.
    pub mask: u32,
    /// Value to OR into the masked register fields.
    pub value: u32,
}

impl HardwareTunable {
    /// Parse a tunable array from device tree property bytes.
    ///
    /// Property contains big-endian u32 triplets: `[offset, mask, value, ...]`.
    pub fn parse_from_property(prop_bytes: &[u8]) -> Vec<Self> {
        let mut tunables = Vec::new();
        let chunks = prop_bytes.chunks_exact(12);
        for chunk in chunks {
            let offset = u32::from_be_bytes(chunk[0..4].try_into().unwrap_or([0; 4]));
            let mask = u32::from_be_bytes(chunk[4..8].try_into().unwrap_or([0; 4]));
            let value = u32::from_be_bytes(chunk[8..12].try_into().unwrap_or([0; 4]));
            tunables.push(Self {
                offset,
                mask,
                value,
            });
        }
        tunables
    }

    /// Apply this tunable to a 32-bit register read from `base + offset`.
    pub fn apply(&self, base: usize) {
        let old = unsafe { mmio::read32(base + self.offset as usize) };
        let new = (old & !self.mask) | self.value;
        if new != old {
            unsafe { mmio::write32(base + self.offset as usize, new) };
        }
    }
}

/// Apply a slice of tunables to an MMIO base.
///
/// # Arguments
///
/// * `tunables` - Tunable entries to apply in order.
/// * `base` - Virtual MMIO base address for the target register block.
pub fn apply_tunables(tunables: &[HardwareTunable], base: usize) {
    for t in tunables {
        t.apply(base);
    }
}

/// Parse an `apple,tunable-*` property from the device info.
fn parse_tunable_prop(device: &PlatformDeviceInfo, name: &str) -> Vec<HardwareTunable> {
    device
        .property(name)
        .map(|p| HardwareTunable::parse_from_property(p.value()))
        .unwrap_or_default()
}

// =============================================================================
// ATC PHY Mode
// =============================================================================

/// Supported Apple ATC PHY protocol modes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AtcPhyMode {
    /// USB 3.x SuperSpeed mode.
    Usb3,
    /// DisplayPort-only mode.
    DisplayPort,
    /// Combined USB 3.x and DisplayPort mode.
    Usb3Dp,
}

// =============================================================================
// ATC PHY Instance
// =============================================================================

/// Apple ATC PHY hardware instance.
///
/// The instance owns mapped MMIO bases and bootloader-provided tunable tables
/// used to initialize USB3 and DisplayPort lanes.
pub struct AppleAtcPhy {
    core_base: usize,
    lpdptx_base: Option<usize>,
    axi2af_base: Option<usize>,
    usb2phy_base: usize,
    pipehandler_base: usize,
    common_a: Vec<HardwareTunable>,
    common_b: Vec<HardwareTunable>,
    axi2af_tunables: Vec<HardwareTunable>,
    lane0_usb: Vec<HardwareTunable>,
    lane1_usb: Vec<HardwareTunable>,
    lane0_dp: Vec<HardwareTunable>,
    lane1_dp: Vec<HardwareTunable>,
    pipehandler_up: bool,
}

impl AppleAtcPhy {
    /// Create a new Apple ATC PHY instance from mapped MMIO regions.
    ///
    /// # Arguments
    ///
    /// * `core_base` - Virtual base for the ATC PHY core register block.
    /// * `lpdptx_base` - Optional virtual base for the LPDP TX register block.
    /// * `axi2af_base` - Optional virtual base for the AXI2AF register block.
    /// * `usb2phy_base` - Virtual base for the USB2 PHY register block.
    /// * `pipehandler_base` - Virtual base for the pipehandler register block.
    ///
    /// # Returns
    ///
    /// An uninitialized PHY instance with empty tunable tables.
    pub fn new(
        core_base: usize,
        lpdptx_base: Option<usize>,
        axi2af_base: Option<usize>,
        usb2phy_base: usize,
        pipehandler_base: usize,
    ) -> Self {
        Self {
            core_base,
            lpdptx_base,
            axi2af_base,
            usb2phy_base,
            pipehandler_base,
            common_a: Vec::new(),
            common_b: Vec::new(),
            axi2af_tunables: Vec::new(),
            lane0_usb: Vec::new(),
            lane1_usb: Vec::new(),
            lane0_dp: Vec::new(),
            lane1_dp: Vec::new(),
            pipehandler_up: false,
        }
    }

    fn small_delay(&self) {
        for _ in 0..1000 {
            core::hint::spin_loop();
        }
    }

    fn core_read32(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.core_base + offset) }
    }

    fn core_write32(&self, offset: usize, val: u32) {
        unsafe { mmio::write32(self.core_base + offset, val) }
    }

    fn core_set32(&self, offset: usize, bits: u32) {
        self.core_write32(offset, self.core_read32(offset) | bits);
    }

    fn core_clear32(&self, offset: usize, bits: u32) {
        self.core_write32(offset, self.core_read32(offset) & !bits);
    }

    fn core_mask32(&self, offset: usize, mask: u32, set: u32) {
        let old = self.core_read32(offset);
        self.core_write32(offset, (old & !mask) | set);
    }

    fn usb2phy_read32(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.usb2phy_base + offset) }
    }

    fn usb2phy_write32(&self, offset: usize, val: u32) {
        unsafe { mmio::write32(self.usb2phy_base + offset, val) }
    }

    fn usb2phy_set32(&self, offset: usize, bits: u32) {
        self.usb2phy_write32(offset, self.usb2phy_read32(offset) | bits);
    }

    fn usb2phy_clear32(&self, offset: usize, bits: u32) {
        self.usb2phy_write32(offset, self.usb2phy_read32(offset) & !bits);
    }

    fn ph_read32(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.pipehandler_base + offset) }
    }

    fn ph_write32(&self, offset: usize, val: u32) {
        unsafe { mmio::write32(self.pipehandler_base + offset, val) }
    }

    fn ph_set32(&self, offset: usize, bits: u32) {
        self.ph_write32(offset, self.ph_read32(offset) | bits);
    }

    fn ph_clear32(&self, offset: usize, bits: u32) {
        self.ph_write32(offset, self.ph_read32(offset) & !bits);
    }

    fn ph_mask32(&self, offset: usize, mask: u32, set: u32) {
        let old = self.ph_read32(offset);
        self.ph_write32(offset, (old & !mask) | set);
    }

    fn lpdptx_read32(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.lpdptx_base.unwrap() + offset) }
    }

    fn lpdptx_write32(&self, offset: usize, val: u32) {
        unsafe { mmio::write32(self.lpdptx_base.unwrap() + offset, val) }
    }

    fn axi2af_read32(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.axi2af_base.unwrap() + offset) }
    }

    fn axi2af_write32(&self, offset: usize, val: u32) {
        unsafe { mmio::write32(self.axi2af_base.unwrap() + offset, val) }
    }

    fn poll_core(
        &self,
        offset: usize,
        mask: u32,
        domain: &'static str,
    ) -> Result<(), &'static str> {
        let mut timeout = 10000;
        while timeout != 0 {
            if self.core_read32(offset) & mask == mask {
                return Ok(());
            }
            self.small_delay();
            timeout -= 1;
        }
        early_println!("[apple-atcphy] timeout waiting for {} power domain", domain);
        Err("apple-atcphy: core power domain timeout")
    }

    fn usb2_power_on(&self) {
        let sig = USB2PHY_SIG_VBUSDET_FORCE_VAL
            | USB2PHY_SIG_VBUSDET_FORCE_EN
            | USB2PHY_SIG_VBUSVLDEXT_FORCE_VAL
            | USB2PHY_SIG_VBUSVLDEXT_FORCE_EN;
        let host = self.usb2phy_read32(USB2PHY_SIG) & USB2PHY_SIG_HOST;
        self.usb2phy_write32(USB2PHY_SIG, sig | host);
        self.small_delay();

        self.usb2phy_clear32(USB2PHY_CTL, USB2PHY_CTL_SIDDQ);
        self.small_delay();

        self.usb2phy_clear32(USB2PHY_CTL, USB2PHY_CTL_RESET);
        self.small_delay();
        self.usb2phy_clear32(USB2PHY_CTL, USB2PHY_CTL_PORT_RESET);
        self.small_delay();
        self.usb2phy_set32(USB2PHY_CTL, USB2PHY_CTL_APB_RESET_N);
        self.small_delay();

        self.usb2phy_clear32(USB2PHY_MISCTUNE, USB2PHY_MISCTUNE_APBCLK_GATE_OFF);
        self.usb2phy_clear32(USB2PHY_MISCTUNE, USB2PHY_MISCTUNE_REFCLK_GATE_OFF);

        self.usb2phy_write32(USB2PHY_USBCTL, USB2PHY_USBCTL_RUN);
    }

    fn usb2_power_off(&self) {
        self.usb2phy_write32(USB2PHY_USBCTL, USB2PHY_USBCTL_ISOLATION);
        scarlet::time::udelay(10);

        self.usb2phy_set32(USB2PHY_CTL, USB2PHY_CTL_SIDDQ);
        scarlet::time::udelay(10);

        self.usb2phy_set32(USB2PHY_CTL, USB2PHY_CTL_PORT_RESET);
        scarlet::time::udelay(10);
        self.usb2phy_set32(USB2PHY_CTL, USB2PHY_CTL_RESET);
        scarlet::time::udelay(10);
        self.usb2phy_clear32(USB2PHY_CTL, USB2PHY_CTL_APB_RESET_N);
        scarlet::time::udelay(10);

        self.usb2phy_set32(USB2PHY_MISCTUNE, USB2PHY_MISCTUNE_APBCLK_GATE_OFF);
        self.usb2phy_set32(USB2PHY_MISCTUNE, USB2PHY_MISCTUNE_REFCLK_GATE_OFF);
    }

    fn usb2_set_mode(&self, mode: PhyMode) -> Result<(), PhyError> {
        match mode {
            PhyMode::UsbHost | PhyMode::UsbOtg => {
                self.usb2phy_set32(USB2PHY_SIG, USB2PHY_SIG_HOST);
                early_println!("[apple-atcphy] usb2 mode host");
                Ok(())
            }
            PhyMode::UsbDevice => {
                self.usb2phy_clear32(USB2PHY_SIG, USB2PHY_SIG_HOST);
                early_println!("[apple-atcphy] usb2 mode device");
                Ok(())
            }
            _ => Err(PhyError::InvalidMode),
        }
    }

    fn core_power_on(&self) -> Result<(), &'static str> {
        self.core_set32(ATCPHY_MISC, ATCPHY_MISC_RESET_N);

        self.core_set32(ATCPHY_POWER_CTRL, ATCPHY_POWER_SLEEP_SMALL);
        self.poll_core(ATCPHY_POWER_STAT, ATCPHY_POWER_SLEEP_SMALL, "small")?;

        self.core_set32(ATCPHY_POWER_CTRL, ATCPHY_POWER_SLEEP_BIG);
        self.poll_core(ATCPHY_POWER_STAT, ATCPHY_POWER_SLEEP_BIG, "big")?;

        self.core_clear32(ATCPHY_POWER_CTRL, ATCPHY_POWER_CLAMP_EN);
        self.core_set32(ATCPHY_POWER_CTRL, ATCPHY_POWER_APB_RESET_N);

        Ok(())
    }

    fn configure_crossbar(&self) {
        let crossbar = self.core_read32(ACIOPHY_CROSSBAR);
        self.core_write32(
            ACIOPHY_CROSSBAR,
            (crossbar & !ACIOPHY_CROSSBAR_PROTOCOL_MASK) | ACIOPHY_CROSSBAR_PROTOCOL_USB3_DP,
        );

        let lane_mode = (ACIOPHY_LANE_MODE_USB3 << 0)
            | (ACIOPHY_LANE_MODE_USB3 << 3)
            | (ACIOPHY_LANE_MODE_DP << 6)
            | (ACIOPHY_LANE_MODE_DP << 9);
        self.core_write32(ACIOPHY_LANE_MODE, lane_mode);
    }

    fn configure_pipehandler_usb3(&mut self, host: bool) {
        if self.pipehandler_up {
            return;
        }

        self.ph_clear32(
            PIPEHANDLER_OVERRIDE_VALUES,
            PIPEHANDLER_OVERRIDE_VAL_RXDETECT0 | PIPEHANDLER_OVERRIDE_VAL_RXDETECT1,
        );

        if host {
            self.ph_set32(PIPEHANDLER_OVERRIDE, PIPEHANDLER_OVERRIDE_RXVALID);
            self.ph_set32(PIPEHANDLER_OVERRIDE, PIPEHANDLER_OVERRIDE_RXDETECT);
            self.ph_set32(PIPEHANDLER_LOCK_REQ, PIPEHANDLER_LOCK_EN);

            let nonselected = self.ph_read32(PIPEHANDLER_NONSELECTED_OVERRIDE);
            self.ph_write32(
                PIPEHANDLER_NONSELECTED_OVERRIDE,
                (nonselected & !PIPEHANDLER_NATIVE_POWER_DOWN_MASK) | 3,
            );
            self.ph_clear32(PIPEHANDLER_NONSELECTED_OVERRIDE, PIPEHANDLER_NATIVE_RESET);
        }

        let mut mux = self.ph_read32(PIPEHANDLER_MUX_CTRL);
        mux = (mux & !PIPEHANDLER_MUX_CTRL_CLK_MASK) | (PIPEHANDLER_MUX_CTRL_CLK_OFF << 3);
        self.ph_write32(PIPEHANDLER_MUX_CTRL, mux);
        self.small_delay();

        mux = (mux & !PIPEHANDLER_MUX_CTRL_DATA_MASK) | PIPEHANDLER_MUX_CTRL_DATA_USB3;
        self.ph_write32(PIPEHANDLER_MUX_CTRL, mux);
        self.small_delay();

        mux = (mux & !PIPEHANDLER_MUX_CTRL_CLK_MASK) | (PIPEHANDLER_MUX_CTRL_CLK_USB3 << 3);
        self.ph_write32(PIPEHANDLER_MUX_CTRL, mux);
        self.small_delay();

        self.ph_clear32(PIPEHANDLER_OVERRIDE, PIPEHANDLER_OVERRIDE_RXVALID);
        self.ph_clear32(PIPEHANDLER_OVERRIDE, PIPEHANDLER_OVERRIDE_RXDETECT);

        if host {
            self.ph_clear32(PIPEHANDLER_LOCK_REQ, PIPEHANDLER_LOCK_EN);
        }
        self.pipehandler_up = true;
    }

    fn configure_pipehandler_dummy(&mut self) {
        self.ph_clear32(
            PIPEHANDLER_OVERRIDE_VALUES,
            PIPEHANDLER_OVERRIDE_VAL_RXDETECT0 | PIPEHANDLER_OVERRIDE_VAL_RXDETECT1,
        );
        self.ph_set32(PIPEHANDLER_OVERRIDE, PIPEHANDLER_OVERRIDE_RXVALID);
        self.ph_set32(PIPEHANDLER_OVERRIDE, PIPEHANDLER_OVERRIDE_RXDETECT);
        self.ph_set32(PIPEHANDLER_LOCK_REQ, PIPEHANDLER_LOCK_EN);

        self.ph_mask32(
            PIPEHANDLER_MUX_CTRL,
            PIPEHANDLER_MUX_CTRL_CLK_MASK,
            PIPEHANDLER_MUX_CTRL_CLK_OFF << 3,
        );
        scarlet::time::udelay(10);
        self.ph_mask32(
            PIPEHANDLER_MUX_CTRL,
            PIPEHANDLER_MUX_CTRL_DATA_MASK,
            PIPEHANDLER_MUX_CTRL_DATA_DUMMY,
        );
        scarlet::time::udelay(10);
        self.ph_mask32(
            PIPEHANDLER_MUX_CTRL,
            PIPEHANDLER_MUX_CTRL_CLK_MASK,
            PIPEHANDLER_MUX_CTRL_CLK_DUMMY << 3,
        );
        scarlet::time::udelay(10);

        self.ph_clear32(PIPEHANDLER_LOCK_REQ, PIPEHANDLER_LOCK_EN);
        self.ph_mask32(
            PIPEHANDLER_NONSELECTED_OVERRIDE,
            PIPEHANDLER_NATIVE_POWER_DOWN_MASK,
            2,
        );
        self.ph_set32(PIPEHANDLER_NONSELECTED_OVERRIDE, PIPEHANDLER_NATIVE_RESET);
        self.pipehandler_up = false;
    }

    fn dwc3_reset_assert(&mut self) {
        early_println!("[apple-atcphy] dwc3 reset assert");
        self.ph_clear32(PIPEHANDLER_AON_GEN, PIPEHANDLER_AON_GEN_DWC3_RESET_N);
        self.ph_set32(PIPEHANDLER_AON_GEN, PIPEHANDLER_AON_GEN_DWC3_FORCE_CLAMP_EN);

        if self.pipehandler_up {
            self.configure_pipehandler_dummy();
        }
        self.usb2_power_off();
    }

    fn dwc3_reset_deassert(&mut self) {
        early_println!("[apple-atcphy] dwc3 reset deassert");
        self.ph_clear32(PIPEHANDLER_AON_GEN, PIPEHANDLER_AON_GEN_DWC3_FORCE_CLAMP_EN);
        self.ph_set32(PIPEHANDLER_AON_GEN, PIPEHANDLER_AON_GEN_DWC3_RESET_N);
    }

    fn configure_pipehandler_dp(&self, swap_lanes: bool) {
        let (lane0, _lane1) = if swap_lanes { (1, 0) } else { (0, 1) };

        let nonselected = self.ph_read32(PIPEHANDLER_NONSELECTED_OVERRIDE);
        self.ph_write32(
            PIPEHANDLER_NONSELECTED_OVERRIDE,
            (nonselected & !PIPEHANDLER_NATIVE_POWER_DOWN_MASK) | 3,
        );
        self.ph_clear32(PIPEHANDLER_NONSELECTED_OVERRIDE, PIPEHANDLER_NATIVE_RESET);

        // Configure the DP lane
        let _dp_lane = if lane0 == 0 { 0 } else { 1 };
        let mut mux = self.ph_read32(PIPEHANDLER_MUX_CTRL);

        mux = (mux & !PIPEHANDLER_MUX_CTRL_CLK_MASK) | (PIPEHANDLER_MUX_CTRL_CLK_OFF << 3);
        self.ph_write32(PIPEHANDLER_MUX_CTRL, mux);
        self.small_delay();

        mux = (mux & !PIPEHANDLER_MUX_CTRL_DATA_MASK) | PIPEHANDLER_MUX_CTRL_DATA_DP;
        self.ph_write32(PIPEHANDLER_MUX_CTRL, mux);
        self.small_delay();

        mux = (mux & !PIPEHANDLER_MUX_CTRL_CLK_MASK) | (PIPEHANDLER_MUX_CTRL_CLK_DP << 3);
        self.ph_write32(PIPEHANDLER_MUX_CTRL, mux);
        self.small_delay();
    }

    fn apply_mode_tunables(&self, mode: AtcPhyMode, swap_lanes: bool) {
        let (lane0_idx, lane1_idx) = if swap_lanes { (1, 0) } else { (0, 1) };

        apply_tunables(&self.common_a, self.core_base);

        if let Some(axi2af_base) = self.axi2af_base {
            apply_tunables(&self.axi2af_tunables, axi2af_base);
        }

        apply_tunables(&self.common_b, self.core_base);

        match mode {
            AtcPhyMode::Usb3 => {
                apply_tunables(&self.lane0_usb, self.core_base);
                apply_tunables(&self.lane1_usb, self.core_base);
            }
            AtcPhyMode::DisplayPort => {
                apply_tunables(
                    if lane0_idx == 0 {
                        &self.lane0_dp
                    } else {
                        &self.lane1_dp
                    },
                    self.core_base,
                );
                apply_tunables(
                    if lane1_idx == 0 {
                        &self.lane0_dp
                    } else {
                        &self.lane1_dp
                    },
                    self.core_base,
                );
            }
            AtcPhyMode::Usb3Dp => {
                apply_tunables(
                    if lane0_idx == 0 {
                        &self.lane0_usb
                    } else {
                        &self.lane1_usb
                    },
                    self.core_base,
                );
                apply_tunables(
                    if lane1_idx == 0 {
                        &self.lane0_dp
                    } else {
                        &self.lane1_dp
                    },
                    self.core_base,
                );
            }
        }
    }

    /// Initialize the PHY in USB3 mode.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the PHY is powered and configured for USB3 operation.
    pub fn init(&mut self) -> Result<(), &'static str> {
        early_println!("[apple-atcphy] initializing...");

        self.usb2_power_on();
        self.core_power_on()?;
        self.apply_mode_tunables(AtcPhyMode::Usb3, false);

        self.core_write32(AUSPLL_FSM_CTRL, 0x1fe000);
        self.core_write32(AUSPLL_APB_CMD_OVERRIDE, AUSPLL_APB_CMD_OVERRIDE_UNK28);

        self.core_set32(ACIOPHY_CFG0, ACIOPHY_CFG0_COMMON_SMALL_OV);
        scarlet::time::udelay(10);
        self.core_set32(ACIOPHY_CFG0, ACIOPHY_CFG0_COMMON_BIG_OV);
        scarlet::time::udelay(10);
        self.core_set32(ACIOPHY_CFG0, ACIOPHY_CFG0_COMMON_CLAMP_OV);
        scarlet::time::udelay(10);

        self.core_mask32(ACIOPHY_SLEEP_CTRL, ACIOPHY_SLEEP_CTRL_TX_SMALL_OV, 3 << 6);
        scarlet::time::udelay(10);
        self.core_mask32(ACIOPHY_SLEEP_CTRL, ACIOPHY_SLEEP_CTRL_TX_BIG_OV, 3 << 2);
        scarlet::time::udelay(10);
        self.core_mask32(ACIOPHY_SLEEP_CTRL, ACIOPHY_SLEEP_CTRL_TX_CLAMP_OV, 3 << 10);
        scarlet::time::udelay(10);

        self.core_mask32(ACIOPHY_CFG0, ACIOPHY_CFG0_RX_BIG_OV, 3 << 12);
        scarlet::time::udelay(10);
        self.core_mask32(ACIOPHY_CFG0, ACIOPHY_CFG0_RX_SMALL_OV, 3 << 8);
        scarlet::time::udelay(10);
        self.core_mask32(ACIOPHY_CFG0, ACIOPHY_CFG0_RX_CLAMP_OV, 3 << 16);
        scarlet::time::udelay(10);

        self.configure_crossbar();

        self.core_set32(CIO3PLL_CLK_CTRL, CIO3PLL_CLK_PCLK_EN);
        self.core_set32(CIO3PLL_CLK_CTRL, CIO3PLL_CLK_REFCLK_EN);

        self.core_set32(ATCPHY_POWER_CTRL, ATCPHY_POWER_PHY_RESET_N);

        early_println!("[apple-atcphy] initialized (USB3 PHY)");
        Ok(())
    }

    /// Initialize the PHY in a DisplayPort-capable mode.
    ///
    /// # Arguments
    ///
    /// * `mode` - DisplayPort-related ATC PHY mode to apply.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the PHY is powered and configured for the requested mode.
    pub fn init_dp(&mut self, mode: AtcPhyMode) -> Result<(), &'static str> {
        if self.lpdptx_base.is_none() || self.axi2af_base.is_none() {
            return Err("apple-atcphy: lpdptx/axi2af regions not mapped, cannot init DP");
        }

        early_println!("[apple-atcphy] initializing in DP mode ({:?})...", mode);

        self.usb2_power_on();
        self.core_power_on()?;
        self.configure_crossbar();
        self.apply_mode_tunables(mode, false);

        match mode {
            AtcPhyMode::Usb3Dp => {
                self.configure_pipehandler_usb3(true);
                self.configure_pipehandler_dp(false);
            }
            AtcPhyMode::DisplayPort => {
                self.configure_pipehandler_dp(false);
            }
            _ => {
                self.configure_pipehandler_usb3(true);
            }
        }

        early_println!("[apple-atcphy] initialized ({:?} mode)", mode);
        Ok(())
    }
}

struct AppleAtcPhyProvider {
    phy: Arc<Mutex<AppleAtcPhy>>,
    lanes: Vec<Arc<AppleAtcPhyLane>>,
}

impl AppleAtcPhyProvider {
    fn new(phy: Arc<Mutex<AppleAtcPhy>>) -> Self {
        Self {
            phy: Arc::clone(&phy),
            lanes: alloc::vec![
                Arc::new(AppleAtcPhyLane::new(Arc::clone(&phy), PHY_TYPE_USB2)),
                Arc::new(AppleAtcPhyLane::new(phy, PHY_TYPE_USB3)),
            ],
        }
    }
}

impl PhyProvider for AppleAtcPhyProvider {
    fn name(&self) -> &'static str {
        "apple-atcphy"
    }

    fn phy_cells(&self) -> usize {
        1
    }

    fn get_phy(&self, spec: &[u32]) -> Result<PhyHandle, PhyError> {
        if spec.len() != self.phy_cells() {
            return Err(PhyError::NotFound);
        }

        let lane = self
            .lanes
            .iter()
            .find(|lane| lane.phy_type() == spec[0])
            .ok_or(PhyError::NotFound)?;
        Ok(PhyHandle::new(lane.clone()))
    }
}

impl ResetController for AppleAtcPhyProvider {
    fn name(&self) -> &'static str {
        "apple-atcphy-reset"
    }

    fn reset_cells(&self) -> usize {
        0
    }

    fn assert_reset(&self, spec: &[u32]) -> Result<(), &'static str> {
        if !spec.is_empty() {
            return Err("apple-atcphy: invalid reset specifier");
        }
        self.phy.lock().dwc3_reset_assert();
        Ok(())
    }

    fn deassert_reset(&self, spec: &[u32]) -> Result<(), &'static str> {
        if !spec.is_empty() {
            return Err("apple-atcphy: invalid reset specifier");
        }
        self.phy.lock().dwc3_reset_deassert();
        Ok(())
    }
}

struct AppleAtcPhyLane {
    phy: Arc<Mutex<AppleAtcPhy>>,
    lane: u32,
    mode: Mutex<Option<PhyMode>>,
}

impl AppleAtcPhyLane {
    fn new(phy: Arc<Mutex<AppleAtcPhy>>, phy_type: u32) -> Self {
        Self {
            phy,
            lane: phy_type,
            mode: Mutex::new(None),
        }
    }

    fn phy_type(&self) -> u32 {
        self.lane
    }

    fn atc_mode(&self) -> Result<AtcPhyMode, PhyError> {
        match *self.mode.lock() {
            Some(PhyMode::UsbHost | PhyMode::UsbDevice | PhyMode::UsbOtg) | None => {
                Ok(AtcPhyMode::Usb3)
            }
            Some(PhyMode::DisplayPort) => Ok(AtcPhyMode::DisplayPort),
            Some(PhyMode::Other(0)) => Ok(AtcPhyMode::Usb3),
            Some(PhyMode::Other(1)) => Ok(AtcPhyMode::Usb3Dp),
            Some(_) => Err(PhyError::InvalidMode),
        }
    }

    fn power_on_current_mode(&self) -> Result<(), PhyError> {
        let mode = self.atc_mode()?;
        let mut phy = self.phy.lock();
        match (self.lane, mode) {
            (PHY_TYPE_USB2, AtcPhyMode::Usb3) => {
                let phy_mode = (*self.mode.lock()).unwrap_or(PhyMode::UsbHost);
                phy.usb2_set_mode(phy_mode)
            }
            (PHY_TYPE_USB3, AtcPhyMode::Usb3) => {
                phy.init().map_err(|_| PhyError::PowerOnFailed)?;
                Ok(())
            }
            (_, AtcPhyMode::DisplayPort | AtcPhyMode::Usb3Dp) => {
                phy.init_dp(mode).map_err(|_| PhyError::PowerOnFailed)
            }
            _ => Err(PhyError::PowerOnFailed),
        }
    }
}

impl Phy for AppleAtcPhyLane {
    fn name(&self) -> &'static str {
        match self.lane {
            PHY_TYPE_USB2 => "apple-atcphy-usb2",
            PHY_TYPE_USB3 => "apple-atcphy-usb3",
            _ => "apple-atcphy-lane",
        }
    }

    fn power_on(&self) -> Result<(), PhyError> {
        self.power_on_current_mode()
    }

    fn power_off(&self) -> Result<(), PhyError> {
        Ok(())
    }

    fn reset(&self) -> Result<(), PhyError> {
        self.power_on_current_mode().map_err(|error| match error {
            PhyError::PowerOnFailed => PhyError::ResetFailed,
            other => other,
        })
    }

    fn set_mode(&self, mode: PhyMode) -> Result<(), PhyError> {
        match mode {
            PhyMode::UsbHost
            | PhyMode::UsbDevice
            | PhyMode::UsbOtg
            | PhyMode::DisplayPort
            | PhyMode::Other(0)
            | PhyMode::Other(1) => {
                *self.mode.lock() = Some(mode);
                if self.lane == PHY_TYPE_USB2 {
                    self.phy.lock().usb2_set_mode(mode)?;
                } else if self.lane == PHY_TYPE_USB3 {
                    match mode {
                        PhyMode::UsbHost | PhyMode::UsbOtg | PhyMode::Other(0) => {
                            self.phy.lock().configure_pipehandler_usb3(true);
                        }
                        PhyMode::UsbDevice => {
                            self.phy.lock().configure_pipehandler_usb3(false);
                        }
                        _ => {}
                    }
                }
                Ok(())
            }
            _ => Err(PhyError::InvalidMode),
        }
    }

    fn get_mode(&self) -> Option<PhyMode> {
        *self.mode.lock()
    }
}

// =============================================================================
// Global Registry
// =============================================================================

struct AtcPhyEntry {
    instance: Arc<Mutex<AppleAtcPhy>>,
    phandle: u32,
}

static ATC_PHY_REGISTRY: Mutex<alloc::vec::Vec<AtcPhyEntry>> = Mutex::new(alloc::vec::Vec::new());

/// Register an ATC PHY instance in the legacy local registry.
///
/// # Arguments
///
/// * `phy` - ATC PHY instance to store.
/// * `phandle` - Firmware phandle associated with the PHY node.
///
/// # Returns
///
/// Numeric local registry ID assigned to the instance.
pub fn register_atcphy(phy: AppleAtcPhy, phandle: u32) -> u32 {
    register_atcphy_shared(phy, phandle).0
}

fn register_atcphy_shared(phy: AppleAtcPhy, phandle: u32) -> (u32, Arc<Mutex<AppleAtcPhy>>) {
    let mut guard = ATC_PHY_REGISTRY.lock();
    let id = guard.len() as u32;
    let instance = Arc::new(Mutex::new(phy));
    guard.push(AtcPhyEntry {
        instance: Arc::clone(&instance),
        phandle,
    });
    (id, instance)
}

/// Look up a registered ATC PHY instance by local registry ID.
///
/// # Arguments
///
/// * `id` - Local registry ID returned by [`register_atcphy`].
///
/// # Returns
///
/// Shared ATC PHY instance, or `None` when `id` is unknown.
pub fn get_atcphy(id: u32) -> Option<Arc<Mutex<AppleAtcPhy>>> {
    let guard = ATC_PHY_REGISTRY.lock();
    guard.get(id as usize).map(|e| Arc::clone(&e.instance))
}

/// Look up a registered ATC PHY instance by firmware phandle.
///
/// # Arguments
///
/// * `phandle` - Firmware phandle used when the PHY was registered.
///
/// # Returns
///
/// Shared ATC PHY instance, or `None` when no matching registration exists.
pub fn get_atcphy_by_phandle(phandle: u32) -> Option<Arc<Mutex<AppleAtcPhy>>> {
    let guard = ATC_PHY_REGISTRY.lock();
    guard
        .iter()
        .find(|e| e.phandle == phandle)
        .map(|e| Arc::clone(&e.instance))
}

// =============================================================================
// Platform Driver
// =============================================================================

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resources: Vec<_> = device
        .get_resources()
        .iter()
        .filter(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .collect();

    if mem_resources.len() < 5 {
        return Err("apple-atcphy: expected at least 5 memory resources");
    }

    let core_paddr = mem_resources[0].start;
    let core_size = mem_resources[0].end - mem_resources[0].start + 1;

    let lpdptx_paddr = mem_resources[1].start;
    let lpdptx_size = mem_resources[1].end - mem_resources[1].start + 1;

    let axi2af_paddr = mem_resources[2].start;
    let axi2af_size = mem_resources[2].end - mem_resources[2].start + 1;

    let usb2phy_paddr = mem_resources[3].start;
    let usb2phy_size = mem_resources[3].end - mem_resources[3].start + 1;

    let pipehandler_paddr = mem_resources[4].start;
    let pipehandler_size = mem_resources[4].end - mem_resources[4].start + 1;

    early_println!(
        "[apple-atcphy] probing {} core={:#x} lpdptx={:#x} axi2af={:#x} usb2phy={:#x} ph={:#x}",
        device.name(),
        core_paddr,
        lpdptx_paddr,
        axi2af_paddr,
        usb2phy_paddr,
        pipehandler_paddr
    );

    let core_base = scarlet::vm::ioremap(core_paddr, core_size)
        .map_err(|_| "apple-atcphy: ioremap core failed")?;
    let lpdptx_base = scarlet::vm::ioremap(lpdptx_paddr, lpdptx_size).ok();
    let axi2af_base = scarlet::vm::ioremap(axi2af_paddr, axi2af_size).ok();
    let usb2phy_base = scarlet::vm::ioremap(usb2phy_paddr, usb2phy_size)
        .map_err(|_| "apple-atcphy: ioremap usb2phy failed")?;
    let pipehandler_base = scarlet::vm::ioremap(pipehandler_paddr, pipehandler_size)
        .map_err(|_| "apple-atcphy: ioremap pipehandler failed")?;

    let mut phy = AppleAtcPhy::new(
        core_base,
        lpdptx_base,
        axi2af_base,
        usb2phy_base,
        pipehandler_base,
    );

    phy.common_a = parse_tunable_prop(device, "apple,tunable-common-a");
    phy.common_b = parse_tunable_prop(device, "apple,tunable-common-b");
    phy.axi2af_tunables = parse_tunable_prop(device, "apple,tunable-axi2af");
    phy.lane0_usb = parse_tunable_prop(device, "apple,tunable-lane0-usb");
    phy.lane1_usb = parse_tunable_prop(device, "apple,tunable-lane1-usb");
    phy.lane0_dp = parse_tunable_prop(device, "apple,tunable-lane0-dp");
    phy.lane1_dp = parse_tunable_prop(device, "apple,tunable-lane1-dp");

    let tunable_count = phy.common_a.len()
        + phy.common_b.len()
        + phy.axi2af_tunables.len()
        + phy.lane0_usb.len()
        + phy.lane1_usb.len()
        + phy.lane0_dp.len()
        + phy.lane1_dp.len();
    if tunable_count > 0 {
        early_println!(
            "[apple-atcphy] loaded {} tunables (common={}/{}, axi2af={}, usb={}/{}, dp={}/{})",
            tunable_count,
            phy.common_a.len(),
            phy.common_b.len(),
            phy.axi2af_tunables.len(),
            phy.lane0_usb.len(),
            phy.lane1_usb.len(),
            phy.lane0_dp.len(),
            phy.lane1_dp.len()
        );
    }

    phy.init()?;

    let phandle = device
        .property("phandle")
        .and_then(|p| p.as_usize())
        .map(|v| v as u32)
        .or_else(|| {
            device
                .property("linux,phandle")
                .and_then(|p| p.as_usize())
                .map(|v| v as u32)
        })
        .unwrap_or(0);

    let (_id, phy_instance) = register_atcphy_shared(phy, phandle);
    let provider = Arc::new(AppleAtcPhyProvider::new(phy_instance));
    DeviceManager::get_manager()
        .register_phy_controller(phandle, Arc::clone(&provider) as Arc<dyn PhyProvider>);
    DeviceManager::get_manager()
        .register_reset_controller(phandle, provider as Arc<dyn ResetController>);

    early_println!("[apple-atcphy] registered (id={})", _id);
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_atcphy_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-atcphy",
        probe_fn,
        remove_fn,
        alloc::vec!["apple,t8103-atcphy", "apple,t6000-atcphy"],
    );

    // PHY must be registered before DWC3 (Core), so use Critical priority.
    // PHY nodes appear after USB nodes in Apple FDT, causing probe order issue.
    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical);
}

scarlet::driver_initcall!(register_atcphy_driver);

#[used]
static SCARLET_DRIVER_APPLE_ATCPHY_ANCHOR: fn() = force_link;

/// Keep the driver object linked into kernel builds that rely on initcall anchors.
#[inline(never)]
pub fn force_link() {}
