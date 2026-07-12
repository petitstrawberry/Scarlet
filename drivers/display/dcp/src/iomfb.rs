//! Apple DCP IOMFB shared-memory RPC transport.

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::mem;

use scarlet::device::iommu::{IommuDomain, IommuMapFlags};
use scarlet::device::remoteproc::RemoteProcessor;
use scarlet::mem::page::ContiguousPages;
use scarlet::println;
use scarlet::time;
use scarlet_driver_apple_dart::DartDomain;
use scarlet_driver_apple_rtkit::{AppleRtkit, RtkitMessage};

const ENDPOINT: u8 = 0x37;
const SHMEM_SIZE: usize = 0x10_0000;
const PACKET_ALIGNMENT: usize = 0x40;
const REPLY_TIMEOUT_US: u64 = 5_000_000;

const MESSAGE_TYPE_MASK: u64 = 0xf;
const MESSAGE_TYPE_SET_SHMEM: u64 = 0;
const MESSAGE_TYPE_INITIALIZED: u64 = 1;
const MESSAGE_TYPE_RPC: u64 = 2;
const MESSAGE_CONTEXT_MASK: u64 = 0xf << 8;
const MESSAGE_ACK: u64 = 1 << 6;
const MESSAGE_OFFSET_MASK: u64 = 0xffff << 16;
const MESSAGE_LENGTH_MASK: u64 = 0xffff_ffff << 32;
const SHMEM_FLAG: u64 = 4 << 4;

const CONTEXT_CALLBACK: u8 = 0;
const CONTEXT_COMMAND: u8 = 2;
const CONTEXT_ASYNC: u8 = 3;
const CONTEXT_OOB_CALLBACK: u8 = 4;
const CONTEXT_OOB_COMMAND: u8 = 6;
const CONTEXT_OOB_ASYNC: u8 = 7;
const CALLBACK_OFFSET: usize = 0x60000;
const SWAP_SUBMIT_SIZE_V12_3: usize = 2916;
const SWAP_SUBMIT_SIZE_V13_5: usize = 6276;
const SWAP_SURFACE_SIZE_V12_3: usize = 516;
const SWAP_SURFACE_SIZE_V13_5: usize = 556;
const SWAP_SURFACES_OFFSET_V12_3: usize = 800;
const SWAP_SURFACES_OFFSET_V13_5: usize = 1128;
const SWAP_SURFACE_IOVA_OFFSET_V12_3: usize = 2864;
const SWAP_SURFACE_IOVA_OFFSET_V13_5: usize = 3352;
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct PacketHeader {
    tag: [u8; 4],
    input_len: u32,
    output_len: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct IoUserClient {
    handle: u64,
    unknown: u32,
    flag1: u8,
    flag2: u8,
    padding: [u8; 2],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct SwapStartRequest {
    swap_id: u32,
    client: IoUserClient,
    swap_id_null: u8,
    client_null: u8,
    padding: [u8; 2],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct SwapStartResponse {
    swap_id: u32,
    client: IoUserClient,
    result: u32,
}

struct Allocation {
    pages: ContiguousPages,
    dva: u64,
    size: usize,
    id: u32,
    piodma_mapped: bool,
}

struct PhysicalMapping {
    dva: u64,
    size: usize,
    id: u32,
}

/// Physical bandwidth-control registers returned by the D003 callback.
#[derive(Clone, Copy)]
pub struct BandwidthRegisters {
    /// PMGR bandwidth scratch register address.
    pub scratch: u64,
    /// PMGR bandwidth doorbell register base.
    pub doorbell: u64,
    /// Doorbell bit assigned to this DCP instance.
    pub doorbell_bit: u32,
}

fn bytes_of<T>(value: &T) -> &[u8] {
    // SAFETY: wire values are packed POD and the slice does not outlive `value`.
    unsafe { core::slice::from_raw_parts(value as *const T as *const u8, mem::size_of::<T>()) }
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// Runtime IOMFB transport used for atomic DCP swaps.
pub struct Iomfb {
    rtkit: Arc<AppleRtkit>,
    shmem: ContiguousPages,
    shmem_dva: u64,
    last_completed_swap: u32,
    callback_ends: Vec<(u8, usize)>,
    registers: Vec<(usize, usize)>,
    allocations: Vec<Allocation>,
    physical_mappings: Vec<PhysicalMapping>,
    next_descriptor_id: u32,
    bandwidth: Option<BandwidthRegisters>,
    clock_frequency: u64,
    piodma_domain: Arc<DartDomain>,
    main_display: bool,
    firmware_12_3: bool,
}

impl Iomfb {
    /// Start endpoint 0x37 and install the AP-owned shared-memory region.
    ///
    /// # Arguments
    ///
    /// * `rtkit` - Running DCP RTKit instance.
    ///
    /// # Returns
    ///
    /// An initialized transport, or a protocol/transport error.
    pub fn new(
        rtkit: Arc<AppleRtkit>,
        registers: Vec<(usize, usize)>,
        bandwidth: Option<BandwidthRegisters>,
        clock_frequency: u64,
        piodma_domain: Arc<DartDomain>,
        firmware_12_3: bool,
    ) -> Result<Self, &'static str> {
        let pages = SHMEM_SIZE.div_ceil(scarlet::environment::PAGE_SIZE);
        let shmem = ContiguousPages::new_aligned(pages, rtkit.dma_alignment())
            .ok_or("apple-dcp: IOMFB shmem allocation failed")?;
        // SAFETY: the live contiguous allocation covers exactly `SHMEM_SIZE` bytes.
        unsafe { core::ptr::write_bytes(shmem.as_ptr() as *mut u8, 0, SHMEM_SIZE) };
        scarlet::arch::clean_dcache_to_poc_range(shmem.as_ptr() as usize, SHMEM_SIZE);
        let shmem_dva = rtkit
            .map_dma(shmem.as_paddr(), SHMEM_SIZE)
            .map_err(|_| "apple-dcp: IOMFB shmem mapping failed")?;

        rtkit.start_ep(ENDPOINT)?;
        rtkit.send(&RtkitMessage {
            ep: ENDPOINT,
            msg: MESSAGE_TYPE_SET_SHMEM | SHMEM_FLAG | (shmem_dva << 16),
        })?;

        let mut transport = Self {
            rtkit,
            shmem,
            shmem_dva,
            last_completed_swap: 0,
            callback_ends: Vec::new(),
            registers,
            allocations: Vec::new(),
            physical_mappings: Vec::new(),
            next_descriptor_id: 1,
            bandwidth,
            clock_frequency,
            piodma_domain,
            main_display: true,
            firmware_12_3,
        };
        transport.wait_initialized()?;
        Ok(transport)
    }

    fn wait_initialized(&mut self) -> Result<(), &'static str> {
        let start = time::current_time();
        loop {
            let elapsed = time::current_time().saturating_sub(start);
            if elapsed >= REPLY_TIMEOUT_US {
                return Err("apple-dcp: timeout waiting for IOMFB initialization");
            }
            let message = self.recv_timeout(REPLY_TIMEOUT_US - elapsed)?;
            if message & MESSAGE_TYPE_MASK == MESSAGE_TYPE_INITIALIZED {
                return Ok(());
            }
        }
    }

    fn recv_timeout(&self, timeout_us: u64) -> Result<u64, &'static str> {
        let mut message = RtkitMessage { ep: 0, msg: 0 };
        self.rtkit
            .recv_endpoint_timeout(ENDPOINT, &mut message, timeout_us)?;
        Ok(message.msg)
    }

    fn shared_slice(&self, offset: usize, length: usize) -> Result<&[u8], &'static str> {
        let end = offset
            .checked_add(length)
            .ok_or("apple-dcp: IOMFB shmem range overflow")?;
        if end > SHMEM_SIZE {
            return Err("apple-dcp: IOMFB shmem range out of bounds");
        }
        // SAFETY: the range is checked against the live shared-memory allocation.
        Ok(unsafe {
            core::slice::from_raw_parts((self.shmem.as_ptr() as *const u8).add(offset), length)
        })
    }

    fn shared_slice_mut(
        &mut self,
        offset: usize,
        length: usize,
    ) -> Result<&mut [u8], &'static str> {
        let end = offset
            .checked_add(length)
            .ok_or("apple-dcp: IOMFB shmem range overflow")?;
        if end > SHMEM_SIZE {
            return Err("apple-dcp: IOMFB shmem range out of bounds");
        }
        // SAFETY: the range is checked and the mutable borrow excludes other AP access.
        Ok(unsafe {
            core::slice::from_raw_parts_mut((self.shmem.as_ptr() as *mut u8).add(offset), length)
        })
    }

    fn rpc_message(context: u8, length: usize, offset: usize, ack: bool) -> u64 {
        MESSAGE_TYPE_RPC
            | ((context as u64) << MESSAGE_CONTEXT_MASK.trailing_zeros())
            | ((offset as u64) << MESSAGE_OFFSET_MASK.trailing_zeros())
            | ((length as u64) << MESSAGE_LENGTH_MASK.trailing_zeros())
            | if ack { MESSAGE_ACK } else { 0 }
    }

    fn parse_rpc(message: u64) -> (u8, usize, usize, bool) {
        (
            ((message & MESSAGE_CONTEXT_MASK) >> MESSAGE_CONTEXT_MASK.trailing_zeros()) as u8,
            ((message & MESSAGE_OFFSET_MASK) >> MESSAGE_OFFSET_MASK.trailing_zeros()) as usize,
            ((message & MESSAGE_LENGTH_MASK) >> MESSAGE_LENGTH_MASK.trailing_zeros()) as usize,
            message & MESSAGE_ACK != 0,
        )
    }

    fn channel_offset(context: u8) -> Result<usize, &'static str> {
        match context {
            CONTEXT_CALLBACK => Ok(CALLBACK_OFFSET),
            CONTEXT_ASYNC => Ok(0x40000),
            CONTEXT_OOB_CALLBACK => Ok(0x68000),
            CONTEXT_OOB_ASYNC => Ok(0x48000),
            _ => Self::tx_offset(context),
        }
    }

    fn tx_offset(context: u8) -> Result<usize, &'static str> {
        match context {
            CONTEXT_CALLBACK | CONTEXT_COMMAND => Ok(0),
            CONTEXT_OOB_CALLBACK | CONTEXT_OOB_COMMAND => Ok(0x08000),
            _ => Err("apple-dcp: unsupported IOMFB transmit context"),
        }
    }

    fn nested_command_context(context: u8, end: usize) -> (u8, usize) {
        match context {
            CONTEXT_CALLBACK => (CONTEXT_CALLBACK, end),
            CONTEXT_COMMAND => (CONTEXT_CALLBACK, 0),
            CONTEXT_OOB_CALLBACK => (CONTEXT_OOB_CALLBACK, end),
            CONTEXT_OOB_COMMAND => (CONTEXT_OOB_CALLBACK, 0),
            CONTEXT_ASYNC => (CONTEXT_COMMAND, 0),
            CONTEXT_OOB_ASYNC => (CONTEXT_OOB_COMMAND, 0),
            _ => (CONTEXT_COMMAND, 0),
        }
    }

    fn write_packet(
        &mut self,
        offset: usize,
        tag: [u8; 4],
        input: &[u8],
        output_len: usize,
    ) -> Result<usize, &'static str> {
        let length = mem::size_of::<PacketHeader>()
            .checked_add(input.len())
            .and_then(|length| length.checked_add(output_len))
            .ok_or("apple-dcp: IOMFB packet length overflow")?;
        let aligned = length.div_ceil(PACKET_ALIGNMENT) * PACKET_ALIGNMENT;
        let packet = self.shared_slice_mut(offset, aligned)?;
        packet.fill(0);
        let header = PacketHeader {
            tag,
            input_len: input.len() as u32,
            output_len: output_len as u32,
        };
        // SAFETY: `packet` is aligned only as bytes, so the packed header is written unaligned.
        unsafe { core::ptr::write_unaligned(packet.as_mut_ptr() as *mut PacketHeader, header) };
        packet[mem::size_of::<PacketHeader>()..mem::size_of::<PacketHeader>() + input.len()]
            .copy_from_slice(input);
        scarlet::arch::clean_dcache_to_poc_range(self.shmem.as_ptr() as usize + offset, aligned);
        Ok(length)
    }

    fn packet_header(&self, offset: usize) -> Result<PacketHeader, &'static str> {
        let bytes = self.shared_slice(offset, mem::size_of::<PacketHeader>())?;
        // SAFETY: bounds are checked and the packed header may be unaligned.
        Ok(unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const PacketHeader) })
    }

    fn handle_callback(
        &mut self,
        context: u8,
        offset: usize,
        length: usize,
    ) -> Result<(), &'static str> {
        let base = Self::channel_offset(context)?
            .checked_add(offset)
            .ok_or("apple-dcp: IOMFB callback offset overflow")?;
        scarlet::arch::invalidate_dcache_to_poc_range(self.shmem.as_ptr() as usize + base, length);
        let header = self.packet_header(base)?;
        let tag = [header.tag[3], header.tag[2], header.tag[1], header.tag[0]];
        let input_offset = base + mem::size_of::<PacketHeader>();
        let callback_end = offset
            .checked_add(length.div_ceil(PACKET_ALIGNMENT) * PACKET_ALIGNMENT)
            .ok_or("apple-dcp: IOMFB callback stack overflow")?;
        self.callback_ends.push((context, callback_end));

        if header.output_len != 0 {
            let output_offset = input_offset + header.input_len as usize;
            self.shared_slice_mut(output_offset, header.output_len as usize)?
                .fill(0);
        }

        let result = match &tag {
            b"D120" => {
                self.handle_boot_callback()?;
                if header.output_len != 0 {
                    let output_offset = input_offset + header.input_len as usize;
                    self.shared_slice_mut(output_offset, header.output_len as usize)?[0] = 1;
                }
                Ok(())
            }
            b"D411" => self.handle_map_register(input_offset, header.input_len as usize),
            b"D451" => self.handle_allocate_buffer(input_offset, header.input_len as usize),
            b"D452" => self.handle_map_physical(input_offset, header.input_len as usize),
            b"D454" => self.handle_release_descriptor(input_offset, header.input_len as usize),
            b"D003" => {
                if let Some(bandwidth) = self.bandwidth {
                    let output = self.shared_slice_mut(
                        input_offset + header.input_len as usize,
                        header.output_len as usize,
                    )?;
                    if output.len() < 60 {
                        return Err("apple-dcp: short D003 response");
                    }
                    write_u64(output, 8, bandwidth.scratch);
                    write_u64(output, 16, bandwidth.doorbell);
                    write_u32(output, 28, bandwidth.doorbell_bit);
                    write_u32(output, 44, 4);
                }
                Ok(())
            }
            b"D100" => {
                let mut response = [0u8; 4];
                let tag = if self.firmware_12_3 {
                    *b"A358"
                } else {
                    *b"A374"
                };
                self.call(tag, &[], &mut response)
            }
            b"D206" => {
                let mut response = [0u8; 4];
                self.call(*b"A131", &[], &mut response)?;
                if header.output_len != 0 {
                    self.shared_slice_mut(
                        input_offset + header.input_len as usize,
                        header.output_len as usize,
                    )?[0] = 1;
                }
                Ok(())
            }
            b"D207" => {
                let mut response = [0u8; 4];
                self.call(*b"A132", &[], &mut response)?;
                if header.output_len != 0 {
                    self.shared_slice_mut(
                        input_offset + header.input_len as usize,
                        header.output_len as usize,
                    )?[0] = 1;
                }
                Ok(())
            }
            b"D124" => {
                let input = self.shared_slice(input_offset, header.input_len as usize)?;
                if input.len() >= 72 && header.output_len >= 33 {
                    let value = input[68..72].to_vec();
                    let output = self.shared_slice_mut(
                        input_offset + header.input_len as usize,
                        header.output_len as usize,
                    )?;
                    output[..4].copy_from_slice(&value);
                }
                Ok(())
            }
            b"D126" | b"D127" | b"D128" | b"D414" => {
                if header.output_len != 0 {
                    self.shared_slice_mut(
                        input_offset + header.input_len as usize,
                        header.output_len as usize,
                    )?[0] = 1;
                }
                Ok(())
            }
            b"D129" => {
                if header.output_len >= 20 {
                    let input = self.shared_slice(input_offset, header.input_len as usize)?;
                    if input.len() < 16 {
                        return Err("apple-dcp: short D129 request");
                    }
                    let values = input[..16].to_vec();
                    let output = self.shared_slice_mut(
                        input_offset + header.input_len as usize,
                        header.output_len as usize,
                    )?;
                    output[..16].copy_from_slice(&values);
                    write_u32(output, 16, 1);
                }
                Ok(())
            }
            b"D114" => {
                if header.output_len < 8 {
                    return Err("apple-dcp: short D114 response");
                }
                let output = self.shared_slice_mut(
                    input_offset + header.input_len as usize,
                    header.output_len as usize,
                )?;
                write_u32(output, 4, 1);
                Ok(())
            }
            b"D201" => self.handle_map_piodma(input_offset, header.input_len as usize),
            b"D202" => self.handle_unmap_piodma(input_offset, header.input_len as usize),
            b"D209" | b"D300" | b"D401" | b"D404" | b"D406" | b"D593" => Ok(()),
            b"D408" => {
                let clock_frequency = self.clock_frequency;
                let output = self.shared_slice_mut(
                    input_offset + header.input_len as usize,
                    header.output_len as usize,
                )?;
                if output.len() < 8 {
                    return Err("apple-dcp: short D408 response");
                }
                write_u64(output, 0, clock_frequency);
                Ok(())
            }
            b"D589" => {
                let input = self.shared_slice(input_offset, header.input_len as usize)?;
                if input.len() < mem::size_of::<u32>() {
                    return Err("apple-dcp: short IOMFB swap completion");
                }
                self.last_completed_swap = u32::from_le_bytes(
                    input[..4]
                        .try_into()
                        .map_err(|_| "apple-dcp: invalid IOMFB swap completion")?,
                );
                Ok(())
            }
            b"D000" | b"D001" | b"D108" | b"D109" | b"D110" | b"D111" | b"D112" | b"D113"
            | b"D413" | b"D415" | b"D552" | b"D561" | b"D563" | b"D565" | b"D567" | b"D582" => {
                if header.output_len != 0 {
                    let output_offset = input_offset + header.input_len as usize;
                    self.shared_slice_mut(output_offset, header.output_len as usize)?[0] = 1;
                }
                Ok(())
            }
            b"D002" | b"D006" | b"D101" | b"D102" | b"D103" | b"D104" | b"D107" | b"D115"
            | b"D121" | b"D122" | b"D208" | b"D574" | b"D576" | b"D577" | b"D584" | b"D588"
            | b"D591" | b"D592" | b"D594" | b"D596" | b"D597" | b"D598" => Ok(()),
            _ => {
                let input_len = header.input_len;
                let output_len = header.output_len;
                println!(
                    "[apple-dcp] unsupported IOMFB callback {}{}{}{} in={} out={}",
                    tag[0] as char,
                    tag[1] as char,
                    tag[2] as char,
                    tag[3] as char,
                    input_len,
                    output_len
                );
                Err("apple-dcp: unsupported IOMFB callback")
            }
        };
        self.callback_ends.pop();
        result?;

        if tag == *b"D209" {
            let output = self.shared_slice_mut(
                input_offset + header.input_len as usize,
                header.output_len as usize,
            )?;
            if output.len() < 8 {
                return Err("apple-dcp: short D209 response");
            }
            let milliseconds = time::system_time_us().unwrap_or_else(time::current_time) / 1_000;
            write_u64(output, 0, milliseconds);
        }

        if header.output_len != 0 {
            let output_offset = input_offset + header.input_len as usize;
            scarlet::arch::clean_dcache_to_poc_range(
                self.shmem.as_ptr() as usize + output_offset,
                header.output_len as usize,
            );
        }
        self.rtkit.send(&RtkitMessage {
            ep: ENDPOINT,
            msg: Self::rpc_message(context, 0, 0, true),
        })?;
        Ok(())
    }

    fn next_descriptor_id(&mut self) -> u32 {
        let id = self.next_descriptor_id;
        self.next_descriptor_id = self.next_descriptor_id.saturating_add(1);
        id
    }

    fn handle_map_register(
        &mut self,
        input_offset: usize,
        input_len: usize,
    ) -> Result<(), &'static str> {
        let input = self.shared_slice(input_offset, input_len)?;
        if input.len() < 8 {
            return Err("apple-dcp: short D411 request");
        }
        let index = u32::from_le_bytes(
            input[4..8]
                .try_into()
                .map_err(|_| "apple-dcp: invalid D411 register index")?,
        ) as usize;
        let (paddr, size) = *self
            .registers
            .get(index)
            .ok_or("apple-dcp: D411 register index out of range")?;
        let dva = self
            .rtkit
            .map_dma(paddr, size)
            .map_err(|_| "apple-dcp: D411 register mapping failed")?;
        let header = self.packet_header(input_offset - mem::size_of::<PacketHeader>())?;
        let output_offset = input_offset + header.input_len as usize;
        let output = self.shared_slice_mut(output_offset, header.output_len as usize)?;
        if output.len() < 28 {
            return Err("apple-dcp: short D411 response");
        }
        write_u64(output, 0, dva);
        write_u64(output, 8, paddr as u64);
        write_u64(output, 16, size as u64);
        write_u32(output, 24, 0);
        let id = self.next_descriptor_id();
        self.physical_mappings
            .push(PhysicalMapping { dva, size, id });
        Ok(())
    }

    fn handle_map_piodma(
        &mut self,
        input_offset: usize,
        input_len: usize,
    ) -> Result<(), &'static str> {
        let input = self.shared_slice(input_offset, input_len)?;
        if input.len() < 8 {
            return Err("apple-dcp: short D201 request");
        }
        let id = u64::from_le_bytes(
            input[0..8]
                .try_into()
                .map_err(|_| "apple-dcp: invalid D201 id")?,
        ) as u32;
        let header = self.packet_header(input_offset - mem::size_of::<PacketHeader>())?;
        if header.output_len < 20 {
            return Err("apple-dcp: short D201 response");
        }
        let Some(index) = self
            .allocations
            .iter()
            .position(|allocation| allocation.id == id)
        else {
            let output =
                self.shared_slice_mut(input_offset + input_len, header.output_len as usize)?;
            write_u32(output, 16, 22);
            return Ok(());
        };

        let (paddr, dva, size, already_mapped) = {
            let allocation = &self.allocations[index];
            (
                allocation.pages.as_paddr(),
                allocation.dva,
                allocation.size,
                allocation.piodma_mapped,
            )
        };
        if !already_mapped {
            let page_size = self.piodma_domain.page_size();
            if !(paddr.is_multiple_of(page_size) && (dva as usize).is_multiple_of(page_size)) {
                let output =
                    self.shared_slice_mut(input_offset + input_len, header.output_len as usize)?;
                write_u32(output, 16, 22);
                return Ok(());
            }
            if self
                .piodma_domain
                .map(dva, paddr, size, IommuMapFlags::READ | IommuMapFlags::WRITE)
                .is_err()
            {
                let output =
                    self.shared_slice_mut(input_offset + input_len, header.output_len as usize)?;
                write_u32(output, 16, 22);
                return Ok(());
            }
            self.allocations[index].piodma_mapped = true;
        }

        let output = self.shared_slice_mut(input_offset + input_len, header.output_len as usize)?;
        write_u64(output, 8, dva);
        Ok(())
    }

    fn handle_unmap_piodma(
        &mut self,
        input_offset: usize,
        input_len: usize,
    ) -> Result<(), &'static str> {
        let input = self.shared_slice(input_offset, input_len)?;
        if input.len() < 24 {
            return Err("apple-dcp: short D202 request");
        }
        let id = u64::from_le_bytes(
            input[0..8]
                .try_into()
                .map_err(|_| "apple-dcp: invalid D202 id")?,
        ) as u32;
        let dva = u64::from_le_bytes(
            input[16..24]
                .try_into()
                .map_err(|_| "apple-dcp: invalid D202 DVA")?,
        );
        let Some(index) = self.allocations.iter().position(|allocation| {
            allocation.id == id && allocation.dva == dva && allocation.piodma_mapped
        }) else {
            println!(
                "[apple-dcp] ignoring invalid D202 descriptor={} dva={:#x}",
                id, dva
            );
            return Ok(());
        };
        self.unmap_piodma(index)
    }

    fn unmap_piodma(&mut self, allocation_index: usize) -> Result<(), &'static str> {
        let (dva, size, mapped) = {
            let allocation = &self.allocations[allocation_index];
            (allocation.dva, allocation.size, allocation.piodma_mapped)
        };
        if !mapped {
            return Ok(());
        }
        self.piodma_domain
            .unmap(dva, size)
            .map_err(|_| "apple-dcp: PIODMA unmap failed")?;
        self.allocations[allocation_index].piodma_mapped = false;
        Ok(())
    }

    fn handle_allocate_buffer(
        &mut self,
        input_offset: usize,
        input_len: usize,
    ) -> Result<(), &'static str> {
        let input = self.shared_slice(input_offset, input_len)?;
        if input.len() < 12 {
            return Err("apple-dcp: short D451 request");
        }
        let size = u64::from_le_bytes(
            input[4..12]
                .try_into()
                .map_err(|_| "apple-dcp: invalid D451 size")?,
        ) as usize;
        if size == 0 {
            return Err("apple-dcp: zero-sized D451 allocation");
        }
        let page_size = scarlet::environment::PAGE_SIZE;
        let dma_size = size.div_ceil(self.rtkit.dma_alignment()) * self.rtkit.dma_alignment();
        let pages = dma_size.div_ceil(page_size);
        let allocation = ContiguousPages::new_aligned(pages, self.rtkit.dma_alignment())
            .ok_or("apple-dcp: D451 allocation failed")?;
        // SAFETY: the contiguous allocation covers `dma_size` bytes.
        unsafe { core::ptr::write_bytes(allocation.as_ptr() as *mut u8, 0, dma_size) };
        scarlet::arch::clean_dcache_to_poc_range(allocation.as_ptr() as usize, dma_size);
        let dva = self
            .rtkit
            .map_dma(allocation.as_paddr(), dma_size)
            .map_err(|_| "apple-dcp: D451 mapping failed")?;
        let id = self.next_descriptor_id();
        let header = self.packet_header(input_offset - mem::size_of::<PacketHeader>())?;
        let output_offset = input_offset + header.input_len as usize;
        let output = self.shared_slice_mut(output_offset, header.output_len as usize)?;
        if output.len() < 28 {
            return Err("apple-dcp: short D451 response");
        }
        write_u64(output, 0, 0);
        write_u64(output, 8, dva);
        write_u64(output, 16, size.div_ceil(4096) as u64 * 4096);
        write_u32(output, 24, id);
        self.allocations.push(Allocation {
            pages: allocation,
            dva,
            size: dma_size,
            id,
            piodma_mapped: false,
        });
        Ok(())
    }

    fn handle_map_physical(
        &mut self,
        input_offset: usize,
        input_len: usize,
    ) -> Result<(), &'static str> {
        let input = self.shared_slice(input_offset, input_len)?;
        if input.len() < 16 {
            return Err("apple-dcp: short D452 request");
        }
        let paddr = u64::from_le_bytes(
            input[0..8]
                .try_into()
                .map_err(|_| "apple-dcp: invalid D452 paddr")?,
        ) as usize;
        let size = u64::from_le_bytes(
            input[8..16]
                .try_into()
                .map_err(|_| "apple-dcp: invalid D452 size")?,
        ) as usize;
        let end = paddr
            .checked_add(size)
            .ok_or("apple-dcp: D452 range overflow")?;
        if !self
            .registers
            .iter()
            .any(|(start, length)| paddr >= *start && end <= start.saturating_add(*length))
        {
            return Err("apple-dcp: refusing D452 outside display registers");
        }
        let dva = self
            .rtkit
            .map_dma(paddr, size)
            .map_err(|_| "apple-dcp: D452 mapping failed")?;
        let id = self.next_descriptor_id();
        let header = self.packet_header(input_offset - mem::size_of::<PacketHeader>())?;
        let output_offset = input_offset + header.input_len as usize;
        let output = self.shared_slice_mut(output_offset, header.output_len as usize)?;
        if output.len() < 20 {
            return Err("apple-dcp: short D452 response");
        }
        write_u64(output, 0, dva);
        write_u64(output, 8, size as u64);
        write_u32(output, 16, id);
        self.physical_mappings
            .push(PhysicalMapping { dva, size, id });
        Ok(())
    }

    fn handle_release_descriptor(
        &mut self,
        input_offset: usize,
        input_len: usize,
    ) -> Result<(), &'static str> {
        let input = self.shared_slice(input_offset, input_len)?;
        if input.len() < 4 {
            return Err("apple-dcp: short D454 request");
        }
        let id = u32::from_le_bytes(
            input[0..4]
                .try_into()
                .map_err(|_| "apple-dcp: invalid D454 id")?,
        );
        if let Some(index) = self
            .allocations
            .iter()
            .position(|allocation| allocation.id == id)
        {
            self.unmap_piodma(index)?;
            let allocation = self.allocations.remove(index);
            self.rtkit.unmap_dma(allocation.dva, allocation.size);
        } else if let Some(index) = self
            .physical_mappings
            .iter()
            .position(|mapping| mapping.id == id)
        {
            let mapping = self.physical_mappings.remove(index);
            self.rtkit.unmap_dma(mapping.dva, mapping.size);
        }
        let header = self.packet_header(input_offset - mem::size_of::<PacketHeader>())?;
        if header.output_len != 0 {
            self.shared_slice_mut(input_offset + input_len, header.output_len as usize)?[0] = 1;
        }
        Ok(())
    }

    fn handle_boot_callback(&mut self) -> Result<(), &'static str> {
        let set_create_dfb = if self.firmware_12_3 {
            *b"A357"
        } else {
            *b"A373"
        };
        self.call(set_create_dfb, &[], &mut [])?;
        let mut default_fb = [0u8; 4];
        let create_default_fb = if self.firmware_12_3 {
            *b"A443"
        } else {
            *b"A445"
        };
        self.call(create_default_fb, &[], &mut default_fb)?;
        self.call(*b"A029", &[], &mut [])?;
        let flush_supports_power = if self.firmware_12_3 {
            *b"A463"
        } else {
            *b"A466"
        };
        self.call(flush_supports_power, &1u32.to_le_bytes(), &mut [])?;
        let mut late_init = [0u8; 4];
        if self.firmware_12_3 {
            self.call(*b"A000", &[], &mut late_init)?;
        } else {
            self.call(*b"A000", &1u32.to_le_bytes(), &mut late_init)?;
        }
        let mut refresh_properties = [0u8; 4];
        let refresh = if self.firmware_12_3 {
            *b"A460"
        } else {
            *b"A463"
        };
        self.call(refresh, &[], &mut refresh_properties)
    }

    /// Run the Asahi v13.3 IOMFB start sequence.
    ///
    /// # Returns
    ///
    /// Success after A401 and its nested D120 boot callback sequence complete.
    pub fn start(&mut self) -> Result<(), &'static str> {
        let mut output = [0u8; 4];
        self.call(*b"A401", &[], &mut output)?;

        let mut color_remap_request = [0u8; 8];
        write_u32(&mut color_remap_request, 0, 6);
        let mut color_remap_response = [0u8; 8];
        self.call(*b"A426", &color_remap_request, &mut color_remap_response)?;

        let mut video_power_response = [0u8; 4];
        let video_power = if self.firmware_12_3 {
            *b"A447"
        } else {
            *b"A449"
        };
        self.call(video_power, &0u32.to_le_bytes(), &mut video_power_response)?;
        let first_client_open = if self.firmware_12_3 {
            *b"A454"
        } else {
            *b"A456"
        };
        self.call(first_client_open, &[], &mut [])?;
        let mut main_display = [0u8; 4];
        self.call(*b"A411", &[], &mut main_display)?;
        self.main_display = u32::from_le_bytes(main_display) != 0;
        Ok(())
    }

    /// Notify IOMFB that the main display client is powered on.
    ///
    /// # Returns
    ///
    /// Success after A410 and A472 complete.
    pub fn power_on(&mut self) -> Result<(), &'static str> {
        let handle = if self.main_display { 0u32 } else { 2u32 };
        let mut display_response = [0u8; 4];
        self.call(*b"A410", &handle.to_le_bytes(), &mut display_response)?;

        if !self.main_display {
            let mut parameter = [0u8; 40];
            write_u32(&mut parameter, 0, 14);
            write_u32(&mut parameter, 36, 3);
            let tag = if self.firmware_12_3 {
                *b"A439"
            } else {
                *b"A441"
            };
            let mut parameter_response = [0u8; 4];
            self.call(tag, &parameter, &mut parameter_response)?;
        }

        let mut request = [0u8; 12];
        write_u64(&mut request, 0, 1);
        let mut response = [0u8; 8];
        let tag = if self.firmware_12_3 {
            *b"A468"
        } else {
            *b"A472"
        };
        self.call(tag, &request, &mut response)?;
        let result = u32::from_le_bytes(
            response[4..8]
                .try_into()
                .map_err(|_| "apple-dcp: invalid A472 response")?,
        );
        if result != 0 {
            return Err("apple-dcp: IOMFB power_on failed");
        }
        Ok(())
    }

    fn call(&mut self, tag: [u8; 4], input: &[u8], output: &mut [u8]) -> Result<(), &'static str> {
        let (expected_context, packet_offset) = self
            .callback_ends
            .last()
            .copied()
            .map(|(context, end)| Self::nested_command_context(context, end))
            .unwrap_or((CONTEXT_COMMAND, 0));
        let packet_base = Self::tx_offset(expected_context)?;
        let wire_tag = [tag[3], tag[2], tag[1], tag[0]];
        let length =
            self.write_packet(packet_base + packet_offset, wire_tag, input, output.len())?;
        self.rtkit.send(&RtkitMessage {
            ep: ENDPOINT,
            msg: Self::rpc_message(expected_context, length, packet_offset, false),
        })?;

        let start = time::current_time();
        loop {
            let elapsed = time::current_time().saturating_sub(start);
            if elapsed >= REPLY_TIMEOUT_US {
                return Err("apple-dcp: timeout waiting for IOMFB RPC reply");
            }
            let message = self.recv_timeout(REPLY_TIMEOUT_US - elapsed)?;
            if message & MESSAGE_TYPE_MASK != MESSAGE_TYPE_RPC {
                continue;
            }
            let (context, offset, callback_length, ack) = Self::parse_rpc(message);
            if ack && context == expected_context {
                let response_base = Self::channel_offset(expected_context)?;
                scarlet::arch::invalidate_dcache_to_poc_range(
                    self.shmem.as_ptr() as usize + response_base + packet_offset,
                    length,
                );
                let output_offset =
                    response_base + packet_offset + mem::size_of::<PacketHeader>() + input.len();
                output.copy_from_slice(self.shared_slice(output_offset, output.len())?);
                return Ok(());
            }
            if !ack {
                self.handle_callback(context, offset, callback_length)?;
            }
        }
    }

    /// Begin a runtime swap and return the firmware-assigned swap identifier.
    ///
    /// # Returns
    ///
    /// The swap identifier used by the subsequent submit and D589 completion.
    pub fn swap_start(&mut self) -> Result<u32, &'static str> {
        let request = SwapStartRequest::default();
        let mut output = [0u8; mem::size_of::<SwapStartResponse>()];
        self.call(*b"A407", bytes_of(&request), &mut output)?;
        // SAFETY: the output has the exact packed response size and may be unaligned.
        let response =
            unsafe { core::ptr::read_unaligned(output.as_ptr() as *const SwapStartResponse) };
        if response.result != 0 {
            return Err("apple-dcp: IOMFB swap_start failed");
        }
        Ok(response.swap_id)
    }

    /// Submit one linear BGRA surface for an atomic runtime swap.
    ///
    /// # Arguments
    ///
    /// * `swap_id` - Identifier returned by [`Self::swap_start`].
    /// * `surface_dva` - DCP-visible address of the surface.
    /// * `width` - Surface and destination width.
    /// * `height` - Surface and destination height.
    /// * `stride` - Surface row stride in bytes.
    ///
    /// # Returns
    ///
    /// Success after A408 accepts the swap. Completion is reported separately
    /// by [`Self::wait_swap_complete`].
    pub fn swap_submit(
        &mut self,
        swap_id: u32,
        surface_dva: u64,
        width: u32,
        height: u32,
        stride: u32,
    ) -> Result<(), &'static str> {
        let (submit_size, surface_size, surfaces_offset, surface_iova_offset) =
            if self.firmware_12_3 {
                (
                    SWAP_SUBMIT_SIZE_V12_3,
                    SWAP_SURFACE_SIZE_V12_3,
                    SWAP_SURFACES_OFFSET_V12_3,
                    SWAP_SURFACE_IOVA_OFFSET_V12_3,
                )
            } else {
                (
                    SWAP_SUBMIT_SIZE_V13_5,
                    SWAP_SURFACE_SIZE_V13_5,
                    SWAP_SURFACES_OFFSET_V13_5,
                    SWAP_SURFACE_IOVA_OFFSET_V13_5,
                )
            };
        let mut request = vec![0u8; submit_size];
        write_u32(&mut request, 80, swap_id);
        write_u32(&mut request, 132, 0);
        write_u32(&mut request, 136, 0);
        write_u32(&mut request, 140, width);
        write_u32(&mut request, 144, height);
        write_u32(&mut request, 228, 0);
        write_u32(&mut request, 232, 0);
        write_u32(&mut request, 236, width);
        write_u32(&mut request, 240, height);
        write_u32(&mut request, 260, 1 << 2);
        write_u32(&mut request, 264, 1 << 2);

        let surface = surfaces_offset + 2 * surface_size;
        write_u32(&mut request, surface + 3, 1);
        write_u32(&mut request, surface + 7, 1);
        write_u32(&mut request, surface + 11, u32::from_le_bytes(*b"ARGB"));
        request[surface + 19] = 13;
        request[surface + 20] = 12;
        write_u32(&mut request, surface + 21, stride);
        request[surface + 25..surface + 27].copy_from_slice(&1u16.to_le_bytes());
        request[surface + 27] = 1;
        request[surface + 28] = 1;
        write_u32(&mut request, surface + 33, width);
        write_u32(&mut request, surface + 37, height);
        write_u32(&mut request, surface + 41, height.saturating_mul(stride));
        write_u64(&mut request, surface + 81, 1);
        write_u32(&mut request, surface + 89, width);
        write_u32(&mut request, surface + 93, height);
        write_u32(&mut request, surface + 105, stride);
        write_u32(&mut request, surface + 109, height.saturating_mul(stride));
        request[surface + 113..surface + 115].copy_from_slice(&1u16.to_le_bytes());
        request[surface + 115] = 1;
        request[surface + 116] = 1;
        write_u64(&mut request, surface + 329, 1);
        write_u64(&mut request, surface_iova_offset + 2 * 8, surface_dva);

        if self.firmware_12_3 {
            request[2909] = 1;
            request[2910..2914].fill(1);
            request[2912] = 0;
            request[2914] = 1;
        } else {
            request[6263..6267].fill(1);
            request[6265] = 0;
            request[6267..6272].fill(1);
            request[6273] = 1;
            request[6274] = 1;
        }

        let mut output = [0u8; 12];
        let output_len = if self.firmware_12_3 { 8 } else { 12 };
        self.call(*b"A408", &request, &mut output[..output_len])?;
        let result_offset = if self.firmware_12_3 { 1 } else { 5 };
        let result = u32::from_le_bytes(
            output[result_offset..result_offset + 4]
                .try_into()
                .map_err(|_| "apple-dcp: invalid IOMFB swap_submit response")?,
        );
        if result != 0 {
            return Err("apple-dcp: IOMFB swap_submit failed");
        }
        Ok(())
    }

    /// Wait until D589 reports completion of a submitted swap.
    ///
    /// # Arguments
    ///
    /// * `swap_id` - Submitted swap identifier.
    ///
    /// # Returns
    ///
    /// Success once the corresponding hardware flip has completed.
    pub fn wait_swap_complete(&mut self, swap_id: u32) -> Result<(), &'static str> {
        let start = time::current_time();
        while self.last_completed_swap != swap_id {
            let elapsed = time::current_time().saturating_sub(start);
            if elapsed >= REPLY_TIMEOUT_US {
                return Err("apple-dcp: timeout waiting for IOMFB swap completion");
            }
            let message = self.recv_timeout(REPLY_TIMEOUT_US - elapsed)?;
            if message & MESSAGE_TYPE_MASK != MESSAGE_TYPE_RPC {
                continue;
            }
            let (context, offset, length, ack) = Self::parse_rpc(message);
            if !ack {
                self.handle_callback(context, offset, length)?;
            }
        }
        Ok(())
    }
}

impl Drop for Iomfb {
    fn drop(&mut self) {
        for index in 0..self.allocations.len() {
            let _ = self.unmap_piodma(index);
        }
        for allocation in &self.allocations {
            self.rtkit.unmap_dma(allocation.dva, allocation.size);
        }
        for mapping in &self.physical_mappings {
            self.rtkit.unmap_dma(mapping.dva, mapping.size);
        }
        self.rtkit.unmap_dma(self.shmem_dva, SHMEM_SIZE);
    }
}
