//! Host-independent eMMC card initialization and block I/O.

extern crate alloc;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::any::Any;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::device::block::BlockDevice;
use crate::device::block::request::{BlockIORequest, BlockIORequestType, BlockIOResult};
use crate::device::mmc::{
    MmcBusWidth, MmcCommand, MmcData, MmcError, MmcHost, MmcResponseType, MmcResult,
};
use crate::device::{Device, DeviceType};
use crate::object::capability::selectable::Selectable;
use crate::object::capability::{ControlOps, MemoryMappingOps};
use crate::sync::{IrqSpinLock, Mutex};

const CMD_GO_IDLE_STATE: u8 = 0;
const CMD_SEND_OP_COND: u8 = 1;
const CMD_ALL_SEND_CID: u8 = 2;
const CMD_SET_RELATIVE_ADDR: u8 = 3;
const CMD_SWITCH: u8 = 6;
const CMD_SELECT_CARD: u8 = 7;
const CMD_SEND_EXT_CSD: u8 = 8;
const CMD_SEND_CSD: u8 = 9;
const CMD_SET_BLOCK_LENGTH: u8 = 16;
const CMD_READ_SINGLE_BLOCK: u8 = 17;
const CMD_WRITE_SINGLE_BLOCK: u8 = 24;

const IDENTIFICATION_CLOCK_HZ: u32 = 400_000;
const LEGACY_MMC_CLOCK_HZ: u32 = 26_000_000;
const MMC_SECTOR_SIZE: usize = 512;
const MMC_RELATIVE_ADDRESS: u16 = 1;
const OCR_BUSY: u32 = 1 << 31;
const OCR_SECTOR_MODE: u32 = 1 << 30;
const OCR_VOLTAGE_WINDOW: u32 = 0x00ff_8080;
const OCR_POLL_ATTEMPTS: usize = 1_000;
const EXT_CSD_REVISION: usize = 192;
const EXT_CSD_DEVICE_TYPE: usize = 196;
const EXT_CSD_SECTOR_COUNT: usize = 212;
const EXT_CSD_BUS_WIDTH: u8 = 183;
const MMC_SWITCH_WRITE_BYTE: u32 = 3 << 24;
const EXT_CSD_BUS_WIDTH_4: u8 = 1;
const EXT_CSD_BUS_WIDTH_8: u8 = 2;
const R1_STATUS_ERROR_MASK: u32 = 0xfff9_a000;
const R1_SWITCH_ERROR: u32 = 1 << 7;

/// Information discovered while identifying one eMMC device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmmcCardInfo {
    sector_count: u64,
    high_capacity: bool,
    ext_csd_revision: u8,
    device_type: u8,
}

impl EmmcCardInfo {
    fn disk_size(self) -> usize {
        self.sector_count
            .saturating_mul(MMC_SECTOR_SIZE as u64)
            .min(usize::MAX as u64) as usize
    }

    /// Return the number of addressable 512-byte sectors.
    ///
    /// # Returns
    ///
    /// Card capacity expressed in sectors.
    pub const fn sector_count(self) -> u64 {
        self.sector_count
    }

    /// Return whether the card uses sector rather than byte addressing.
    ///
    /// # Returns
    ///
    /// `true` when commands address 512-byte sectors directly, or `false`
    /// when commands use byte addresses.
    pub const fn is_high_capacity(self) -> bool {
        self.high_capacity
    }

    /// Return the decoded EXT_CSD revision byte.
    ///
    /// # Returns
    ///
    /// Raw `EXT_CSD_REV` value reported by the card.
    pub const fn ext_csd_revision(self) -> u8 {
        self.ext_csd_revision
    }

    /// Return the EXT_CSD device-type capability byte.
    ///
    /// # Returns
    ///
    /// Raw `DEVICE_TYPE` value reported by the card.
    pub const fn device_type(self) -> u8 {
        self.device_type
    }
}

/// Block-device adapter around one initialized eMMC card.
#[allow(clippy::vec_box)]
pub struct EmmcBlockDevice {
    name: &'static str,
    host: Mutex<Box<dyn MmcHost>>,
    card: EmmcCardInfo,
    request_queue: IrqSpinLock<Vec<Box<BlockIORequest>>>,
    media_online: AtomicBool,
    media_generation: AtomicU64,
}

impl EmmcBlockDevice {
    /// Identify an eMMC card and construct its block-device adapter.
    ///
    /// # Arguments
    ///
    /// * `name` - Stable device name published through `DeviceManager`.
    /// * `host` - Host controller connected to the eMMC card.
    ///
    /// # Returns
    ///
    /// An initialized block device, or the card/transport error encountered
    /// during identification.
    pub fn probe(name: &'static str, host: Box<dyn MmcHost>) -> MmcResult<Self> {
        Self::probe_with_bus_width(name, host, MmcBusWidth::One)
    }

    /// Identify an eMMC card and construct its block-device adapter at a bus width.
    ///
    /// The card and host are both configured for `bus_width` while still at
    /// the legacy MMC clock. This supports only SDR one-, four-, and eight-bit
    /// operation; it does not enable high-speed timing modes.
    ///
    /// # Arguments
    ///
    /// * `name` - Stable device name published through `DeviceManager`.
    /// * `host` - Host controller connected to the eMMC card.
    /// * `bus_width` - Data-bus width required by the board wiring.
    ///
    /// # Returns
    ///
    /// An initialized block device, or the card/transport error encountered
    /// during identification or bus-width configuration.
    pub fn probe_with_bus_width(
        name: &'static str,
        mut host: Box<dyn MmcHost>,
        bus_width: MmcBusWidth,
    ) -> MmcResult<Self> {
        let card = initialize_emmc(host.as_mut(), bus_width)?;
        Ok(Self {
            name,
            host: Mutex::new(host),
            card,
            request_queue: IrqSpinLock::new(Vec::new()),
            media_online: AtomicBool::new(true),
            media_generation: AtomicU64::new(1),
        })
    }

    /// Return the current media generation.
    ///
    /// # Returns
    ///
    /// A value incremented when removal is first observed. Future removable
    /// media support uses this to invalidate stale partition endpoints.
    pub fn media_generation(&self) -> u64 {
        self.media_generation.load(Ordering::Acquire)
    }

    /// Return information discovered during card identification.
    ///
    /// # Returns
    ///
    /// A copy of the immutable card metadata.
    pub const fn card_info(&self) -> EmmcCardInfo {
        self.card
    }

    fn note_media_removed(&self) {
        if self.media_online.swap(false, Ordering::AcqRel) {
            self.media_generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn command_address(&self, sector: u64) -> MmcResult<u32> {
        let address = if self.card.high_capacity {
            sector
        } else {
            sector
                .checked_mul(MMC_SECTOR_SIZE as u64)
                .ok_or(MmcError::OutOfRange)?
        };
        u32::try_from(address).map_err(|_| MmcError::OutOfRange)
    }

    fn process_request(&self, request: &mut BlockIORequest) -> Result<(), &'static str> {
        if request.sector_count == 0 {
            if matches!(request.request_type, BlockIORequestType::Read) {
                request.buffer.clear();
            }
            return Ok(());
        }

        let first_sector = request.sector as u64;
        let sector_count = request.sector_count as u64;
        if first_sector >= self.card.sector_count
            || sector_count > self.card.sector_count - first_sector
        {
            return Err(MmcError::OutOfRange.as_str());
        }
        let byte_count = request
            .sector_count
            .checked_mul(MMC_SECTOR_SIZE)
            .ok_or(MmcError::OutOfRange.as_str())?;

        match request.request_type {
            BlockIORequestType::Read => request.buffer.resize(byte_count, 0),
            BlockIORequestType::Write if request.buffer.len() < byte_count => {
                return Err(MmcError::InvalidArgument.as_str());
            }
            BlockIORequestType::Write => {}
        }

        let mut host = self.host.lock();
        if !host.card_present() {
            self.note_media_removed();
            return Err(MmcError::NoMedia.as_str());
        }
        if !self.media_online.load(Ordering::Acquire) {
            return Err(MmcError::MediaChanged.as_str());
        }

        for offset in 0..request.sector_count {
            let sector = first_sector
                .checked_add(offset as u64)
                .ok_or(MmcError::OutOfRange.as_str())?;
            let argument = self.command_address(sector).map_err(MmcError::as_str)?;
            let start = offset * MMC_SECTOR_SIZE;
            let end = start + MMC_SECTOR_SIZE;
            let result = match request.request_type {
                BlockIORequestType::Read => host.send_command(
                    MmcCommand::new(CMD_READ_SINGLE_BLOCK, argument, MmcResponseType::R1),
                    Some(MmcData::Read(&mut request.buffer[start..end])),
                ),
                BlockIORequestType::Write => host.send_command(
                    MmcCommand::new(CMD_WRITE_SINGLE_BLOCK, argument, MmcResponseType::R1),
                    Some(MmcData::Write(&request.buffer[start..end])),
                ),
            };
            result.map_err(MmcError::as_str)?;
        }
        Ok(())
    }
}

fn initialize_emmc(host: &mut dyn MmcHost, bus_width: MmcBusWidth) -> MmcResult<EmmcCardInfo> {
    if !host.card_present() {
        return Err(MmcError::NoMedia);
    }

    host.reset()?;
    host.set_bus_width(MmcBusWidth::One)?;
    host.set_clock(IDENTIFICATION_CLOCK_HZ)?;
    host.send_command(
        MmcCommand::new(CMD_GO_IDLE_STATE, 0, MmcResponseType::None),
        None,
    )?;

    let requested_ocr = OCR_SECTOR_MODE | OCR_VOLTAGE_WINDOW;
    let mut negotiated_ocr = 0u32;
    for _ in 0..OCR_POLL_ATTEMPTS {
        let response = host.send_command(
            MmcCommand::new(CMD_SEND_OP_COND, requested_ocr, MmcResponseType::R3),
            None,
        )?;
        negotiated_ocr = response.word(0);
        if negotiated_ocr & OCR_BUSY != 0 {
            break;
        }
        crate::time::udelay(1_000);
    }
    if negotiated_ocr & OCR_BUSY == 0 {
        return Err(MmcError::Timeout);
    }

    host.send_command(
        MmcCommand::new(CMD_ALL_SEND_CID, 0, MmcResponseType::R2),
        None,
    )?;
    let rca_argument = u32::from(MMC_RELATIVE_ADDRESS) << 16;
    host.send_command(
        MmcCommand::new(CMD_SET_RELATIVE_ADDR, rca_argument, MmcResponseType::R1),
        None,
    )?;
    host.send_command(
        MmcCommand::new(CMD_SEND_CSD, rca_argument, MmcResponseType::R2),
        None,
    )?;
    host.send_command(
        MmcCommand::new(CMD_SELECT_CARD, rca_argument, MmcResponseType::R1b),
        None,
    )?;

    let mut ext_csd = [0u8; MMC_SECTOR_SIZE];
    host.send_command(
        MmcCommand::new(CMD_SEND_EXT_CSD, 0, MmcResponseType::R1),
        Some(MmcData::Read(&mut ext_csd)),
    )?;
    let sector_count = u32::from_le_bytes(
        ext_csd[EXT_CSD_SECTOR_COUNT..EXT_CSD_SECTOR_COUNT + 4]
            .try_into()
            .map_err(|_| MmcError::Response)?,
    ) as u64;
    if sector_count == 0 {
        return Err(MmcError::Unsupported);
    }

    configure_emmc_bus_width(host, bus_width)?;

    let high_capacity = negotiated_ocr & OCR_SECTOR_MODE != 0;
    if !high_capacity {
        host.send_command(
            MmcCommand::new(
                CMD_SET_BLOCK_LENGTH,
                MMC_SECTOR_SIZE as u32,
                MmcResponseType::R1,
            ),
            None,
        )?;
    }
    host.set_clock(LEGACY_MMC_CLOCK_HZ)?;

    Ok(EmmcCardInfo {
        sector_count,
        high_capacity,
        ext_csd_revision: ext_csd[EXT_CSD_REVISION],
        device_type: ext_csd[EXT_CSD_DEVICE_TYPE],
    })
}

fn configure_emmc_bus_width(host: &mut dyn MmcHost, bus_width: MmcBusWidth) -> MmcResult<()> {
    if let Some(argument) = bus_width_switch_argument(bus_width) {
        let response = host.send_command(
            MmcCommand::new(CMD_SWITCH, argument, MmcResponseType::R1b),
            None,
        )?;
        if response.word(0) & (R1_STATUS_ERROR_MASK | R1_SWITCH_ERROR) != 0 {
            return Err(MmcError::Response);
        }
    }
    host.set_bus_width(bus_width)
}

const fn bus_width_switch_argument(bus_width: MmcBusWidth) -> Option<u32> {
    let value = match bus_width {
        MmcBusWidth::One => return None,
        MmcBusWidth::Four => EXT_CSD_BUS_WIDTH_4,
        MmcBusWidth::Eight => EXT_CSD_BUS_WIDTH_8,
    };
    Some(MMC_SWITCH_WRITE_BYTE | ((EXT_CSD_BUS_WIDTH as u32) << 16) | ((value as u32) << 8))
}

impl Device for EmmcBlockDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_block_device(&self) -> Option<&dyn BlockDevice> {
        Some(self)
    }

    fn into_block_device(self: Arc<Self>) -> Option<Arc<dyn BlockDevice>> {
        Some(self)
    }
}

impl BlockDevice for EmmcBlockDevice {
    fn get_disk_name(&self) -> &'static str {
        self.name
    }

    fn get_disk_size(&self) -> usize {
        self.card.disk_size()
    }

    fn get_sector_size(&self) -> usize {
        MMC_SECTOR_SIZE
    }

    fn enqueue_request(&self, request: Box<BlockIORequest>) {
        self.request_queue.lock().push(request);
    }

    fn process_requests(&self) -> Vec<BlockIOResult> {
        let requests = {
            let mut queue = self.request_queue.lock();
            core::mem::take(&mut *queue)
        };
        self.submit_requests(requests)
    }

    fn submit_requests(&self, requests: Vec<Box<BlockIORequest>>) -> Vec<BlockIOResult> {
        requests
            .into_iter()
            .map(|mut request| {
                let result = self.process_request(&mut request);
                BlockIOResult { request, result }
            })
            .collect()
    }
}

impl ControlOps for EmmcBlockDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported by eMMC")
    }
}

impl MemoryMappingOps for EmmcBlockDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported by eMMC")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for EmmcBlockDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use alloc::vec;

    struct MockHost {
        commands: Arc<IrqSpinLock<Vec<MmcCommand>>>,
        bus_widths: Arc<IrqSpinLock<Vec<MmcBusWidth>>>,
        storage: Arc<IrqSpinLock<Vec<u8>>>,
        sector_count: u32,
        present: Arc<AtomicBool>,
        switch_status: u32,
    }

    impl MockHost {
        fn new(sector_count: u32) -> Self {
            Self {
                commands: Arc::new(IrqSpinLock::new(Vec::new())),
                bus_widths: Arc::new(IrqSpinLock::new(Vec::new())),
                storage: Arc::new(IrqSpinLock::new(vec![
                    0;
                    sector_count as usize
                        * MMC_SECTOR_SIZE
                ])),
                sector_count,
                present: Arc::new(AtomicBool::new(true)),
                switch_status: 0,
            }
        }
    }

    impl MmcHost for MockHost {
        fn reset(&mut self) -> MmcResult<()> {
            Ok(())
        }

        fn set_clock(&mut self, _frequency_hz: u32) -> MmcResult<()> {
            Ok(())
        }

        fn set_bus_width(&mut self, width: MmcBusWidth) -> MmcResult<()> {
            self.bus_widths.lock().push(width);
            Ok(())
        }

        fn card_present(&self) -> bool {
            self.present.load(Ordering::Acquire)
        }

        fn is_removable(&self) -> bool {
            false
        }

        fn send_command(
            &mut self,
            command: MmcCommand,
            data: Option<MmcData<'_>>,
        ) -> MmcResult<crate::device::mmc::MmcResponse> {
            self.commands.lock().push(command);
            if command.index() == CMD_SEND_OP_COND {
                return Ok(crate::device::mmc::MmcResponse::new([
                    OCR_BUSY | OCR_SECTOR_MODE | OCR_VOLTAGE_WINDOW,
                    0,
                    0,
                    0,
                ]));
            }
            if command.index() == CMD_SWITCH {
                return Ok(crate::device::mmc::MmcResponse::new([
                    self.switch_status,
                    0,
                    0,
                    0,
                ]));
            }
            if command.index() == CMD_SEND_EXT_CSD {
                let Some(MmcData::Read(buffer)) = data else {
                    return Err(MmcError::InvalidArgument);
                };
                buffer.fill(0);
                buffer[EXT_CSD_REVISION] = 8;
                buffer[EXT_CSD_DEVICE_TYPE] = 1;
                buffer[EXT_CSD_SECTOR_COUNT..EXT_CSD_SECTOR_COUNT + 4]
                    .copy_from_slice(&self.sector_count.to_le_bytes());
            } else if command.index() == CMD_READ_SINGLE_BLOCK {
                let Some(MmcData::Read(buffer)) = data else {
                    return Err(MmcError::InvalidArgument);
                };
                let start = command.argument() as usize * MMC_SECTOR_SIZE;
                buffer.copy_from_slice(&self.storage.lock()[start..start + MMC_SECTOR_SIZE]);
            } else if command.index() == CMD_WRITE_SINGLE_BLOCK {
                let Some(MmcData::Write(buffer)) = data else {
                    return Err(MmcError::InvalidArgument);
                };
                let start = command.argument() as usize * MMC_SECTOR_SIZE;
                self.storage.lock()[start..start + MMC_SECTOR_SIZE].copy_from_slice(buffer);
            }
            Ok(crate::device::mmc::MmcResponse::default())
        }
    }

    #[test_case]
    fn initializes_emmc_through_host_independent_sequence() {
        let host = MockHost::new(32);
        let commands = host.commands.clone();
        let device = EmmcBlockDevice::probe("mmcblk-test", Box::new(host)).unwrap();

        assert_eq!(device.get_disk_size(), 32 * MMC_SECTOR_SIZE);
        let command_indices: Vec<u8> = commands
            .lock()
            .iter()
            .map(|command| command.index())
            .collect();
        assert_eq!(
            command_indices.as_slice(),
            &[
                CMD_GO_IDLE_STATE,
                CMD_SEND_OP_COND,
                CMD_ALL_SEND_CID,
                CMD_SET_RELATIVE_ADDR,
                CMD_SEND_CSD,
                CMD_SELECT_CARD,
                CMD_SEND_EXT_CSD,
            ]
        );
    }

    #[test_case]
    fn probe_with_eight_bit_bus_switches_card_before_host_width() {
        let host = MockHost::new(32);
        let commands = host.commands.clone();
        let bus_widths = host.bus_widths.clone();
        let _device = EmmcBlockDevice::probe_with_bus_width(
            "mmcblk-test",
            Box::new(host),
            MmcBusWidth::Eight,
        )
        .unwrap();

        let commands = commands.lock();
        let switch = commands.last().unwrap();
        assert_eq!(switch.index(), CMD_SWITCH);
        assert_eq!(switch.response(), MmcResponseType::R1b);
        assert_eq!(
            switch.argument(),
            MMC_SWITCH_WRITE_BYTE
                | (u32::from(EXT_CSD_BUS_WIDTH) << 16)
                | (u32::from(EXT_CSD_BUS_WIDTH_8) << 8)
        );
        assert_eq!(
            bus_widths.lock().as_slice(),
            &[MmcBusWidth::One, MmcBusWidth::Eight]
        );
    }

    #[test_case]
    fn four_bit_bus_uses_ext_csd_bus_width_value() {
        assert_eq!(
            bus_width_switch_argument(MmcBusWidth::Four),
            Some(
                MMC_SWITCH_WRITE_BYTE
                    | (u32::from(EXT_CSD_BUS_WIDTH) << 16)
                    | (u32::from(EXT_CSD_BUS_WIDTH_4) << 8)
            )
        );
        assert_eq!(bus_width_switch_argument(MmcBusWidth::One), None);
    }

    #[test_case]
    fn rejected_bus_width_switch_keeps_the_host_in_one_bit_mode() {
        let mut host = MockHost::new(32);
        host.switch_status = R1_SWITCH_ERROR;
        let bus_widths = host.bus_widths.clone();

        let result = EmmcBlockDevice::probe_with_bus_width(
            "mmcblk-test",
            Box::new(host),
            MmcBusWidth::Eight,
        );

        assert!(matches!(result, Err(MmcError::Response)));
        assert_eq!(bus_widths.lock().as_slice(), &[MmcBusWidth::One]);
    }

    #[test_case]
    fn block_adapter_round_trips_one_sector() {
        let host = MockHost::new(32);
        let device = EmmcBlockDevice::probe("mmcblk-test", Box::new(host)).unwrap();
        let pattern = vec![0x5a; MMC_SECTOR_SIZE];
        let write = Box::new(BlockIORequest {
            request_type: BlockIORequestType::Write,
            sector: 3,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: pattern.clone(),
        });
        assert!(device.submit_requests(vec![write])[0].result.is_ok());

        let read = Box::new(BlockIORequest {
            request_type: BlockIORequestType::Read,
            sector: 3,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: Vec::new(),
        });
        let mut results = device.submit_requests(vec![read]);
        let result = results.pop().unwrap();
        assert!(result.result.is_ok());
        assert_eq!(result.request.buffer, pattern);
    }

    #[test_case]
    fn removed_media_invalidates_the_existing_block_endpoint() {
        let host = MockHost::new(32);
        let present = host.present.clone();
        let device = EmmcBlockDevice::probe("mmcblk-test", Box::new(host)).unwrap();
        assert_eq!(device.media_generation(), 1);

        present.store(false, Ordering::Release);
        let removed_read = Box::new(BlockIORequest {
            request_type: BlockIORequestType::Read,
            sector: 0,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: Vec::new(),
        });
        let removed_result = device.submit_requests(vec![removed_read]).pop().unwrap();
        assert_eq!(removed_result.result, Err(MmcError::NoMedia.as_str()));
        assert_eq!(device.media_generation(), 2);

        present.store(true, Ordering::Release);
        let stale_read = Box::new(BlockIORequest {
            request_type: BlockIORequestType::Read,
            sector: 0,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: Vec::new(),
        });
        let stale_result = device.submit_requests(vec![stale_read]).pop().unwrap();
        assert_eq!(stale_result.result, Err(MmcError::MediaChanged.as_str()));
        assert_eq!(device.media_generation(), 2);
    }
}
