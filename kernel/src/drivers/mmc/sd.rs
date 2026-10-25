//! SD memory-card identification, shared by all MMC host controllers.
//!
//! Negotiate card/host timing and voltage capabilities before selecting SD speed.

use super::core::{MmcCardInfo, check_r1, sd_card_info};
use crate::device::mmc::{
    MmcBusWidth, MmcCommand, MmcData, MmcError, MmcHost, MmcResponse, MmcResponseType, MmcResult,
    MmcTiming,
};

const CMD_SWITCH_FUNC: u8 = 6;
const SD_SWITCH_HIGH_SPEED: u32 = 0x80ff_fff1;
const SD_SWITCH_DEFAULT_SPEED: u32 = 0x80ff_fff0;
const SD_HIGH_SPEED_CLOCK_HZ: u32 = 50_000_000;
const R1_SWITCH_ERROR: u32 = 1 << 7;

fn command(
    host: &mut dyn MmcHost,
    index: u8,
    arg: u32,
    kind: MmcResponseType,
) -> MmcResult<MmcResponse> {
    host.send_command(MmcCommand::new(index, arg, kind), None)
}

fn app_command(host: &mut dyn MmcHost, rca: u32) -> MmcResult<()> {
    let status = command(host, 55, rca, MmcResponseType::R1)?.word(0);
    check_r1(status)?;
    if status & (1 << 5) == 0 {
        return Err(MmcError::Unsupported);
    }
    Ok(())
}

fn capacity(csd: MmcResponse, high_capacity: bool) -> MmcResult<u64> {
    match (csd.bits(126, 2), high_capacity) {
        (0, false) => {
            let read_bl_len = csd.bits(80, 4);
            // The driver transfers 512-byte logical sectors using CMD16.
            if !(9..=11).contains(&read_bl_len) {
                return Err(MmcError::Unsupported);
            }
            let size = u64::from(csd.bits(62, 12)) + 1;
            let shift = csd.bits(47, 3) + 2 + read_bl_len - 9;
            Ok(size << shift)
        }
        (1, true) => Ok((u64::from(csd.bits(48, 22)) + 1) * 1024),
        _ => Err(MmcError::Unsupported),
    }
}

fn switch_status(host: &mut dyn MmcHost, argument: u32) -> MmcResult<[u8; 64]> {
    let mut status = [0u8; 64];
    let response = host.send_command(
        MmcCommand::new(CMD_SWITCH_FUNC, argument, MmcResponseType::R1),
        Some(MmcData::Read(&mut status)),
    )?;
    check_r1(response.word(0))?;
    if response.word(0) & R1_SWITCH_ERROR != 0 {
        return Err(MmcError::Response);
    }
    Ok(status)
}

fn high_speed_selected(status: &[u8]) -> bool {
    status.get(16).is_some_and(|byte| byte & 0x0f == 1)
}

/// Select SD High Speed at 3.3 V.
///
/// Function group 1 reports the selected mode in bits 3:0 of byte 16. The
/// host switches to high-speed sampling only after the card accepts the mode.
/// A host that cannot change timing sends the card back to default speed, so
/// the caller can still finish at 25 MHz.
fn enable_high_speed(host: &mut dyn MmcHost) -> MmcResult<()> {
    let status = switch_status(host, SD_SWITCH_HIGH_SPEED)?;
    if !high_speed_selected(&status) {
        return Err(MmcError::Unsupported);
    }
    if host.set_timing(MmcTiming::HighSpeed).is_err() {
        let _ = switch_status(host, SD_SWITCH_DEFAULT_SPEED);
        return Err(MmcError::Unsupported);
    }
    Ok(())
}

fn enable_uhs(host: &mut dyn MmcHost) -> MmcResult<&'static str> {
    let status = switch_status(host, 0x00ff_ffff)?;
    let supported = status[13] & host.sd_uhs_modes();
    let (function, timing, clock, name) = if supported & (1 << 3) != 0 {
        (3, MmcTiming::Sdr104, 208_000_000, "sdr104")
    } else if supported & (1 << 2) != 0 {
        (2, MmcTiming::Sdr50, 100_000_000, "sdr50")
    } else if supported & (1 << 1) != 0 {
        (1, MmcTiming::Sdr25, 50_000_000, "sdr25")
    } else if supported & 1 != 0 {
        (0, MmcTiming::Sdr12, 25_000_000, "sdr12")
    } else {
        return Err(MmcError::Unsupported);
    };
    let selected = switch_status(host, 0x80ff_fff0 | function)?;
    if selected[16] & 15 != function as u8 {
        return Err(MmcError::Response);
    }
    host.set_timing(timing)?;
    host.set_clock(clock)?;
    // Platforms can skip tuning at fixed-feedback SDR50 rates when their
    // hardware permits it. SDR104 always validates the CMD19 data pattern.
    if timing == MmcTiming::Sdr104 || timing == MmcTiming::Sdr50 {
        host.execute_sd_tuning()?;
    }
    Ok(name)
}

pub(super) fn initialize_sd(host: &mut dyn MmcHost, width: MmcBusWidth) -> MmcResult<MmcCardInfo> {
    let uhs = host.sd_uhs_modes() != 0;
    match initialize_sd_mode(host, width, uhs) {
        Err(error) if uhs => {
            crate::println!(
                "[mmc] UHS initialization failed: {}; power-cycle and retry at 3.3 V",
                error.as_str()
            );
            initialize_sd_mode(host, width, false)
        }
        result => result,
    }
}

fn initialize_sd_mode(
    host: &mut dyn MmcHost,
    width: MmcBusWidth,
    allow_uhs: bool,
) -> MmcResult<MmcCardInfo> {
    if width == MmcBusWidth::Eight {
        return Err(MmcError::InvalidArgument);
    }
    if !host.card_present() {
        return Err(MmcError::NoMedia);
    }
    host.reset()?;
    host.set_bus_width(MmcBusWidth::One)?;
    host.set_clock(400_000)?;
    // At least 74 clocks with CMD high before CMD0 after power-on.
    crate::time::udelay(1_000);
    command(host, 0, 0, MmcResponseType::None)?;
    let version2 = match command(host, 8, 0x1aa, MmcResponseType::R7) {
        Ok(response) if response.word(0) & 0xfff == 0x1aa => true,
        // Older SDSC cards do not answer SEND_IF_COND.
        Err(MmcError::Timeout) => false,
        Ok(_) => return Err(MmcError::Response),
        Err(error) => return Err(error),
    };

    // Request S18R only when the platform can switch the rail and recover
    // through a full card power cycle. No extra power or SD Express request.
    let request_uhs = allow_uhs && version2 && width == MmcBusWidth::Four;
    let requested =
        0x0030_0000 | if version2 { 1 << 30 } else { 0 } | if request_uhs { 1 << 24 } else { 0 };
    let mut ocr = 0;
    for _ in 0..1000 {
        app_command(host, 0)?;
        ocr = command(host, 41, requested, MmcResponseType::R3)?.word(0);
        if ocr & (1 << 31) != 0 {
            break;
        }
        crate::time::udelay(1_000);
    }
    if ocr & (1 << 31) == 0 {
        return Err(MmcError::Timeout);
    }
    if ocr & 0x0030_0000 == 0 {
        return Err(MmcError::Unsupported);
    }
    let high_capacity = version2 && ocr & (1 << 30) != 0;
    let uhs = request_uhs && high_capacity && ocr & (1 << 24) != 0;
    if uhs {
        check_r1(command(host, 11, 0, MmcResponseType::R1)?.word(0))?;
        host.switch_signal_voltage_1v8()?;
    }
    command(host, 2, 0, MmcResponseType::R2)?;
    let response = command(host, 3, 0, MmcResponseType::R6)?.word(0);
    let rca = response & 0xffff_0000;
    // R6's lower status word differs from R1; the RCA is not an error mask.
    if rca == 0 || response & 0xe000 != 0 {
        return Err(MmcError::Response);
    }
    let csd = command(host, 9, rca, MmcResponseType::R2)?;
    let sectors = capacity(csd, high_capacity)?;
    check_r1(command(host, 7, rca, MmcResponseType::R1b)?.word(0))?;
    if !high_capacity {
        check_r1(command(host, 16, 512, MmcResponseType::R1)?.word(0))?;
    }

    app_command(host, rca)?;
    let mut scr = [0u8; 8];
    let response = host.send_command(
        MmcCommand::new(51, 0, MmcResponseType::R1),
        Some(MmcData::Read(&mut scr)),
    )?;
    check_r1(response.word(0))?;
    if scr[0] >> 4 != 0 || scr[1] & 1 == 0 {
        return Err(MmcError::Response);
    }
    if width == MmcBusWidth::Four {
        if scr[1] & 4 == 0 {
            return Err(MmcError::Unsupported);
        }
        app_command(host, rca)?;
        check_r1(command(host, 6, 2, MmcResponseType::R1)?.word(0))?;
        host.set_bus_width(MmcBusWidth::Four)?;
    }
    host.set_clock(25_000_000)?;
    let timing = if uhs {
        enable_uhs(host)?
    } else {
        match enable_high_speed(host) {
            Ok(()) => match host.set_clock(SD_HIGH_SPEED_CLOCK_HZ) {
                Ok(()) => "high-speed",
                Err(_) => {
                    // The card is already in high-speed mode, which remains valid
                    // at 25 MHz. Restore legacy host sampling for the fallback.
                    let _ = host.set_timing(MmcTiming::Legacy);
                    let _ = host.set_clock(25_000_000);
                    "default"
                }
            },
            Err(_) => {
                let _ = host.set_timing(MmcTiming::Legacy);
                "default"
            }
        }
    };
    crate::println!(
        "[mmc] SD ready: rca={:#06x} sectors={} addressing={} width={:?} timing={} SCR={:02x?}",
        rca >> 16,
        sectors,
        if high_capacity { "sector" } else { "byte" },
        width,
        timing,
        scr,
    );
    Ok(sd_card_info(sectors, high_capacity))
}

#[cfg(test)]
mod tests {
    use super::high_speed_selected;

    #[test_case]
    fn high_speed_status_requires_group_one_function_one() {
        let mut status = [0u8; 64];
        status[16] = 0x01;
        assert!(high_speed_selected(&status));
        status[16] = 0x02;
        assert!(!high_speed_selected(&status));
        status[16] = 0xf1;
        assert!(high_speed_selected(&status));
        assert!(!high_speed_selected(&[]));
    }
    struct TimingHost {
        commands: alloc::vec::Vec<u32>,
        timing: crate::device::mmc::MmcTiming,
        clock: u32,
        tuned: bool,
        reject: bool,
    }
    impl super::MmcHost for TimingHost {
        fn reset(&mut self) -> super::MmcResult<()> {
            Ok(())
        }
        fn set_clock(&mut self, clock: u32) -> super::MmcResult<()> {
            self.clock = clock;
            Ok(())
        }
        fn set_bus_width(&mut self, _: super::MmcBusWidth) -> super::MmcResult<()> {
            Ok(())
        }
        fn set_timing(&mut self, timing: super::MmcTiming) -> super::MmcResult<()> {
            self.timing = timing;
            Ok(())
        }
        fn card_present(&self) -> bool {
            true
        }
        fn is_removable(&self) -> bool {
            true
        }
        fn sd_uhs_modes(&self) -> u8 {
            15
        }
        fn execute_sd_tuning(&mut self) -> super::MmcResult<()> {
            self.tuned = true;
            Ok(())
        }
        fn send_command(
            &mut self,
            command: super::MmcCommand,
            data: Option<super::MmcData<'_>>,
        ) -> super::MmcResult<super::MmcResponse> {
            assert_eq!(command.index(), 6);
            self.commands.push(command.argument());
            if let Some(super::MmcData::Read(status)) = data {
                status[13] = 15;
                status[16] = if self.reject {
                    15
                } else {
                    (command.argument() & 15) as u8
                };
            }
            Ok(super::MmcResponse::new([0; 4]))
        }
    }
    fn timing_host(reject: bool) -> TimingHost {
        TimingHost {
            commands: alloc::vec::Vec::new(),
            timing: super::MmcTiming::Legacy,
            clock: 25_000_000,
            tuned: false,
            reject,
        }
    }
    #[test_case]
    fn high_speed_cmd6_sets_switch_mode_bit() {
        let mut host = timing_host(false);
        super::enable_high_speed(&mut host).unwrap();
        assert_eq!(host.commands.as_slice(), &[0x80ff_fff1]);
        assert_eq!(host.timing, super::MmcTiming::HighSpeed);
    }
    #[test_case]
    fn uhs_checks_support_then_switches_and_tunes() {
        let mut host = timing_host(false);
        assert_eq!(super::enable_uhs(&mut host), Ok("sdr104"));
        assert_eq!(host.commands.as_slice(), &[0x00ff_ffff, 0x80ff_fff3]);
        assert_eq!(host.timing, super::MmcTiming::Sdr104);
        assert_eq!(host.clock, 208_000_000);
        assert!(host.tuned);
    }
    #[test_case]
    fn rejected_uhs_mode_does_not_raise_host_clock() {
        let mut host = timing_host(true);
        assert!(super::enable_uhs(&mut host).is_err());
        assert_eq!(host.clock, 25_000_000);
        assert_eq!(host.timing, super::MmcTiming::Legacy);
        assert!(!host.tuned);
    }
}
