//! SD memory-card identification, shared by all MMC host controllers.
//!
//! Follows Linux drivers/mmc/core/{sd,sd_ops}.c: negotiate 3.3 V without
//! requesting UHS, decode CSD/SCR, select the card and configure legacy SDR.

use super::core::{MmcCardInfo, check_r1, sd_card_info};
use crate::device::mmc::{
    MmcBusWidth, MmcCommand, MmcData, MmcError, MmcHost, MmcResponse, MmcResponseType, MmcResult,
};

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

pub(super) fn initialize_sd(host: &mut dyn MmcHost, width: MmcBusWidth) -> MmcResult<MmcCardInfo> {
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

    // Only the 3.2–3.4 V OCR window; never request an unimplemented 1.8 V
    // switch, extra power, UHS timing or SD Express operation.
    let requested = 0x0030_0000 | if version2 { 1 << 30 } else { 0 };
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
    crate::println!(
        "[mmc] SD ready: rca={:#06x} sectors={} addressing={} width={:?} SCR={:02x?}",
        rca >> 16,
        sectors,
        if high_capacity { "sector" } else { "byte" },
        width,
        scr,
    );
    Ok(sd_card_info(sectors, high_capacity))
}
