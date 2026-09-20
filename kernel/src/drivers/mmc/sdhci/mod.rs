//! Generic Secure Digital Host Controller Interface support.
//!
//! This module implements the standard SDHCI register interface using PIO or
//! owned ADMA2 transfers. Bus discovery and platform quirks live in bindings.

use crate::device::mmc::{
    MmcBusWidth, MmcCommand, MmcData, MmcError, MmcHost, MmcResponse, MmcResponseType, MmcResult,
};

mod adma;
pub mod pci;

mod register {
    pub const BLOCK_SIZE: usize = 0x04;
    pub const BLOCK_COUNT: usize = 0x06;
    pub const ARGUMENT: usize = 0x08;
    pub const TRANSFER_MODE: usize = 0x0c;
    pub const COMMAND: usize = 0x0e;
    pub const RESPONSE_0: usize = 0x10;
    pub const BUFFER_DATA: usize = 0x20;
    pub const PRESENT_STATE: usize = 0x24;
    pub const HOST_CONTROL: usize = 0x28;
    pub const POWER_CONTROL: usize = 0x29;
    pub const CLOCK_CONTROL: usize = 0x2c;
    pub const TIMEOUT_CONTROL: usize = 0x2e;
    pub const SOFTWARE_RESET: usize = 0x2f;
    pub const INTERRUPT_STATUS: usize = 0x30;
    pub const NORMAL_STATUS_ENABLE: usize = 0x34;
    pub const ERROR_STATUS_ENABLE: usize = 0x36;
    pub const NORMAL_SIGNAL_ENABLE: usize = 0x38;
    pub const ERROR_SIGNAL_ENABLE: usize = 0x3a;
    pub const HOST_CONTROL2: usize = 0x3e;
    pub const CAPABILITIES: usize = 0x40;
    pub const ADMA_ADDRESS: usize = 0x58;
    pub const ADMA_ADDRESS_HIGH: usize = 0x5c;
    pub const HOST_VERSION: usize = 0xfe;
}

mod present_state {
    pub const COMMAND_INHIBIT: u32 = 1 << 0;
    pub const DATA_INHIBIT: u32 = 1 << 1;
    pub const BUFFER_WRITE_ENABLE: u32 = 1 << 10;
    pub const BUFFER_READ_ENABLE: u32 = 1 << 11;
    pub const CARD_INSERTED: u32 = 1 << 16;
}

mod transfer_mode {
    pub const DMA_ENABLE: u16 = 1 << 0;
    pub const BLOCK_COUNT_ENABLE: u16 = 1 << 1;
    pub const READ: u16 = 1 << 4;
    pub const MULTI_BLOCK: u16 = 1 << 5;
}

mod command_flag {
    pub const RESPONSE_136: u16 = 1 << 0;
    pub const RESPONSE_48: u16 = 1 << 1;
    pub const RESPONSE_48_BUSY: u16 = 3 << 0;
    pub const CRC_CHECK: u16 = 1 << 3;
    pub const INDEX_CHECK: u16 = 1 << 4;
    pub const DATA_PRESENT: u16 = 1 << 5;
}

mod interrupt {
    pub const COMMAND_COMPLETE: u32 = 1 << 0;
    pub const TRANSFER_COMPLETE: u32 = 1 << 1;
    pub const BUFFER_WRITE_READY: u32 = 1 << 4;
    pub const BUFFER_READ_READY: u32 = 1 << 5;
    pub const CARD_INSERTION: u32 = 1 << 6;
    pub const CARD_REMOVAL: u32 = 1 << 7;
    pub const ERROR: u32 = 1 << 15;
    pub const ERROR_MASK: u32 = 0xffff_0000;
}

mod clock_control {
    pub const INTERNAL_ENABLE: u16 = 1 << 0;
    pub const INTERNAL_STABLE: u16 = 1 << 1;
    pub const CARD_ENABLE: u16 = 1 << 2;
}

mod software_reset {
    pub const ALL: u8 = 1 << 0;
    pub const COMMAND: u8 = 1 << 1;
    pub const DATA: u8 = 1 << 2;
}

const HOST_CONTROL_DATA_WIDTH_4: u8 = 1 << 1;
const HOST_CONTROL_HIGH_SPEED_ENABLE: u8 = 1 << 2;
const HOST_CONTROL_DMA_SELECT_MASK: u8 = 0x18;
const HOST_CONTROL_DATA_WIDTH_8: u8 = 1 << 5;
const HOST_CONTROL2_UHS_MODE_MASK: u16 = 0x0007;
const POWER_ON: u8 = 1 << 0;
const POWER_1V8: u8 = 0x0a;
const POWER_3V0: u8 = 0x0c;
const POWER_3V3: u8 = 0x0e;
const CAPABILITY_VOLTAGE_3V3: u32 = 1 << 24;
const CAPABILITY_VOLTAGE_3V0: u32 = 1 << 25;
const CAPABILITY_VOLTAGE_1V8: u32 = 1 << 26;
const COMMAND_TIMEOUT_US: u64 = 1_000_000;
const RESET_TIMEOUT_US: u64 = 100_000;
const CLOCK_TIMEOUT_US: u64 = 100_000;
const DEFAULT_BASE_CLOCK_HZ: u32 = 50_000_000;
const MMC_BLOCK_SIZE: usize = 512;

/// Platform-specific settings for a generic SDHCI host.
///
/// All settings default to the standard SDHCI behavior, so PCI callers do
/// not need to provide this value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SdhciHostConfig {
    /// Avoid intermediate writes to `POWER_CONTROL` while enabling power.
    ///
    /// When `true`, power-on emits only one write containing the selected
    /// voltage and `POWER_ON`. This is for controllers that reject both a
    /// clear write and a voltage-only write during power-up.
    pub single_power_write: bool,

    /// Preserve firmware-owned `POWER_CONTROL` during host reset.
    ///
    /// When `true`, reset validates the inherited `POWER_CONTROL` value before
    /// and after a command/data-only reset, without writing it. The inherited
    /// value must retain `POWER_ON` with a standard supported voltage, or reset
    /// returns [`MmcError::Unsupported`]. This is for platforms whose firmware
    /// owns an always-on card power supply.
    pub preserve_power_control: bool,

    /// Drain posted register writes before the next access. FIFO writes use
    /// PRESENT_STATE as the readback so they never consume receive data.
    pub write_readback: bool,

    /// The platform wrapper checks a GPIO because the controller's internal
    /// card-detect input is not wired. The wrapper must check before commands.
    pub external_card_detect: bool,

    /// Reset the command and data state machines together on any error.
    pub reset_command_and_data_together: bool,

    /// Use 16-byte rather than standard 12-byte ADMA2 64-bit descriptors.
    /// NVIDIA's Tegra210 SDHCI integration uses this padded address format.
    pub adma2_64bit_descriptor_16: bool,
}

/// Generic MMIO-backed SDHCI host.
pub struct SdhciHost {
    mmio_base: usize,
    non_removable: bool,
    base_clock_hz: u32,
    specification_version: u8,
    config: SdhciHostConfig,
    adma: Option<adma::Adma2>,
    adma_active: bool,
    poisoned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResetPlan {
    mask: u8,
    write_power_control: bool,
    normalize_host_timing: bool,
}

impl ResetPlan {
    #[cfg(test)]
    const fn power_control_write_count(self, single_power_write: bool) -> usize {
        if !self.write_power_control {
            0
        } else if single_power_write {
            1
        } else {
            3
        }
    }

    const fn normalized_host_controls(
        self,
        host_control: u8,
        host_control2: u16,
    ) -> Option<(u8, u16)> {
        if self.normalize_host_timing {
            Some((
                host_control & !(HOST_CONTROL_HIGH_SPEED_ENABLE | HOST_CONTROL_DMA_SELECT_MASK),
                host_control2 & !HOST_CONTROL2_UHS_MODE_MASK,
            ))
        } else {
            None
        }
    }
}

impl SdhciHost {
    /// Create an SDHCI host over an already mapped register aperture.
    ///
    /// # Arguments
    ///
    /// * `mmio_base` - Virtual address of the SDHCI register aperture.
    /// * `non_removable` - Whether the slot contains soldered eMMC.
    ///
    /// # Returns
    ///
    /// An initialized register accessor whose base clock is read from the
    /// controller's standard `CAPABILITIES` register. The controller is not
    /// reset until [`MmcHost::reset`] is called.
    pub fn new(mmio_base: usize, non_removable: bool) -> Self {
        Self::new_with_config(mmio_base, non_removable, SdhciHostConfig::default())
    }

    /// Create an SDHCI host with platform-specific settings.
    ///
    /// # Arguments
    ///
    /// * `mmio_base` - Virtual address of the SDHCI register aperture.
    /// * `non_removable` - Whether the slot contains soldered eMMC.
    /// * `config` - Generic controller settings selected by the platform.
    ///
    /// # Returns
    ///
    /// An initialized register accessor whose base clock is read from the
    /// controller's standard `CAPABILITIES` register. The controller is not
    /// reset until [`MmcHost::reset`] is called.
    pub fn new_with_config(mmio_base: usize, non_removable: bool, config: SdhciHostConfig) -> Self {
        let capabilities = Self::read32_at(mmio_base, register::CAPABILITIES);
        let base_clock_hz = Self::base_clock_hz_from_capabilities(capabilities);

        Self::new_with_base_clock_and_config(mmio_base, non_removable, base_clock_hz, config)
    }

    /// Create an SDHCI host with a platform-provided source clock.
    ///
    /// Use this constructor when the standard SDHCI `CAPABILITIES` base-clock
    /// field is absent or unreliable. A zero `base_clock_hz` is treated as an
    /// unknown clock and falls back to the same conservative default as
    /// [`Self::new`].
    ///
    /// # Arguments
    ///
    /// * `mmio_base` - Virtual address of the SDHCI register aperture.
    /// * `non_removable` - Whether the slot contains soldered eMMC.
    /// * `base_clock_hz` - Source clock frequency supplied by the platform,
    ///   in hertz; zero selects the default fallback clock.
    ///
    /// # Returns
    ///
    /// An initialized register accessor. The controller is not reset until
    /// [`MmcHost::reset`] is called.
    pub fn new_with_base_clock(mmio_base: usize, non_removable: bool, base_clock_hz: u32) -> Self {
        Self::new_with_base_clock_and_config(
            mmio_base,
            non_removable,
            base_clock_hz,
            SdhciHostConfig::default(),
        )
    }

    /// Create an SDHCI host with a platform-provided source clock and settings.
    ///
    /// Use this constructor when the standard SDHCI `CAPABILITIES` base-clock
    /// field is absent or unreliable and the controller requires generic
    /// SDHCI quirk handling.
    ///
    /// # Arguments
    ///
    /// * `mmio_base` - Virtual address of the SDHCI register aperture.
    /// * `non_removable` - Whether the slot contains soldered eMMC.
    /// * `base_clock_hz` - Source clock frequency supplied by the platform,
    ///   in hertz; zero selects the default fallback clock.
    /// * `config` - Generic controller settings selected by the platform.
    ///
    /// # Returns
    ///
    /// An initialized register accessor. The controller is not reset until
    /// [`MmcHost::reset`] is called.
    pub fn new_with_base_clock_and_config(
        mmio_base: usize,
        non_removable: bool,
        base_clock_hz: u32,
        config: SdhciHostConfig,
    ) -> Self {
        let specification_version =
            (Self::read16_at(mmio_base, register::HOST_VERSION) & 0xff) as u8;

        Self {
            mmio_base,
            non_removable,
            base_clock_hz: Self::normalize_base_clock_hz(base_clock_hz),
            specification_version,
            config,
            adma: None,
            adma_active: false,
            poisoned: false,
        }
    }

    /// Return the virtual base address of this controller's register aperture.
    ///
    /// # Returns
    ///
    /// The MMIO base address supplied when this host was constructed.
    pub const fn mmio_base(&self) -> usize {
        self.mmio_base
    }

    /// Enable ADMA2 after the binding has established a valid DMA context.
    /// Unsupported controllers retain the existing PIO path. No caller buffer
    /// is exposed to DMA; mappings and aligned storage are allocated once.
    pub fn enable_adma2(
        &mut self,
        context: &crate::device::iommu::DmaContext,
    ) -> Result<(), &'static str> {
        if self.adma_active || self.poisoned || self.adma.is_some() {
            return Err("SDHCI: cannot replace an active DMA context");
        }
        let capabilities = self.read32(register::CAPABILITIES);
        if capabilities == u32::MAX
            || capabilities & (1 << 19) == 0
            || self.specification_version < 1
            || self.specification_version > 3
        {
            return Err("SDHCI: ADMA2 requires a supported SDHCI 2.00/3.00/4.00 controller");
        }
        self.adma = Some(adma::Adma2::new(
            context,
            capabilities & (1 << 28) != 0,
            self.config.adma2_64bit_descriptor_16 || self.specification_version == 3,
        )?);
        Ok(())
    }

    /// Return the source clock used to program the SDHCI divider.
    ///
    /// # Returns
    ///
    /// The configured source clock frequency in hertz. This is never zero.
    pub const fn base_clock_hz(&self) -> u32 {
        self.base_clock_hz
    }

    /// Return the SDHCI specification version reported by the controller.
    ///
    /// # Returns
    ///
    /// The low byte of the `HOST_VERSION` register.
    pub const fn specification_version(&self) -> u8 {
        self.specification_version
    }

    /// Return whether the controller's slot is configured as non-removable.
    ///
    /// # Returns
    ///
    /// `true` for soldered media such as eMMC, or `false` for a removable
    /// slot.
    pub const fn is_non_removable(&self) -> bool {
        self.non_removable
    }

    /// Return whether power-up uses a single combined register write.
    ///
    /// # Returns
    ///
    /// `true` when the platform enabled the `single_power_write` quirk.
    pub const fn single_power_write(&self) -> bool {
        self.config.single_power_write
    }

    /// Return whether reset preserves the inherited power-control register.
    ///
    /// # Returns
    ///
    /// `true` when the platform enabled the `preserve_power_control` quirk.
    pub const fn preserves_power_control(&self) -> bool {
        self.config.preserve_power_control
    }

    const fn normalize_base_clock_hz(base_clock_hz: u32) -> u32 {
        if base_clock_hz == 0 {
            DEFAULT_BASE_CLOCK_HZ
        } else {
            base_clock_hz
        }
    }

    const fn base_clock_hz_from_capabilities(capabilities: u32) -> u32 {
        let reported_base_clock_mhz = (capabilities >> 8) & 0xff;
        Self::normalize_base_clock_hz(reported_base_clock_mhz.saturating_mul(1_000_000))
    }

    fn read8_at(base: usize, offset: usize) -> u8 {
        // SAFETY: `base` is a mapped SDHCI aperture and every call uses a
        // standard register offset within that aperture.
        unsafe { crate::arch::mmio::read8(base + offset) }
    }

    fn read16_at(base: usize, offset: usize) -> u16 {
        // SAFETY: `base` is a mapped SDHCI aperture and every call uses an
        // aligned standard register offset within that aperture.
        unsafe { crate::arch::mmio::read16(base + offset) }
    }

    fn read32_at(base: usize, offset: usize) -> u32 {
        // SAFETY: `base` is a mapped SDHCI aperture and every call uses an
        // aligned standard register offset within that aperture.
        unsafe { crate::arch::mmio::read32(base + offset) }
    }

    fn read8(&self, offset: usize) -> u8 {
        Self::read8_at(self.mmio_base, offset)
    }

    fn read16(&self, offset: usize) -> u16 {
        Self::read16_at(self.mmio_base, offset)
    }

    fn read32(&self, offset: usize) -> u32 {
        Self::read32_at(self.mmio_base, offset)
    }

    fn write8(&self, offset: usize, value: u8) {
        // SAFETY: `mmio_base` is a mapped SDHCI aperture and every call uses a
        // standard register offset within that aperture.
        unsafe { crate::arch::mmio::write8(self.mmio_base + offset, value) }
        if self.config.write_readback {
            let _ = self.read8(offset);
        }
    }

    fn write16(&self, offset: usize, value: u16) {
        // SAFETY: `mmio_base` is a mapped SDHCI aperture and every call uses an
        // aligned standard register offset within that aperture.
        unsafe { crate::arch::mmio::write16(self.mmio_base + offset, value) }
        if self.config.write_readback {
            let _ = self.read16(offset);
        }
    }

    fn write32(&self, offset: usize, value: u32) {
        // SAFETY: `mmio_base` is a mapped SDHCI aperture and every call uses an
        // aligned standard register offset within that aperture.
        unsafe { crate::arch::mmio::write32(self.mmio_base + offset, value) }
        if self.config.write_readback {
            let _ = self.read32(if offset == register::BUFFER_DATA {
                register::PRESENT_STATE
            } else {
                offset
            });
        }
    }

    fn wait_until(&self, timeout_us: u64, mut condition: impl FnMut() -> bool) -> MmcResult<()> {
        let started = crate::time::current_time();
        loop {
            if condition() {
                return Ok(());
            }
            if crate::time::current_time().wrapping_sub(started) >= timeout_us {
                return Err(MmcError::Timeout);
            }
            core::hint::spin_loop();
        }
    }

    fn reset_lines(&self, mask: u8) -> MmcResult<()> {
        let mask = if self.config.reset_command_and_data_together
            && mask & (software_reset::COMMAND | software_reset::DATA) != 0
        {
            mask | software_reset::COMMAND | software_reset::DATA
        } else {
            mask
        };
        self.write8(register::SOFTWARE_RESET, mask);
        self.wait_until(RESET_TIMEOUT_US, || {
            self.read8(register::SOFTWARE_RESET) & mask == 0
        })
    }

    fn power_on(&self) -> MmcResult<()> {
        let capabilities = self.read32(register::CAPABILITIES);
        let voltage = if capabilities & CAPABILITY_VOLTAGE_3V3 != 0 {
            POWER_3V3
        } else if capabilities & CAPABILITY_VOLTAGE_3V0 != 0 {
            POWER_3V0
        } else if capabilities & CAPABILITY_VOLTAGE_1V8 != 0 {
            POWER_1V8
        } else {
            return Err(MmcError::Unsupported);
        };

        let (writes, write_count) = Self::power_control_writes(self.single_power_write(), voltage);
        for value in writes.iter().take(write_count) {
            self.write8(register::POWER_CONTROL, *value);
        }
        Ok(())
    }

    const fn is_supported_power_control(power_control: u8) -> bool {
        power_control & POWER_ON != 0
            && matches!(power_control & 0x0e, POWER_3V3 | POWER_3V0 | POWER_1V8)
    }

    const fn reset_plan(preserve_power_control: bool) -> ResetPlan {
        if preserve_power_control {
            ResetPlan {
                mask: software_reset::COMMAND | software_reset::DATA,
                write_power_control: false,
                normalize_host_timing: true,
            }
        } else {
            ResetPlan {
                mask: software_reset::ALL,
                write_power_control: true,
                normalize_host_timing: false,
            }
        }
    }

    fn power_control_writes(single_power_write: bool, voltage: u8) -> ([u8; 3], usize) {
        if single_power_write {
            ([voltage | POWER_ON, 0, 0], 1)
        } else {
            ([0, voltage, voltage | POWER_ON], 3)
        }
    }

    fn clock_divider(&self, frequency_hz: u32) -> u16 {
        if frequency_hz == 0 || frequency_hz >= self.base_clock_hz {
            return 0;
        }

        if self.specification_version >= 2 {
            let denominator = u64::from(frequency_hz).saturating_mul(2);
            let divider = u64::from(self.base_clock_hz)
                .div_ceil(denominator)
                .clamp(1, 0x3ff) as u16;
            ((divider & 0xff) << 8) | ((divider & 0x300) >> 2)
        } else {
            let mut divisor = 2u32;
            while divisor < 256 && self.base_clock_hz / divisor > frequency_hz {
                divisor = divisor.saturating_mul(2);
            }
            (((divisor / 2).min(0xff) as u16) & 0xff) << 8
        }
    }

    fn wait_for_inhibit(&self, include_data: bool) -> MmcResult<()> {
        let mask = present_state::COMMAND_INHIBIT
            | if include_data {
                present_state::DATA_INHIBIT
            } else {
                0
            };
        self.wait_until(COMMAND_TIMEOUT_US, || {
            self.read32(register::PRESENT_STATE) & mask == 0
        })
    }

    fn wait_for_interrupt(&self, wanted: u32, data_phase: bool) -> MmcResult<u32> {
        let started = crate::time::current_time();
        let may_sleep = self.adma_active
            && crate::arch::interrupt::are_interrupts_enabled()
            && crate::sync::preempt::preempt_count() == 0
            && crate::sched::scheduler::scheduler_ready(crate::arch::get_cpu().get_cpuid());
        loop {
            let status = self.read32(register::INTERRUPT_STATUS);
            if status & interrupt::ERROR != 0 || status & interrupt::ERROR_MASK != 0 {
                self.write32(register::INTERRUPT_STATUS, status);
                let reset = if data_phase {
                    software_reset::COMMAND | software_reset::DATA
                } else {
                    software_reset::COMMAND
                };
                let _ = self.reset_lines(reset);
                return Err(if status & ((1 << 16) | (1 << 20)) != 0 {
                    MmcError::Timeout
                } else if data_phase {
                    MmcError::Data
                } else {
                    MmcError::Command
                });
            }
            if status & wanted != 0 {
                return Ok(status);
            }
            if crate::time::current_time().wrapping_sub(started) >= COMMAND_TIMEOUT_US {
                let reset = if data_phase {
                    software_reset::COMMAND | software_reset::DATA
                } else {
                    software_reset::COMMAND
                };
                let _ = self.reset_lines(reset);
                return Err(MmcError::Timeout);
            }
            // DMA moves the payload independently. After a short command
            // latency window, relinquish the CPU between status checks. Early
            // identification still works before the scheduler/timer is ready.
            if may_sleep && crate::time::current_time().wrapping_sub(started) >= 50 {
                if let Some(task) = crate::task::mytask() {
                    task.sleep_with_precision(
                        task.get_trapframe(),
                        100_000,
                        crate::timer::TimerPrecision::Exact,
                    );
                }
            } else {
                core::hint::spin_loop();
            }
        }
    }

    fn read_pio(&self, buffer: &mut [u8], block_size: usize) -> MmcResult<()> {
        for block in buffer.chunks_exact_mut(block_size) {
            let status = self.wait_for_interrupt(interrupt::BUFFER_READ_READY, true)?;
            self.wait_until(COMMAND_TIMEOUT_US, || {
                self.read32(register::PRESENT_STATE) & present_state::BUFFER_READ_ENABLE != 0
            })?;
            self.write32(
                register::INTERRUPT_STATUS,
                status & interrupt::BUFFER_READ_READY,
            );
            for bytes in block.chunks_mut(4) {
                let value = self.read32(register::BUFFER_DATA).to_le_bytes();
                bytes.copy_from_slice(&value[..bytes.len()]);
            }
        }
        Ok(())
    }

    fn write_pio(&self, buffer: &[u8], block_size: usize) -> MmcResult<()> {
        for block in buffer.chunks_exact(block_size) {
            let status = self.wait_for_interrupt(interrupt::BUFFER_WRITE_READY, true)?;
            self.wait_until(COMMAND_TIMEOUT_US, || {
                self.read32(register::PRESENT_STATE) & present_state::BUFFER_WRITE_ENABLE != 0
            })?;
            self.write32(
                register::INTERRUPT_STATUS,
                status & interrupt::BUFFER_WRITE_READY,
            );
            for bytes in block.chunks(4) {
                let mut value = [0u8; 4];
                value[..bytes.len()].copy_from_slice(bytes);
                self.write32(register::BUFFER_DATA, u32::from_le_bytes(value));
            }
        }
        Ok(())
    }

    fn command_bits(command: MmcCommand, has_data: bool) -> u16 {
        let response = match command.response() {
            MmcResponseType::None => 0,
            MmcResponseType::R1 | MmcResponseType::R6 | MmcResponseType::R7 => {
                command_flag::RESPONSE_48 | command_flag::CRC_CHECK | command_flag::INDEX_CHECK
            }
            MmcResponseType::R1b => {
                command_flag::RESPONSE_48_BUSY | command_flag::CRC_CHECK | command_flag::INDEX_CHECK
            }
            MmcResponseType::R2 => command_flag::RESPONSE_136 | command_flag::CRC_CHECK,
            MmcResponseType::R3 => command_flag::RESPONSE_48,
        };
        ((u16::from(command.index()) & 0x3f) << 8)
            | response
            | if has_data {
                command_flag::DATA_PRESENT
            } else {
                0
            }
    }
}

impl MmcHost for SdhciHost {
    fn max_blocks_per_transfer(&self) -> usize {
        // Bound a polling request to 64 KiB. Larger block-layer requests are
        // split by the card layer rather than monopolizing the host forever.
        128
    }

    fn reset(&mut self) -> MmcResult<()> {
        if self.poisoned {
            return Err(MmcError::Data);
        }
        let reset_plan = Self::reset_plan(self.preserves_power_control());
        let inherited_power_control = if reset_plan.write_power_control {
            None
        } else {
            let power_control = self.read8(register::POWER_CONTROL);
            if !Self::is_supported_power_control(power_control) {
                return Err(MmcError::Unsupported);
            }
            Some(power_control)
        };

        self.reset_lines(reset_plan.mask)?;
        if let Some(inherited_power_control) = inherited_power_control {
            if self.read8(register::POWER_CONTROL) != inherited_power_control {
                return Err(MmcError::Unsupported);
            }
            self.write16(register::CLOCK_CONTROL, 0);
            let host_control = self.read8(register::HOST_CONTROL);
            let host_control2 = self.read16(register::HOST_CONTROL2);
            if let Some((host_control, host_control2)) =
                reset_plan.normalized_host_controls(host_control, host_control2)
            {
                // A command/data reset preserves firmware timing state. Use
                // the registers' native widths and clear only legacy timing
                // fields, preserving POWER_CONTROL and unrelated host state.
                self.write8(register::HOST_CONTROL, host_control);
                self.write16(register::HOST_CONTROL2, host_control2);
            }
        } else {
            self.write16(register::CLOCK_CONTROL, 0);
            self.power_on()?;
        }
        self.write8(register::TIMEOUT_CONTROL, 0x0e);
        self.write32(register::INTERRUPT_STATUS, u32::MAX);
        self.write16(
            register::NORMAL_STATUS_ENABLE,
            (interrupt::COMMAND_COMPLETE
                | interrupt::TRANSFER_COMPLETE
                | interrupt::BUFFER_WRITE_READY
                | interrupt::BUFFER_READ_READY
                | interrupt::CARD_INSERTION
                | interrupt::CARD_REMOVAL
                | interrupt::ERROR) as u16,
        );
        self.write16(register::ERROR_STATUS_ENABLE, u16::MAX);
        self.write16(register::NORMAL_SIGNAL_ENABLE, 0);
        self.write16(register::ERROR_SIGNAL_ENABLE, 0);
        self.set_bus_width(MmcBusWidth::One)
    }

    fn set_clock(&mut self, frequency_hz: u32) -> MmcResult<()> {
        if frequency_hz == 0 {
            self.write16(register::CLOCK_CONTROL, 0);
            return Ok(());
        }

        let divider = self.clock_divider(frequency_hz);
        self.write16(register::CLOCK_CONTROL, 0);
        self.write16(
            register::CLOCK_CONTROL,
            divider | clock_control::INTERNAL_ENABLE,
        );
        self.wait_until(CLOCK_TIMEOUT_US, || {
            self.read16(register::CLOCK_CONTROL) & clock_control::INTERNAL_STABLE != 0
        })?;
        self.write16(
            register::CLOCK_CONTROL,
            divider | clock_control::INTERNAL_ENABLE | clock_control::CARD_ENABLE,
        );
        Ok(())
    }

    fn set_bus_width(&mut self, width: MmcBusWidth) -> MmcResult<()> {
        let mut control = self.read8(register::HOST_CONTROL);
        control &= !(HOST_CONTROL_DATA_WIDTH_4 | HOST_CONTROL_DATA_WIDTH_8);
        match width {
            MmcBusWidth::One => {}
            MmcBusWidth::Four => control |= HOST_CONTROL_DATA_WIDTH_4,
            MmcBusWidth::Eight => control |= HOST_CONTROL_DATA_WIDTH_8,
        }
        self.write8(register::HOST_CONTROL, control);
        Ok(())
    }

    fn card_present(&self) -> bool {
        self.non_removable
            || self.config.external_card_detect
            || self.read32(register::PRESENT_STATE) & present_state::CARD_INSERTED != 0
    }

    fn is_removable(&self) -> bool {
        !self.non_removable
    }

    fn send_command(
        &mut self,
        command: MmcCommand,
        data: Option<MmcData<'_>>,
    ) -> MmcResult<MmcResponse> {
        if self.poisoned {
            return Err(MmcError::Data);
        }
        let result = self.issue_command(command, data);
        if self.adma_active {
            // Every failure after COMMAND publication must stop DMA before
            // storage can be reused or freed, including command-phase errors.
            if self
                .reset_lines(software_reset::COMMAND | software_reset::DATA)
                .is_err()
            {
                self.poisoned = true;
                // A broken controller may still fetch descriptors/data. Keep
                // mappings and backing alive permanently rather than freeing
                // memory that hardware can still access.
                core::mem::forget(self.adma.take());
            }
            self.adma_active = false;
        }
        result
    }
}

impl SdhciHost {
    fn issue_command(
        &mut self,
        command: MmcCommand,
        data: Option<MmcData<'_>>,
    ) -> MmcResult<MmcResponse> {
        if !self.card_present() {
            return Err(MmcError::NoMedia);
        }

        let (data_len, is_read) = data
            .as_ref()
            .map(|data| (data.len(), data.is_read()))
            .unwrap_or((0, false));
        if data.as_ref().is_some_and(MmcData::is_empty) {
            return Err(MmcError::InvalidArgument);
        }

        let (block_size, block_count) = if data_len == 0 {
            (0usize, 0usize)
        } else if data_len <= MMC_BLOCK_SIZE {
            (data_len, 1)
        } else if data_len.is_multiple_of(MMC_BLOCK_SIZE) {
            (MMC_BLOCK_SIZE, data_len / MMC_BLOCK_SIZE)
        } else {
            return Err(MmcError::InvalidArgument);
        };
        if block_size > 0x0fff || block_count > u16::MAX as usize {
            return Err(MmcError::InvalidArgument);
        }

        let data_phase = data_len != 0;
        let use_adma =
            self.adma.is_some() && data_len >= MMC_BLOCK_SIZE && data_len <= adma::MAX_TRANSFER;
        // CMD12 must be able to abort a data transfer with DATA_INHIBIT set.
        self.wait_for_inhibit(
            command.index() != 12
                && (data_phase || matches!(command.response(), MmcResponseType::R1b)),
        )?;
        self.write32(register::INTERRUPT_STATUS, u32::MAX);

        let mut mode = 0u16;
        if data_phase {
            // Some SDHCI integrations, including Qualcomm's v5 controller,
            // only latch the block count when the adjacent block-size/count
            // register pair is written as one 32-bit value.
            self.write32(
                register::BLOCK_SIZE,
                (block_size as u32) | ((block_count as u32) << 16),
            );
            mode |= transfer_mode::BLOCK_COUNT_ENABLE;
            if is_read {
                mode |= transfer_mode::READ;
            }
            if block_count > 1 {
                if !matches!(command.index(), 18 | 25) {
                    return Err(MmcError::Unsupported);
                }
                // Terminate open-ended SD/eMMC multi-block commands. Merely
                // programming BLOCK_COUNT does not stop the card itself.
                mode |= transfer_mode::MULTI_BLOCK | (1 << 2); // Auto CMD12.
            }
        }
        let dma_control = if use_adma {
            let adma = self.adma.as_mut().unwrap();
            adma.prepare(
                data_len,
                match data.as_ref() {
                    Some(MmcData::Write(buffer)) => Some(*buffer),
                    _ => None,
                },
            );
            let address = adma.table_address();
            let control = adma.host_control();
            let address_64 = control == 3 << 3;
            if self.specification_version == 3 {
                // SDHCI 4.00 selects address width in HOST_CONTROL2. The
                // HOST_CONTROL DMA selector remains ADMA2 (not legacy ADMA64).
                self.write16(
                    register::HOST_CONTROL2,
                    (self.read16(register::HOST_CONTROL2) & !(1 << 13))
                        | (1 << 12)
                        | if address_64 { 1 << 13 } else { 0 },
                );
            }
            self.write32(register::ADMA_ADDRESS, address as u32);
            if address_64 {
                self.write32(register::ADMA_ADDRESS_HIGH, (address >> 32) as u32);
            }
            mode |= transfer_mode::DMA_ENABLE;
            if self.specification_version == 3 {
                2 << 3
            } else {
                control
            }
        } else {
            0
        };
        // PIO must not inherit ADMA selection from the preceding command.
        self.write8(
            register::HOST_CONTROL,
            (self.read8(register::HOST_CONTROL) & !HOST_CONTROL_DMA_SELECT_MASK) | dma_control,
        );
        // Program TRANSFER_MODE for every command, including command-only
        // operations. Firmware may leave this register non-zero, and the
        // controller samples it together with the subsequent COMMAND write.
        self.write16(register::TRANSFER_MODE, mode);
        self.write32(register::ARGUMENT, command.argument());
        let encoded_command = Self::command_bits(command, data_phase);
        self.adma_active = use_adma;
        self.write16(register::COMMAND, encoded_command);

        let status = self.wait_for_interrupt(interrupt::COMMAND_COMPLETE, false)?;
        self.write32(
            register::INTERRUPT_STATUS,
            status & interrupt::COMMAND_COMPLETE,
        );

        let words = [
            self.read32(register::RESPONSE_0),
            self.read32(register::RESPONSE_0 + 4),
            self.read32(register::RESPONSE_0 + 8),
            self.read32(register::RESPONSE_0 + 12),
        ];
        let response = if command.response() == MmcResponseType::R2 {
            // SDHCI strips the CRC/end byte from a long response. Restore
            // protocol bit positions and return the most significant word
            // first, as Linux does before its host-independent CSD decoder.
            MmcResponse::new([
                (words[3] << 8) | (words[2] >> 24),
                (words[2] << 8) | (words[1] >> 24),
                (words[1] << 8) | (words[0] >> 24),
                words[0] << 8,
            ])
        } else {
            MmcResponse::new(words)
        };

        let mut data = data;
        if !use_adma {
            match data.as_mut() {
                Some(MmcData::Read(buffer)) => self.read_pio(buffer, block_size)?,
                Some(MmcData::Write(buffer)) => self.write_pio(buffer, block_size)?,
                None => {}
            }
        }

        if data_phase {
            let status = self.wait_for_interrupt(interrupt::TRANSFER_COMPLETE, true)?;
            self.write32(
                register::INTERRUPT_STATUS,
                status & interrupt::TRANSFER_COMPLETE,
            );
            self.adma_active = false;
            if use_adma {
                if let Some(MmcData::Read(buffer)) = data {
                    self.adma.as_ref().unwrap().finish_read(buffer);
                }
            }
        } else if matches!(command.response(), MmcResponseType::R1b) {
            self.wait_until(COMMAND_TIMEOUT_US, || {
                self.read32(register::PRESENT_STATE) & present_state::DATA_INHIBIT == 0
            })?;
        }

        Ok(response)
    }
}

impl Drop for SdhciHost {
    fn drop(&mut self) {
        // Bindings may already have unmapped MMIO, so no hardware access here.
        // Normal completion/error handling retires DMA before reaching Drop.
        if self.adma_active {
            core::mem::forget(self.adma.take());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn command_encoding_matches_sdhci_response_flags() {
        let command = MmcCommand::new(17, 42, MmcResponseType::R1);
        assert_eq!(
            SdhciHost::command_bits(command, true),
            (17 << 8)
                | command_flag::RESPONSE_48
                | command_flag::CRC_CHECK
                | command_flag::INDEX_CHECK
                | command_flag::DATA_PRESENT
        );
    }

    #[test_case]
    fn command_encoding_omits_checks_for_ocr() {
        let command = MmcCommand::new(1, 0, MmcResponseType::R3);
        assert_eq!(
            SdhciHost::command_bits(command, false),
            (1 << 8) | command_flag::RESPONSE_48
        );
    }

    #[test_case]
    fn base_clock_capability_uses_reported_frequency_or_fallback() {
        assert_eq!(
            SdhciHost::base_clock_hz_from_capabilities(0),
            DEFAULT_BASE_CLOCK_HZ
        );
        assert_eq!(
            SdhciHost::base_clock_hz_from_capabilities(200 << 8),
            200_000_000
        );
    }

    #[test_case]
    fn explicit_zero_base_clock_uses_fallback() {
        assert_eq!(SdhciHost::normalize_base_clock_hz(0), DEFAULT_BASE_CLOCK_HZ);
        assert_eq!(SdhciHost::normalize_base_clock_hz(19_200_000), 19_200_000);
    }

    #[test_case]
    fn single_power_write_avoids_intermediate_power_control_writes() {
        let voltage = POWER_1V8;
        assert_eq!(
            SdhciHost::power_control_writes(false, voltage),
            ([0, voltage, voltage | POWER_ON], 3)
        );
        assert_eq!(
            SdhciHost::power_control_writes(true, voltage),
            ([voltage | POWER_ON, 0, 0], 1)
        );
    }

    #[test_case]
    fn preserve_power_control_uses_non_destructive_reset_plan() {
        let plan = SdhciHost::reset_plan(true);
        assert_eq!(plan.mask, software_reset::COMMAND | software_reset::DATA);
        assert_eq!(plan.power_control_write_count(false), 0);
        assert_eq!(plan.power_control_write_count(true), 0);

        let default_plan = SdhciHost::reset_plan(false);
        assert_eq!(default_plan.mask, software_reset::ALL);
        assert_eq!(default_plan.power_control_write_count(false), 3);
        assert_eq!(default_plan.power_control_write_count(true), 1);
    }

    #[test_case]
    fn preserve_reset_normalizes_only_host_timing_fields() {
        let plan = SdhciHost::reset_plan(true);
        assert_eq!(plan.normalized_host_controls(0x24, 0x0004), Some((0x20, 0)));
        assert_eq!(
            plan.normalized_host_controls(0x3d, 0x000c),
            Some((0x21, 0x0008))
        );

        let default_plan = SdhciHost::reset_plan(false);
        assert_eq!(default_plan.normalized_host_controls(0x3d, 0x000c), None);
    }

    #[test_case]
    fn preserved_power_control_requires_an_enabled_supported_voltage() {
        assert!(SdhciHost::is_supported_power_control(POWER_1V8 | POWER_ON));
        assert!(SdhciHost::is_supported_power_control(POWER_3V0 | POWER_ON));
        assert!(SdhciHost::is_supported_power_control(POWER_3V3 | POWER_ON));
        assert!(!SdhciHost::is_supported_power_control(POWER_1V8));
        assert!(!SdhciHost::is_supported_power_control(POWER_ON));
        assert!(!SdhciHost::is_supported_power_control(0));
    }
}
