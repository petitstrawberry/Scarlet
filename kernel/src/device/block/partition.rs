//! Generic block-device partition scanning.
//!
//! This module detects partition tables on block devices and registers
//! partition block devices as offset-limited wrappers over the parent device.

use core::any::Any;
use core::fmt;

use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    sync::Arc,
    vec,
    vec::Vec,
};
use crate::sync::Mutex;

use super::{
    BlockDevice,
    request::{BlockIORequest, BlockIORequestType, BlockIOResult},
};
use crate::device::manager::DeviceManager;
use crate::device::{Device, DeviceType};
use crate::object::capability::selectable::Selectable;
use crate::object::capability::{ControlOps, MemoryMappingOps};

const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
const GPT_HEADER_LBA: u64 = 1;
const GPT_MIN_HEADER_SIZE: usize = 92;
const GPT_MIN_ENTRY_SIZE: usize = 128;
const GPT_PARTITION_NAME_BYTES: usize = 72;
const PROTECTIVE_MBR_PARTITION_TYPE: u8 = 0xee;
const MBR_PARTITION_TABLE_OFFSET: usize = 446;
const MBR_PARTITION_ENTRY_SIZE: usize = 16;
const MBR_PARTITION_ENTRY_COUNT: usize = 4;
const MBR_SIGNATURE_OFFSET: usize = 510;

/// Block device representing one partition on a parent block device.
#[allow(clippy::vec_box)]
pub struct PartitionBlockDevice {
    name: &'static str,
    parent: Arc<dyn BlockDevice>,
    first_lba: u64,
    lba_count: u64,
    sector_size: usize,
    request_queue: Mutex<Vec<Box<BlockIORequest>>>,
}

impl PartitionBlockDevice {
    /// Create a partition block device.
    ///
    /// # Arguments
    ///
    /// * `name` - Device name to expose through `DeviceManager`.
    /// * `parent` - Parent block device that owns the underlying storage.
    /// * `first_lba` - First parent LBA included in this partition.
    /// * `lba_count` - Number of LBAs included in this partition.
    /// * `sector_size` - Logical sector size in bytes.
    ///
    /// # Returns
    ///
    /// A partition block device.
    pub fn new(
        name: String,
        parent: Arc<dyn BlockDevice>,
        first_lba: u64,
        lba_count: u64,
        sector_size: usize,
    ) -> Self {
        let name = Box::leak(name.into_boxed_str());
        Self {
            name,
            parent,
            first_lba,
            lba_count,
            sector_size,
            request_queue: Mutex::new(Vec::new()),
        }
    }

    fn request_is_in_range(&self, request: &BlockIORequest) -> bool {
        if request.sector_count == 0 {
            return true;
        }

        let sector = request.sector as u64;
        let sector_count = request.sector_count as u64;
        sector < self.lba_count && sector_count <= self.lba_count - sector
    }
}

impl Device for PartitionBlockDevice {
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

impl BlockDevice for PartitionBlockDevice {
    fn get_disk_name(&self) -> &'static str {
        self.name
    }

    fn get_disk_size(&self) -> usize {
        let bytes = self.lba_count.saturating_mul(self.sector_size as u64);
        bytes.min(usize::MAX as u64) as usize
    }

    fn get_sector_size(&self) -> usize {
        self.sector_size
    }

    fn enqueue_request(&self, request: Box<BlockIORequest>) {
        self.request_queue.lock().push(request);
    }

    fn process_requests(&self) -> Vec<BlockIOResult> {
        let requests = {
            let mut queue = self.request_queue.lock();
            core::mem::take(&mut *queue)
        };

        let mut results = Vec::new();
        for mut request in requests {
            if request.sector_count == 0 {
                if matches!(request.request_type, BlockIORequestType::Read) {
                    request.buffer.clear();
                }
                results.push(BlockIOResult {
                    request,
                    result: Ok(()),
                });
                continue;
            }

            if !self.request_is_in_range(&request) {
                results.push(BlockIOResult {
                    request,
                    result: Err("Partition request out of range"),
                });
                continue;
            }

            let parent_sector = match self.first_lba.checked_add(request.sector as u64) {
                Some(sector) if sector <= usize::MAX as u64 => sector as usize,
                _ => {
                    results.push(BlockIOResult {
                        request,
                        result: Err("Partition parent sector overflow"),
                    });
                    continue;
                }
            };

            let parent_request = Box::new(BlockIORequest {
                request_type: request.request_type,
                sector: parent_sector,
                sector_count: request.sector_count,
                head: request.head,
                cylinder: request.cylinder,
                buffer: request.buffer.clone(),
            });

            self.parent.enqueue_request(parent_request);
            let mut parent_results = self.parent.process_requests();
            if parent_results.is_empty() {
                results.push(BlockIOResult {
                    request,
                    result: Err("No result from parent block device"),
                });
                continue;
            }

            let BlockIOResult {
                request: parent_request,
                result,
            } = parent_results.remove(0);
            let parent_request = *parent_request;
            if result.is_ok() && matches!(request.request_type, BlockIORequestType::Read) {
                request.buffer = parent_request.buffer;
            }

            results.push(BlockIOResult { request, result });
        }

        results
    }
}

impl ControlOps for PartitionBlockDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for PartitionBlockDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported by partition block device")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for PartitionBlockDevice {
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

/// Return whether a device is a partition wrapper.
///
/// # Arguments
///
/// * `device` - Device to inspect.
///
/// # Returns
///
/// `true` if the device is a `PartitionBlockDevice`.
pub fn is_partition_device(device: &dyn Device) -> bool {
    device.as_any().is::<PartitionBlockDevice>()
}

/// Scan a block device for partitions and register discovered partitions.
///
/// # Arguments
///
/// * `parent_name` - Registered name of the parent block device.
/// * `parent` - Parent block device to scan.
/// * `manager` - Device manager used to register partition devices.
///
/// # Returns
///
/// Number of partition devices registered.
pub fn scan_and_register_partitions(
    parent_name: &str,
    parent: Arc<dyn BlockDevice>,
    manager: &DeviceManager,
) -> Result<usize, &'static str> {
    let sector_size = parent.get_sector_size();
    if sector_size != 512 {
        crate::early_println!(
            "[partition] Skipping {}: unsupported logical sector size {}",
            parent_name,
            sector_size
        );
        return Ok(0);
    }

    let total_lbas = (parent.get_disk_size() / sector_size) as u64;
    if total_lbas <= GPT_HEADER_LBA {
        return Ok(0);
    }

    scan_gpt(parent_name, parent, manager, sector_size, total_lbas)
}

fn scan_gpt(
    parent_name: &str,
    parent: Arc<dyn BlockDevice>,
    manager: &DeviceManager,
    sector_size: usize,
    total_lbas: u64,
) -> Result<usize, &'static str> {
    let mbr = read_lbas(parent.as_ref(), 0, 1, sector_size)?;
    let header_sector = read_lbas(parent.as_ref(), GPT_HEADER_LBA, 1, sector_size)?;

    if header_sector.get(0..GPT_SIGNATURE.len()) != Some(GPT_SIGNATURE) {
        return Ok(0);
    }

    if !has_protective_mbr(&mbr) {
        crate::early_println!(
            "[partition] {} has GPT signature without protective MBR",
            parent_name
        );
    }

    let header = GptHeader::parse(&header_sector, sector_size, total_lbas)?;
    let entry_array_bytes =
        checked_entry_array_len(header.num_partition_entries, header.size_of_partition_entry)?;
    let entry_array_sectors = entry_array_bytes.div_ceil(sector_size);
    if entry_array_sectors == 0 {
        return Ok(0);
    }

    let entry_array_end = header
        .partition_entry_lba
        .checked_add(entry_array_sectors as u64)
        .ok_or("GPT partition entry array LBA overflow")?;
    if entry_array_end > total_lbas {
        return Err("GPT partition entry array outside parent device");
    }

    let entries = read_lbas(
        parent.as_ref(),
        header.partition_entry_lba,
        entry_array_sectors,
        sector_size,
    )?;
    let entry_array = entries
        .get(..entry_array_bytes)
        .ok_or("GPT partition entry array read too short")?;
    let entries_crc = crc32(entry_array);
    if entries_crc != header.partition_entry_array_crc32 {
        return Err("GPT partition entry array CRC mismatch");
    }

    let mut registered = 0usize;
    for index in 0..header.num_partition_entries as usize {
        let offset = index
            .checked_mul(header.size_of_partition_entry as usize)
            .ok_or("GPT partition entry offset overflow")?;
        let end = offset
            .checked_add(header.size_of_partition_entry as usize)
            .ok_or("GPT partition entry end overflow")?;
        let Some(entry_bytes) = entry_array.get(offset..end) else {
            break;
        };

        let Some(entry) = GptPartitionEntry::parse(entry_bytes)? else {
            continue;
        };

        if !entry.is_inside_parent(&header, total_lbas) {
            crate::early_println!(
                "[partition] Skipping {}p{}: invalid GPT range first_lba={} last_lba={}",
                parent_name,
                index + 1,
                entry.first_lba,
                entry.last_lba
            );
            continue;
        }

        let lba_count = entry
            .last_lba
            .checked_sub(entry.first_lba)
            .and_then(|last_offset| last_offset.checked_add(1))
            .ok_or("GPT partition size overflow")?;
        let partition_name = format!("{}p{}", parent_name, index + 1);
        let size_bytes = lba_count.saturating_mul(sector_size as u64);
        crate::early_println!(
            "[partition] {}: type={} unique={} name=\"{}\" first_lba={} last_lba={} size={} bytes",
            partition_name,
            entry.partition_type_guid,
            entry.unique_partition_guid,
            entry.name,
            entry.first_lba,
            entry.last_lba,
            size_bytes
        );

        let partition = Arc::new(PartitionBlockDevice::new(
            partition_name.clone(),
            parent.clone(),
            entry.first_lba,
            lba_count,
            sector_size,
        ));
        manager.register_device_with_name(partition_name, partition);
        registered += 1;
    }

    Ok(registered)
}

fn read_lbas(
    device: &dyn BlockDevice,
    lba: u64,
    sector_count: usize,
    sector_size: usize,
) -> Result<Vec<u8>, &'static str> {
    let byte_len = sector_count
        .checked_mul(sector_size)
        .ok_or("Block read length overflow")?;
    let mut buffer = Vec::with_capacity(byte_len);

    for sector_offset in 0..sector_count {
        let sector = lba
            .checked_add(sector_offset as u64)
            .ok_or("Block read LBA overflow")?;
        if sector > usize::MAX as u64 {
            return Err("Block read LBA overflow");
        }

        let request = Box::new(BlockIORequest {
            request_type: BlockIORequestType::Read,
            sector: sector as usize,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: vec![0; sector_size],
        });

        device.enqueue_request(request);
        let results = device.process_requests();
        let Some(result) = results.into_iter().next() else {
            return Err("No result from block device read");
        };
        if result.result.is_err() {
            return Err("Block device read failed");
        }
        if result.request.buffer.len() < sector_size {
            return Err("Block device read returned a short sector");
        }

        buffer.extend_from_slice(&result.request.buffer[..sector_size]);
    }

    Ok(buffer)
}

fn has_protective_mbr(mbr: &[u8]) -> bool {
    if mbr.get(MBR_SIGNATURE_OFFSET) != Some(&0x55)
        || mbr.get(MBR_SIGNATURE_OFFSET + 1) != Some(&0xaa)
    {
        return false;
    }

    for index in 0..MBR_PARTITION_ENTRY_COUNT {
        let offset = MBR_PARTITION_TABLE_OFFSET + index * MBR_PARTITION_ENTRY_SIZE;
        if mbr.get(offset + 4) == Some(&PROTECTIVE_MBR_PARTITION_TYPE) {
            return true;
        }
    }

    false
}

fn checked_entry_array_len(num_entries: u32, entry_size: u32) -> Result<usize, &'static str> {
    let bytes = (num_entries as u64)
        .checked_mul(entry_size as u64)
        .ok_or("GPT partition entry array size overflow")?;
    if bytes > usize::MAX as u64 {
        return Err("GPT partition entry array too large");
    }
    Ok(bytes as usize)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid([u8; 16]);

impl Guid {
    fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        let guid = bytes.get(..16).ok_or("GUID field truncated")?;
        let mut data = [0u8; 16];
        data.copy_from_slice(guid);
        Ok(Self(data))
    }

    fn is_zero(self) -> bool {
        self.0.iter().all(|byte| *byte == 0)
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d1 = u32::from_le_bytes([self.0[0], self.0[1], self.0[2], self.0[3]]);
        let d2 = u16::from_le_bytes([self.0[4], self.0[5]]);
        let d3 = u16::from_le_bytes([self.0[6], self.0[7]]);
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            d1,
            d2,
            d3,
            self.0[8],
            self.0[9],
            self.0[10],
            self.0[11],
            self.0[12],
            self.0[13],
            self.0[14],
            self.0[15]
        )
    }
}

struct GptHeader {
    first_usable_lba: u64,
    last_usable_lba: u64,
    partition_entry_lba: u64,
    num_partition_entries: u32,
    size_of_partition_entry: u32,
    partition_entry_array_crc32: u32,
}

impl GptHeader {
    fn parse(sector: &[u8], sector_size: usize, total_lbas: u64) -> Result<Self, &'static str> {
        if sector.get(0..GPT_SIGNATURE.len()) != Some(GPT_SIGNATURE) {
            return Err("Invalid GPT signature");
        }

        let header_size = read_le_u32(sector, 12)? as usize;
        if !(GPT_MIN_HEADER_SIZE..=sector_size).contains(&header_size) {
            return Err("Invalid GPT header size");
        }

        let stored_header_crc = read_le_u32(sector, 16)?;
        let mut header_for_crc = sector
            .get(..header_size)
            .ok_or("GPT header truncated")?
            .to_vec();
        header_for_crc[16..20].fill(0);
        if crc32(&header_for_crc) != stored_header_crc {
            return Err("GPT header CRC mismatch");
        }

        let current_lba = read_le_u64(sector, 24)?;
        if current_lba != GPT_HEADER_LBA {
            return Err("GPT header is not at primary LBA");
        }

        let first_usable_lba = read_le_u64(sector, 40)?;
        let last_usable_lba = read_le_u64(sector, 48)?;
        if first_usable_lba > last_usable_lba || last_usable_lba >= total_lbas {
            return Err("GPT usable LBA range outside parent device");
        }

        let partition_entry_lba = read_le_u64(sector, 72)?;
        let num_partition_entries = read_le_u32(sector, 80)?;
        let size_of_partition_entry = read_le_u32(sector, 84)?;
        if size_of_partition_entry < GPT_MIN_ENTRY_SIZE as u32 {
            return Err("GPT partition entry size too small");
        }

        Ok(Self {
            first_usable_lba,
            last_usable_lba,
            partition_entry_lba,
            num_partition_entries,
            size_of_partition_entry,
            partition_entry_array_crc32: read_le_u32(sector, 88)?,
        })
    }
}

struct GptPartitionEntry {
    partition_type_guid: Guid,
    unique_partition_guid: Guid,
    first_lba: u64,
    last_lba: u64,
    name: String,
}

impl GptPartitionEntry {
    fn parse(bytes: &[u8]) -> Result<Option<Self>, &'static str> {
        let partition_type_guid = Guid::from_bytes(bytes.get(..16).ok_or("GPT entry truncated")?)?;
        if partition_type_guid.is_zero() {
            return Ok(None);
        }

        let unique_partition_guid =
            Guid::from_bytes(bytes.get(16..32).ok_or("GPT entry truncated")?)?;
        let first_lba = read_le_u64(bytes, 32)?;
        let last_lba = read_le_u64(bytes, 40)?;
        let name = decode_gpt_name(bytes.get(56..128).ok_or("GPT entry name truncated")?);

        Ok(Some(Self {
            partition_type_guid,
            unique_partition_guid,
            first_lba,
            last_lba,
            name,
        }))
    }

    fn is_inside_parent(&self, header: &GptHeader, total_lbas: u64) -> bool {
        self.first_lba <= self.last_lba
            && self.first_lba >= header.first_usable_lba
            && self.last_lba <= header.last_usable_lba
            && self.last_lba < total_lbas
    }
}

fn decode_gpt_name(bytes: &[u8]) -> String {
    let mut name = String::new();
    for index in 0..(GPT_PARTITION_NAME_BYTES / 2) {
        let offset = index * 2;
        let Some(unit_bytes) = bytes.get(offset..offset + 2) else {
            break;
        };
        let unit = u16::from_le_bytes([unit_bytes[0], unit_bytes[1]]);
        if unit == 0 {
            break;
        }

        if (0x20..=0x7e).contains(&unit) {
            name.push(unit as u8 as char);
        } else {
            name.push('?');
        }
    }

    if name.is_empty() {
        "(unnamed)".to_string()
    } else {
        name
    }
}

fn read_le_u32(bytes: &[u8], offset: usize) -> Result<u32, &'static str> {
    let value = bytes.get(offset..offset + 4).ok_or("u32 field truncated")?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_le_u64(bytes: &[u8], offset: usize) -> Result<u64, &'static str> {
    let value = bytes.get(offset..offset + 8).ok_or("u64 field truncated")?;
    Ok(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use alloc::vec;

    use super::*;
    use crate::device::block::mockblk::MockBlockDevice;

    const TEST_SECTORS: usize = 128;

    fn write_sector(device: &Arc<MockBlockDevice>, sector: usize, data: &[u8]) {
        let mut buffer = vec![0; 512];
        buffer[..data.len()].copy_from_slice(data);
        device.enqueue_request(Box::new(BlockIORequest {
            request_type: BlockIORequestType::Write,
            sector,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer,
        }));
        let results = device.process_requests();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].result, Ok(()));
    }

    fn read_sector(device: &Arc<MockBlockDevice>, sector: usize) -> Vec<u8> {
        device.enqueue_request(Box::new(BlockIORequest {
            request_type: BlockIORequestType::Read,
            sector,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: vec![0; 512],
        }));
        let results = device.process_requests();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].result, Ok(()));
        results[0].request.buffer.clone()
    }

    fn put_le_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_le_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn write_test_gpt(device: &Arc<MockBlockDevice>) {
        let mut mbr = vec![0; 512];
        mbr[MBR_PARTITION_TABLE_OFFSET + 4] = PROTECTIVE_MBR_PARTITION_TYPE;
        put_le_u32(&mut mbr, MBR_PARTITION_TABLE_OFFSET + 8, 1);
        put_le_u32(
            &mut mbr,
            MBR_PARTITION_TABLE_OFFSET + 12,
            (TEST_SECTORS - 1) as u32,
        );
        mbr[MBR_SIGNATURE_OFFSET] = 0x55;
        mbr[MBR_SIGNATURE_OFFSET + 1] = 0xaa;
        write_sector(device, 0, &mbr);

        let mut entries = vec![0; 128 * 128];
        entries[0..16].copy_from_slice(&[
            0xaf, 0x3d, 0xc6, 0x0f, 0x83, 0x84, 0x72, 0x47, 0x8e, 0x79, 0x3d, 0x69, 0xd8, 0x47,
            0x7d, 0xe4,
        ]);
        entries[16..32].copy_from_slice(&[
            0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef,
        ]);
        put_le_u64(&mut entries, 32, 34);
        put_le_u64(&mut entries, 40, 49);
        for (index, unit) in ['r' as u16, 'o' as u16, 'o' as u16, 't' as u16]
            .iter()
            .enumerate()
        {
            entries[56 + index * 2..58 + index * 2].copy_from_slice(&unit.to_le_bytes());
        }

        let entries_crc = crc32(&entries);
        for (index, chunk) in entries.chunks(512).enumerate() {
            write_sector(device, 2 + index, chunk);
        }

        let mut header = vec![0; 512];
        header[0..8].copy_from_slice(GPT_SIGNATURE);
        put_le_u32(&mut header, 8, 0x0001_0000);
        put_le_u32(&mut header, 12, GPT_MIN_HEADER_SIZE as u32);
        put_le_u64(&mut header, 24, 1);
        put_le_u64(&mut header, 32, (TEST_SECTORS - 1) as u64);
        put_le_u64(&mut header, 40, 34);
        put_le_u64(&mut header, 48, 120);
        header[56..72].copy_from_slice(&[
            0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
            0x88, 0x99,
        ]);
        put_le_u64(&mut header, 72, 2);
        put_le_u32(&mut header, 80, 128);
        put_le_u32(&mut header, 84, 128);
        put_le_u32(&mut header, 88, entries_crc);
        let header_crc = crc32(&header[..GPT_MIN_HEADER_SIZE]);
        put_le_u32(&mut header, 16, header_crc);
        write_sector(device, 1, &header);
    }

    #[test_case]
    fn test_scan_gpt_registers_partition_device() {
        let device = Arc::new(MockBlockDevice::new("test_disk", 512, TEST_SECTORS));
        write_test_gpt(&device);

        let manager = DeviceManager::new_for_test();
        let parent: Arc<dyn BlockDevice> = device.clone();
        let registered = scan_and_register_partitions("vblk0", parent, &manager).unwrap();
        assert_eq!(registered, 1);

        let partition = manager
            .get_device_by_name("vblk0p1")
            .expect("partition device should be registered")
            .into_block_device()
            .expect("partition device should be a block device");
        assert_eq!(partition.get_disk_name(), "vblk0p1");
        assert_eq!(partition.get_disk_size(), 16 * 512);
    }

    #[test_case]
    fn test_partition_device_offsets_io_to_parent() {
        let device = Arc::new(MockBlockDevice::new("test_disk", 512, TEST_SECTORS));
        let partition =
            PartitionBlockDevice::new("vblk0p1".to_string(), device.clone(), 34, 16, 512);

        partition.enqueue_request(Box::new(BlockIORequest {
            request_type: BlockIORequestType::Write,
            sector: 0,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: vec![0xab; 512],
        }));
        let results = partition.process_requests();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].result, Ok(()));

        let parent_sector = read_sector(&device, 34);
        assert_eq!(parent_sector, vec![0xab; 512]);
    }
}
