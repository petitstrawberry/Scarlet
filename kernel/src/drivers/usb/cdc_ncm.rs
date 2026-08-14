//! USB Communications Device Class Network Control Model support.
//!
//! This module implements the device-independent portion of CDC-NCM: USB
//! configuration descriptor matching, Ethernet MAC address decoding, NTB16
//! framing, and Scarlet network-device integration. Host-controller drivers
//! provide the actual control and bulk transfers through [`CdcNcmTransport`].

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::device::network::{
    DevicePacket, EthernetDevice, MacAddress, NetworkDevice, NetworkInterfaceConfig, NetworkStats,
};
use crate::device::{Device, DeviceType};
use crate::network::config::apply_pending_ip_for_interface;
use crate::network::ethernet_interface::EthernetNetworkInterface;
use crate::network::get_network_manager;
use crate::object::capability::{ControlOps, MemoryMappingOps, Selectable};
use crate::sync::{IrqSpinLock, Lazy};

const USB_DT_CONFIGURATION: u8 = 0x02;
const USB_DT_INTERFACE: u8 = 0x04;
const USB_DT_ENDPOINT: u8 = 0x05;
const USB_DT_CS_INTERFACE: u8 = 0x24;
const USB_CLASS_COMMUNICATIONS: u8 = 0x02;
const USB_CLASS_CDC_DATA: u8 = 0x0a;
const USB_CDC_SUBCLASS_NCM: u8 = 0x0d;
const USB_CDC_UNION_TYPE: u8 = 0x06;
const USB_CDC_ETHERNET_TYPE: u8 = 0x0f;
const USB_CDC_NCM_TYPE: u8 = 0x1a;
const USB_ENDPOINT_XFER_BULK: u8 = 0x02;

const NTH16_SIGNATURE: u32 = 0x484d_434e;
const NDP16_NO_CRC_SIGNATURE: u32 = 0x304d_434e;
const NTH16_LENGTH: usize = 12;
const NDP16_ONE_DATAGRAM_LENGTH: usize = 16;
const ETHERNET_HEADER_LENGTH: usize = 14;
const ETHERNET_MTU: usize = 1500;
const NCM_MIN_NTB_SIZE: usize = 2_048;
const NCM_DEFAULT_NTB_SIZE: usize = 16_384;
const NCM_MAX_NTB16_SIZE: usize = u16::MAX as usize;
const NCM_RX_QUEUE_LIMIT: usize = 256;
const NCM_RX_WORK_BUDGET: usize = 64;
const NCM_MAX_DATAGRAMS_PER_NTB: usize = 64;

const CDC_PACKET_TYPE_ALL_MULTICAST: u16 = 1 << 1;
const CDC_PACKET_TYPE_DIRECTED: u16 = 1 << 2;
const CDC_PACKET_TYPE_BROADCAST: u16 = 1 << 3;
const CDC_PACKET_TYPE_PROMISCUOUS: u16 = 1 << 0;

const DEFAULT_PACKET_FILTER: u16 =
    CDC_PACKET_TYPE_DIRECTED | CDC_PACKET_TYPE_BROADCAST | CDC_PACKET_TYPE_ALL_MULTICAST;

/// Host-controller operations required by a CDC-NCM network device.
pub trait CdcNcmTransport: Send + Sync {
    /// Queue one complete Network Transfer Block for the bulk OUT endpoint.
    ///
    /// # Arguments
    ///
    /// * `ntb` - Owned complete NTB16 payload to transfer.
    /// * `frame_len` - Length of the Ethernet frame carried by the NTB.
    ///
    /// # Returns
    ///
    /// `Ok(())` once the host controller accepts the request. Completion is
    /// reported asynchronously through [`CdcNcmDevice::handle_transmit_complete`]
    /// or [`CdcNcmDevice::handle_transmit_error`].
    fn enqueue_ntb(&self, ntb: Vec<u8>, frame_len: usize) -> Result<(), &'static str>;

    /// Program the CDC Ethernet packet filter on the control interface.
    ///
    /// # Arguments
    ///
    /// * `filter` - CDC packet-filter bitmap.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the class-specific control request succeeds.
    fn set_packet_filter(&self, filter: u16) -> Result<(), &'static str>;
}

/// USB interface and functional-descriptor data needed to bind CDC-NCM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CdcNcmInterfaceConfig {
    pub configuration_value: u8,
    pub control_interface: u8,
    pub data_interface: u8,
    pub data_alternate_setting: u8,
    pub notification_endpoint: u8,
    pub notification_max_packet_size: u16,
    pub notification_interval: u8,
    pub bulk_in_endpoint: u8,
    pub bulk_in_max_packet_size: u16,
    pub bulk_out_endpoint: u8,
    pub bulk_out_max_packet_size: u16,
    pub mac_string_index: u8,
    pub max_segment_size: u16,
    pub network_capabilities: u8,
}

/// Device-provided NTB sizing and alignment parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CdcNcmParameters {
    pub formats_supported: u16,
    pub ntb_in_max_size: u32,
    pub ndp_in_divisor: u16,
    pub ndp_in_payload_remainder: u16,
    pub ndp_in_alignment: u16,
    pub ntb_out_max_size: u32,
    pub ndp_out_divisor: u16,
    pub ndp_out_payload_remainder: u16,
    pub ndp_out_alignment: u16,
    pub ntb_out_max_datagrams: u16,
}

impl CdcNcmParameters {
    /// Decode the 28-byte GET_NTB_PARAMETERS response.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Class-control response bytes.
    ///
    /// # Returns
    ///
    /// Parsed parameters, or an error for a short or unusable response.
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 28 || read_u16(bytes, 0)? < 28 {
            return Err("Invalid CDC-NCM NTB parameters");
        }

        let parameters = Self {
            formats_supported: read_u16(bytes, 2)?,
            ntb_in_max_size: read_u32(bytes, 4)?,
            ndp_in_divisor: read_u16(bytes, 8)?,
            ndp_in_payload_remainder: read_u16(bytes, 10)?,
            ndp_in_alignment: read_u16(bytes, 12)?,
            ntb_out_max_size: read_u32(bytes, 16)?,
            ndp_out_divisor: read_u16(bytes, 20)?,
            ndp_out_payload_remainder: read_u16(bytes, 22)?,
            ndp_out_alignment: read_u16(bytes, 24)?,
            ntb_out_max_datagrams: read_u16(bytes, 26)?,
        };
        if parameters.formats_supported & 1 == 0 {
            return Err("CDC-NCM device does not support NTB16");
        }
        if parameters.ntb_in_max_size < NCM_MIN_NTB_SIZE as u32
            || (parameters.ntb_out_max_size != 0
                && parameters.ntb_out_max_size < NCM_MIN_NTB_SIZE as u32)
        {
            return Err("CDC-NCM device reports an unusable NTB size");
        }
        Ok(parameters)
    }

    /// Select the conservative receive NTB size used by Scarlet.
    pub(crate) fn receive_size(self) -> usize {
        (self.ntb_in_max_size as usize)
            .min(NCM_DEFAULT_NTB_SIZE)
            .clamp(NCM_MIN_NTB_SIZE, NCM_MAX_NTB16_SIZE)
    }

    /// Select the conservative transmit NTB size used by Scarlet.
    ///
    /// # Returns
    ///
    /// Maximum NTB16 transfer length accepted by the device and Scarlet.
    pub(crate) fn transmit_size(self) -> usize {
        let reported_max_size = if self.ntb_out_max_size == 0 {
            NCM_DEFAULT_NTB_SIZE
        } else {
            self.ntb_out_max_size as usize
        };
        reported_max_size
            .min(NCM_DEFAULT_NTB_SIZE)
            .min(NCM_MAX_NTB16_SIZE)
    }
}

#[derive(Clone, Copy)]
struct DataInterfaceCandidate {
    interface_number: u8,
    alternate_setting: u8,
    bulk_in: Option<(u8, u16)>,
    bulk_out: Option<(u8, u16)>,
}

/// Parse one USB configuration and return its CDC-NCM function.
///
/// # Arguments
///
/// * `bytes` - Raw configuration descriptor and all subordinate descriptors.
///
/// # Returns
///
/// The matched NCM interfaces and endpoints, or an error when this
/// configuration is not a usable CDC-NCM function.
pub(crate) fn parse_configuration(bytes: &[u8]) -> Result<CdcNcmInterfaceConfig, &'static str> {
    if bytes.len() < 9 || bytes[1] != USB_DT_CONFIGURATION {
        return Err("Invalid USB configuration descriptor");
    }
    let total_length = usize::from(read_u16(bytes, 2)?);
    if total_length < 9 || total_length > bytes.len() {
        return Err("Truncated USB configuration descriptor");
    }

    let configuration_value = bytes[5];
    let mut offset = 0usize;
    let mut current_interface: Option<(u8, u8, u8)> = None;
    let mut current_data_candidate: Option<usize> = None;
    let mut control_interface = None;
    let mut union_interfaces = None;
    let mut mac_string_index = None;
    let mut max_segment_size = None;
    let mut network_capabilities = None;
    let mut notification_endpoint = None;
    let mut candidates = Vec::<DataInterfaceCandidate>::new();

    while offset + 2 <= total_length {
        let length = usize::from(bytes[offset]);
        let descriptor_type = bytes[offset + 1];
        if length < 2 || offset + length > total_length {
            return Err("Malformed USB descriptor in CDC-NCM configuration");
        }
        let descriptor = &bytes[offset..offset + length];

        match descriptor_type {
            USB_DT_INTERFACE if length >= 9 => {
                let number = descriptor[2];
                let alternate = descriptor[3];
                let class = descriptor[5];
                let subclass = descriptor[6];
                current_interface = Some((number, class, subclass));
                current_data_candidate = None;

                if class == USB_CLASS_COMMUNICATIONS && subclass == USB_CDC_SUBCLASS_NCM {
                    control_interface = Some(number);
                } else if class == USB_CLASS_CDC_DATA {
                    candidates.push(DataInterfaceCandidate {
                        interface_number: number,
                        alternate_setting: alternate,
                        bulk_in: None,
                        bulk_out: None,
                    });
                    current_data_candidate = Some(candidates.len() - 1);
                }
            }
            USB_DT_CS_INTERFACE if length >= 3 => {
                let Some((_, class, subclass)) = current_interface else {
                    offset += length;
                    continue;
                };
                if class != USB_CLASS_COMMUNICATIONS || subclass != USB_CDC_SUBCLASS_NCM {
                    offset += length;
                    continue;
                }

                match descriptor[2] {
                    USB_CDC_UNION_TYPE if length >= 5 => {
                        union_interfaces = Some((descriptor[3], descriptor[4]));
                    }
                    USB_CDC_ETHERNET_TYPE if length >= 13 => {
                        mac_string_index = Some(descriptor[3]);
                        max_segment_size = Some(u16::from_le_bytes([descriptor[8], descriptor[9]]));
                    }
                    USB_CDC_NCM_TYPE if length >= 6 => {
                        network_capabilities = Some(descriptor[5]);
                    }
                    _ => {}
                }
            }
            USB_DT_ENDPOINT if length >= 7 => {
                if let Some((_, class, subclass)) = current_interface
                    && class == USB_CLASS_COMMUNICATIONS
                    && subclass == USB_CDC_SUBCLASS_NCM
                    && descriptor[3] & 0x03 == 0x03
                    && descriptor[2] & 0x80 != 0
                {
                    let max_packet_size =
                        u16::from_le_bytes([descriptor[4], descriptor[5]]) & 0x07ff;
                    if max_packet_size != 0 {
                        notification_endpoint =
                            Some((descriptor[2], max_packet_size, descriptor[6]));
                    }
                    offset += length;
                    continue;
                }

                let Some(candidate_index) = current_data_candidate else {
                    offset += length;
                    continue;
                };
                if descriptor[3] & 0x03 != USB_ENDPOINT_XFER_BULK {
                    offset += length;
                    continue;
                }
                let endpoint = descriptor[2];
                let max_packet_size = u16::from_le_bytes([descriptor[4], descriptor[5]]) & 0x07ff;
                if max_packet_size == 0 {
                    offset += length;
                    continue;
                }
                if endpoint & 0x80 != 0 {
                    candidates[candidate_index].bulk_in = Some((endpoint, max_packet_size));
                } else {
                    candidates[candidate_index].bulk_out = Some((endpoint, max_packet_size));
                }
            }
            _ => {}
        }
        offset += length;
    }

    let control_interface = control_interface.ok_or("No CDC-NCM control interface found")?;
    let (union_control, data_interface) =
        union_interfaces.ok_or("CDC-NCM union descriptor is missing")?;
    if union_control != control_interface {
        return Err("CDC-NCM union descriptor has the wrong control interface");
    }
    let candidate = candidates
        .iter()
        .find(|candidate| {
            candidate.interface_number == data_interface
                && candidate.bulk_in.is_some()
                && candidate.bulk_out.is_some()
        })
        .ok_or("No active CDC-NCM data alternate setting found")?;
    let (bulk_in_endpoint, bulk_in_max_packet_size) = candidate
        .bulk_in
        .ok_or("CDC-NCM bulk IN endpoint disappeared")?;
    let (bulk_out_endpoint, bulk_out_max_packet_size) = candidate
        .bulk_out
        .ok_or("CDC-NCM bulk OUT endpoint disappeared")?;
    let max_segment_size =
        max_segment_size.unwrap_or((ETHERNET_MTU + ETHERNET_HEADER_LENGTH) as u16);
    if max_segment_size < ETHERNET_HEADER_LENGTH as u16 {
        return Err("CDC-NCM Ethernet segment size is invalid");
    }
    let network_capabilities =
        network_capabilities.ok_or("CDC-NCM functional descriptor is missing")?;
    let (notification_endpoint, notification_max_packet_size, notification_interval) =
        notification_endpoint.ok_or("CDC-NCM notification endpoint is missing")?;

    Ok(CdcNcmInterfaceConfig {
        configuration_value,
        control_interface,
        data_interface,
        data_alternate_setting: candidate.alternate_setting,
        notification_endpoint,
        notification_max_packet_size,
        notification_interval,
        bulk_in_endpoint,
        bulk_in_max_packet_size,
        bulk_out_endpoint,
        bulk_out_max_packet_size,
        mac_string_index: mac_string_index.ok_or("CDC Ethernet MAC string is missing")?,
        max_segment_size,
        network_capabilities,
    })
}

/// Decode a CDC Ethernet hexadecimal MAC-address string.
///
/// # Arguments
///
/// * `value` - USB string descriptor contents, with or without separators.
///
/// # Returns
///
/// A valid unicast MAC address or an error for malformed input.
pub(crate) fn parse_mac_address(value: &str) -> Result<MacAddress, &'static str> {
    let mut nibbles = [0u8; 12];
    let mut count = 0usize;
    for byte in value.bytes() {
        let nibble = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            b':' | b'-' | b'.' => continue,
            _ => return Err("CDC Ethernet MAC string is not hexadecimal"),
        };
        if count == nibbles.len() {
            return Err("CDC Ethernet MAC string is too long");
        }
        nibbles[count] = nibble;
        count += 1;
    }
    if count != nibbles.len() {
        return Err("CDC Ethernet MAC string must contain 12 hexadecimal digits");
    }

    let mut bytes = [0u8; 6];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (nibbles[index * 2] << 4) | nibbles[index * 2 + 1];
    }
    validate_mac_address(bytes)
}

/// Validate a six-byte address returned by GET_NET_ADDRESS.
///
/// # Arguments
///
/// * `bytes` - Raw Ethernet address.
///
/// # Returns
///
/// A valid unicast MAC address or an error for a reserved address.
pub(crate) fn validate_mac_address(bytes: [u8; 6]) -> Result<MacAddress, &'static str> {
    let mac = MacAddress::new(bytes);
    if bytes == [0; 6] || mac.is_broadcast() || !mac.is_unicast() {
        return Err("CDC-NCM device reported an invalid unicast MAC address");
    }
    Ok(mac)
}

/// Build the payload for a SET_NTB_INPUT_SIZE request.
///
/// # Arguments
///
/// * `receive_size` - Maximum NTB size Scarlet can receive.
/// * `max_datagrams` - Optional maximum datagram count for devices advertising
///   the extended eight-byte input-size structure.
///
/// # Returns
///
/// The four-byte legacy or eight-byte extended little-endian payload.
pub(crate) fn ntb_input_size_payload(receive_size: u32, max_datagrams: Option<u16>) -> Vec<u8> {
    let mut bytes = receive_size.to_le_bytes().to_vec();
    if let Some(max_datagrams) = max_datagrams {
        bytes.extend_from_slice(&max_datagrams.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
    }
    bytes
}

#[derive(Clone, Copy)]
struct Ntb16TxConfig {
    max_size: usize,
    payload_divisor: usize,
    payload_remainder: usize,
    ndp_alignment: usize,
    max_segment_size: usize,
    output_max_packet_size: usize,
}

impl Ntb16TxConfig {
    fn new(
        parameters: CdcNcmParameters,
        max_segment_size: usize,
        output_max_packet_size: usize,
    ) -> Result<Self, &'static str> {
        let max_size = parameters.transmit_size();
        if max_size < NCM_MIN_NTB_SIZE || output_max_packet_size == 0 {
            return Err("CDC-NCM transmit parameters are unusable");
        }

        let payload_divisor = sanitize_alignment(parameters.ndp_out_divisor, max_size);
        let raw_remainder = usize::from(parameters.ndp_out_payload_remainder);
        let raw_remainder = if raw_remainder < payload_divisor {
            raw_remainder
        } else {
            0
        };
        let payload_remainder =
            raw_remainder.wrapping_sub(ETHERNET_HEADER_LENGTH) & (payload_divisor - 1);

        Ok(Self {
            max_size,
            payload_divisor,
            payload_remainder,
            ndp_alignment: sanitize_alignment(parameters.ndp_out_alignment, max_size),
            max_segment_size,
            output_max_packet_size,
        })
    }
}

fn sanitize_alignment(value: u16, max_size: usize) -> usize {
    let value = usize::from(value);
    if value < 4 || !value.is_power_of_two() || value >= max_size {
        4
    } else {
        value
    }
}

fn align_to_remainder(offset: usize, divisor: usize, remainder: usize) -> usize {
    let delta = (remainder + divisor - (offset & (divisor - 1))) & (divisor - 1);
    offset + delta
}

fn build_ntb16(
    frame: &[u8],
    sequence: u16,
    config: Ntb16TxConfig,
) -> Result<Vec<u8>, &'static str> {
    if frame.len() < ETHERNET_HEADER_LENGTH || frame.len() > config.max_segment_size {
        return Err("Ethernet frame exceeds the CDC-NCM segment size");
    }

    let ndp_offset = align_to_remainder(NTH16_LENGTH, config.ndp_alignment, 0);
    let payload_offset = align_to_remainder(
        ndp_offset + NDP16_ONE_DATAGRAM_LENGTH,
        config.payload_divisor,
        config.payload_remainder,
    );
    let mut block_length = payload_offset
        .checked_add(frame.len())
        .ok_or("CDC-NCM transmit length overflow")?;
    if block_length % config.output_max_packet_size == 0 {
        block_length = block_length
            .checked_add(1)
            .ok_or("CDC-NCM transmit padding overflow")?;
    }
    if block_length > config.max_size || block_length > NCM_MAX_NTB16_SIZE {
        return Err("Ethernet frame does not fit in a CDC-NCM NTB16");
    }

    let mut ntb = vec![0u8; block_length];
    write_u32(&mut ntb, 0, NTH16_SIGNATURE)?;
    write_u16(&mut ntb, 4, NTH16_LENGTH as u16)?;
    write_u16(&mut ntb, 6, sequence)?;
    write_u16(&mut ntb, 8, block_length as u16)?;
    write_u16(&mut ntb, 10, ndp_offset as u16)?;

    write_u32(&mut ntb, ndp_offset, NDP16_NO_CRC_SIGNATURE)?;
    write_u16(&mut ntb, ndp_offset + 4, NDP16_ONE_DATAGRAM_LENGTH as u16)?;
    write_u16(&mut ntb, ndp_offset + 6, 0)?;
    write_u16(&mut ntb, ndp_offset + 8, payload_offset as u16)?;
    write_u16(&mut ntb, ndp_offset + 10, frame.len() as u16)?;
    ntb[payload_offset..payload_offset + frame.len()].copy_from_slice(frame);
    Ok(ntb)
}

fn parse_ntb16(data: &[u8], max_segment_size: usize) -> Result<Vec<DevicePacket>, &'static str> {
    if data.len() < NTH16_LENGTH + NDP16_ONE_DATAGRAM_LENGTH {
        return Err("CDC-NCM receive NTB is too short");
    }
    if read_u32(data, 0)? != NTH16_SIGNATURE || read_u16(data, 4)? != NTH16_LENGTH as u16 {
        return Err("CDC-NCM receive NTH16 is invalid");
    }
    let block_length = usize::from(read_u16(data, 8)?);
    if block_length < NTH16_LENGTH || block_length > data.len() {
        return Err("CDC-NCM receive block length is invalid");
    }

    let mut packets = Vec::new();
    let mut ndp_offset = usize::from(read_u16(data, 10)?);
    let mut remaining_tables = 16usize;
    while ndp_offset != 0 {
        if remaining_tables == 0 || ndp_offset + 8 > block_length {
            return Err("CDC-NCM receive NDP chain is invalid");
        }
        remaining_tables -= 1;
        if read_u32(data, ndp_offset)? != NDP16_NO_CRC_SIGNATURE {
            return Err("CDC-NCM receive NDP16 signature is invalid");
        }
        let ndp_length = usize::from(read_u16(data, ndp_offset + 4)?);
        if ndp_length < NDP16_ONE_DATAGRAM_LENGTH
            || ndp_length & 3 != 0
            || ndp_offset + ndp_length > block_length
        {
            return Err("CDC-NCM receive NDP16 length is invalid");
        }
        let next_ndp = usize::from(read_u16(data, ndp_offset + 6)?);
        if next_ndp == ndp_offset {
            return Err("CDC-NCM receive NDP16 contains a cycle");
        }

        let entry_count = (ndp_length - 8) / 4;
        for entry in 0..entry_count {
            let entry_offset = ndp_offset + 8 + entry * 4;
            let frame_offset = usize::from(read_u16(data, entry_offset)?);
            let frame_length = usize::from(read_u16(data, entry_offset + 2)?);
            if frame_offset == 0 || frame_length == 0 {
                break;
            }
            if frame_length < ETHERNET_HEADER_LENGTH
                || frame_length > max_segment_size
                || frame_offset > block_length
                || frame_length > block_length - frame_offset
            {
                return Err("CDC-NCM receive datagram pointer is invalid");
            }
            packets.push(DevicePacket::with_data(
                data[frame_offset..frame_offset + frame_length].to_vec(),
            ));
            if packets.len() > NCM_MAX_DATAGRAMS_PER_NTB {
                return Err("CDC-NCM receive NTB contains too many datagrams");
            }
        }
        ndp_offset = next_ndp;
    }
    if packets.is_empty() {
        return Err("CDC-NCM receive NTB contains no datagrams");
    }
    Ok(packets)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, &'static str> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or("CDC-NCM structure is truncated")?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, &'static str> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or("CDC-NCM structure is truncated")?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) -> Result<(), &'static str> {
    let destination = bytes
        .get_mut(offset..offset + 2)
        .ok_or("CDC-NCM structure write exceeds the buffer")?;
    destination.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) -> Result<(), &'static str> {
    let destination = bytes
        .get_mut(offset..offset + 4)
        .ok_or("CDC-NCM structure write exceeds the buffer")?;
    destination.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

struct QueuedRxPacket {
    interface_name: String,
    packet: DevicePacket,
}

static RX_WORKER_STARTED: AtomicBool = AtomicBool::new(false);
static RX_PACKET_QUEUE: Lazy<IrqSpinLock<VecDeque<QueuedRxPacket>>> =
    Lazy::new(|| IrqSpinLock::new(VecDeque::new()));
static RX_PACKET_WAKER: crate::sync::Waker = crate::sync::Waker::new_uninterruptible("cdc-ncm-rx");

fn ensure_rx_worker_started() {
    if RX_WORKER_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    let task = crate::task::new_kernel_task(String::from("cdc-ncm-rx"), 1, cdc_ncm_rx_worker_entry);
    task.init();
    crate::sched::scheduler::add_task(task, crate::arch::get_cpu().get_cpuid());
}

fn cdc_ncm_rx_worker_entry() {
    loop {
        let processed = drain_queued_rx_packets(&RX_PACKET_QUEUE, NCM_RX_WORK_BUDGET, |queued| {
            get_network_manager().handle_received_packet(&queued.interface_name, &queued.packet);
        });

        let Some(task) = crate::task::mytask() else {
            crate::arch::instruction::idle();
        };
        if processed == NCM_RX_WORK_BUDGET {
            // All queue locks are released by the bounded drain. Give the
            // consumer and unrelated tasks a chance to run under sustained RX.
            crate::sched::scheduler::schedule(task.get_trapframe());
        } else {
            RX_PACKET_WAKER.wait(task.get_id(), task.get_trapframe());
        }
    }
}

fn drain_queued_rx_packets(
    queue: &IrqSpinLock<VecDeque<QueuedRxPacket>>,
    budget: usize,
    mut handle: impl FnMut(QueuedRxPacket),
) -> usize {
    let mut processed = 0usize;
    while processed < budget {
        // Keep the guard in this inner scope. A lock temporary used directly
        // as a `while let` scrutinee lives through the loop body, which would
        // retain this IRQ spinlock while the network stack (and synchronous
        // reply TX) runs.
        let queued = {
            let mut queue = queue.lock();
            queue.pop_front()
        };
        let Some(queued) = queued else {
            break;
        };
        handle(queued);
        processed += 1;
    }
    processed
}

fn enqueue_rx_packets(interface_name: &str, packets: Vec<DevicePacket>) -> usize {
    ensure_rx_worker_started();
    let mut enqueued = 0usize;
    {
        let mut queue = RX_PACKET_QUEUE.lock();
        for packet in packets {
            if queue.len() >= NCM_RX_QUEUE_LIMIT {
                break;
            }
            queue.push_back(QueuedRxPacket {
                interface_name: interface_name.to_string(),
                packet,
            });
            enqueued += 1;
        }
    }
    if enqueued != 0 {
        RX_PACKET_WAKER.wake_one();
    }
    enqueued
}

/// Scarlet Ethernet device backed by a USB CDC-NCM function.
pub struct CdcNcmDevice {
    transport: Arc<dyn CdcNcmTransport>,
    mac_address: MacAddress,
    max_segment_size: usize,
    mtu: usize,
    tx_config: Ntb16TxConfig,
    tx_sequence: IrqSpinLock<u16>,
    stats: IrqSpinLock<NetworkStats>,
    interface_name: String,
    active: AtomicBool,
    link_up: AtomicBool,
    promiscuous: AtomicBool,
}

impl CdcNcmDevice {
    /// Create a configured CDC-NCM network device.
    ///
    /// # Arguments
    ///
    /// * `transport` - Host-controller transfer implementation.
    /// * `mac_address` - Ethernet address reported by the USB function.
    /// * `max_segment_size` - Maximum Ethernet frame size, including its header.
    /// * `parameters` - Device NTB parameters.
    /// * `output_max_packet_size` - Bulk OUT endpoint packet size.
    /// * `interface_name` - Scarlet interface registration name.
    ///
    /// # Returns
    ///
    /// A ready network device, or an error for unusable parameters.
    pub(crate) fn new(
        transport: Arc<dyn CdcNcmTransport>,
        mac_address: MacAddress,
        max_segment_size: usize,
        parameters: CdcNcmParameters,
        output_max_packet_size: usize,
        interface_name: String,
    ) -> Result<Self, &'static str> {
        if max_segment_size < ETHERNET_HEADER_LENGTH {
            return Err("CDC-NCM maximum segment size is too small");
        }
        let mtu = max_segment_size
            .saturating_sub(ETHERNET_HEADER_LENGTH)
            .min(ETHERNET_MTU);
        Ok(Self {
            transport,
            mac_address,
            max_segment_size,
            mtu,
            tx_config: Ntb16TxConfig::new(parameters, max_segment_size, output_max_packet_size)?,
            tx_sequence: IrqSpinLock::new(0),
            stats: IrqSpinLock::new(NetworkStats::default()),
            interface_name,
            active: AtomicBool::new(true),
            // CDC devices report the authoritative carrier state through the
            // notification endpoint. Start usable so devices that omit the
            // initial notification can still transmit, then follow updates.
            link_up: AtomicBool::new(true),
            promiscuous: AtomicBool::new(false),
        })
    }

    /// Register the device with Scarlet's network and Ethernet layers.
    ///
    /// # Returns
    ///
    /// `Ok(())` after both layers accept the interface.
    pub fn register_interface(self: &Arc<Self>) -> Result<(), &'static str> {
        ensure_rx_worker_started();
        self.transport.set_packet_filter(DEFAULT_PACKET_FILTER)?;
        let interface = Arc::new(EthernetNetworkInterface::new(
            &self.interface_name,
            self.clone(),
        ));
        get_network_manager().register_interface(&self.interface_name, interface.clone())?;

        let ethernet_layer = get_network_manager()
            .get_layer("ethernet")
            .ok_or("Ethernet layer is not initialized")?;
        let ethernet = ethernet_layer
            .as_any()
            .downcast_ref::<crate::network::ethernet::EthernetLayer>()
            .ok_or("Registered Ethernet layer has the wrong type")?;
        ethernet.register_interface(&self.interface_name, self.mac_address, interface);
        apply_pending_ip_for_interface(&self.interface_name);
        Ok(())
    }

    /// Decode a received NTB and enqueue its Ethernet datagrams for the stack.
    ///
    /// # Arguments
    ///
    /// * `ntb` - Bytes completed by the bulk IN transfer.
    pub fn handle_received_ntb(&self, ntb: &[u8]) {
        let packets = match parse_ntb16(ntb, self.max_segment_size) {
            Ok(packets) => packets,
            Err(error) => {
                self.stats.lock().rx_errors += 1;
                crate::println!("[usb-ncm] Dropping invalid receive NTB: {}", error);
                return;
            }
        };

        let packet_count = packets.len();
        let byte_count = packets.iter().map(|packet| packet.len as u64).sum::<u64>();
        {
            let mut stats = self.stats.lock();
            stats.rx_packets += packet_count as u64;
            stats.rx_bytes += byte_count;
        }
        let enqueued = enqueue_rx_packets(&self.interface_name, packets);
        if enqueued < packet_count {
            self.stats.lock().dropped += (packet_count - enqueued) as u64;
        }
    }

    /// Account for a host-controller receive failure.
    ///
    /// # Arguments
    ///
    /// * `error` - Static diagnostic supplied by the host-controller driver.
    pub(crate) fn handle_receive_error(&self, error: &'static str) {
        self.stats.lock().rx_errors += 1;
        crate::println!("[usb-ncm] Receive transfer failed: {}", error);
    }

    /// Account for a completed host-controller transmit request.
    ///
    /// # Arguments
    ///
    /// * `frame_len` - Length of the Ethernet frame carried by the completed NTB.
    pub(crate) fn handle_transmit_complete(&self, frame_len: usize) {
        let mut stats = self.stats.lock();
        stats.tx_packets += 1;
        stats.tx_bytes += frame_len as u64;
    }

    /// Account for an asynchronous host-controller transmit failure.
    ///
    /// # Arguments
    ///
    /// * `error` - Static diagnostic supplied by the host-controller driver.
    pub(crate) fn handle_transmit_error(&self, error: &'static str) {
        self.stats.lock().tx_errors += 1;
        crate::println!(
            "[usb-ncm] Transmit failed on {}: {}",
            self.interface_name,
            error
        );
    }

    /// Process a CDC class notification received from the control interface.
    ///
    /// # Arguments
    ///
    /// * `notification` - Complete notification header and optional payload.
    pub(crate) fn handle_notification(&self, notification: &[u8]) {
        const CDC_NOTIFICATION_HEADER_LENGTH: usize = 8;
        const CDC_NOTIFY_NETWORK_CONNECTION: u8 = 0x00;
        const CDC_NOTIFY_SPEED_CHANGE: u8 = 0x2a;

        if notification.len() < CDC_NOTIFICATION_HEADER_LENGTH || notification[0] != 0xa1 {
            crate::println!("[usb-ncm] Ignoring malformed CDC notification");
            return;
        }

        match notification[1] {
            CDC_NOTIFY_NETWORK_CONNECTION => {
                let connected = u16::from_le_bytes([notification[2], notification[3]]) != 0;
                self.link_up.store(connected, Ordering::Release);
                crate::println!(
                    "[usb-ncm] Interface {} link {}",
                    self.interface_name,
                    if connected { "up" } else { "down" }
                );
            }
            CDC_NOTIFY_SPEED_CHANGE if notification.len() >= 16 => {
                let downlink = u32::from_le_bytes([
                    notification[8],
                    notification[9],
                    notification[10],
                    notification[11],
                ]);
                let uplink = u32::from_le_bytes([
                    notification[12],
                    notification[13],
                    notification[14],
                    notification[15],
                ]);
                crate::println!(
                    "[usb-ncm] Interface {} speed down={}bps up={}bps",
                    self.interface_name,
                    downlink,
                    uplink
                );
            }
            _ => {}
        }
    }

    /// Mark the USB function unavailable after a disconnect.
    pub(crate) fn disconnect(&self) {
        self.link_up.store(false, Ordering::Release);
        self.active.store(false, Ordering::Release);
    }

    /// Return the dynamically assigned Scarlet interface name.
    pub(crate) fn registered_interface_name(&self) -> &str {
        &self.interface_name
    }

    /// Return whether the USB function is still attached.
    pub(crate) fn is_attached(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
}

impl Device for CdcNcmDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Network
    }

    fn name(&self) -> &'static str {
        "cdc-ncm"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_network_device(&self) -> Option<&dyn NetworkDevice> {
        Some(self)
    }
}

impl ControlOps for CdcNcmDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations are not supported by CDC-NCM")
    }
}

impl MemoryMappingOps for CdcNcmDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Memory mapping is not supported by CDC-NCM")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for CdcNcmDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ns: Option<u64>,
        _min_wait_ns: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

impl NetworkDevice for CdcNcmDevice {
    fn get_interface_name(&self) -> &'static str {
        "cdc-ncm"
    }

    fn get_mac_address(&self) -> Result<MacAddress, &'static str> {
        Ok(self.mac_address)
    }

    fn get_mtu(&self) -> Result<usize, &'static str> {
        Ok(self.mtu)
    }

    fn get_interface_config(&self) -> Result<NetworkInterfaceConfig, &'static str> {
        Ok(NetworkInterfaceConfig::new(self.mac_address, self.mtu, "cdc-ncm").with_multicast())
    }

    fn send_packet(&self, packet: DevicePacket) -> Result<(), &'static str> {
        if !self.is_link_up() {
            return Err("CDC-NCM link is down");
        }
        if packet.len > packet.data.len() {
            return Err("CDC-NCM packet length exceeds its buffer");
        }

        let sequence = {
            let mut next_sequence = self.tx_sequence.lock();
            let sequence = *next_sequence;
            *next_sequence = next_sequence.wrapping_add(1);
            sequence
        };
        let ntb = build_ntb16(packet.as_slice(), sequence, self.tx_config)?;
        let result = self.transport.enqueue_ntb(ntb, packet.len);
        if result.is_err() {
            let mut stats = self.stats.lock();
            stats.tx_errors += 1;
            stats.dropped += 1;
        }
        result
    }

    fn receive_packets(&self) -> Result<Vec<DevicePacket>, &'static str> {
        Ok(Vec::new())
    }

    fn set_promiscuous_mode(&self, enabled: bool) -> Result<(), &'static str> {
        let mut filter = DEFAULT_PACKET_FILTER;
        if enabled {
            filter |= CDC_PACKET_TYPE_PROMISCUOUS;
        }
        self.transport.set_packet_filter(filter)?;
        self.promiscuous.store(enabled, Ordering::Release);
        Ok(())
    }

    fn init_network(&mut self) -> Result<(), &'static str> {
        self.active.store(true, Ordering::Release);
        self.link_up.store(true, Ordering::Release);
        Ok(())
    }

    fn is_link_up(&self) -> bool {
        self.active.load(Ordering::Acquire) && self.link_up.load(Ordering::Acquire)
    }

    fn get_stats(&self) -> NetworkStats {
        self.stats.lock().clone()
    }
}

impl EthernetDevice for CdcNcmDevice {
    fn mac_address(&self) -> Result<MacAddress, &'static str> {
        Ok(self.mac_address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ax88179a_ncm_configuration() -> Vec<u8> {
        vec![
            9, 2, 86, 0, 2, 2, 0, 0xa0, 50, // configuration
            9, 4, 0, 0, 1, 2, 13, 0, 0, // control interface
            5, 0x24, 0, 0x10, 0x01, // CDC header
            5, 0x24, 6, 0, 1, // CDC union
            13, 0x24, 0x0f, 5, 0, 0, 0, 0, 0xea, 0x05, 0, 0, 0, // Ethernet
            6, 0x24, 0x1a, 0, 1, 0x2b, // NCM
            7, 5, 0x81, 3, 16, 0, 11, // notification endpoint
            9, 4, 1, 0, 0, 10, 0, 1, 0, // inactive data alternate
            9, 4, 1, 1, 2, 10, 0, 1, 0, // active data alternate
            7, 5, 0x82, 2, 0, 2, 0, // bulk IN
            7, 5, 0x03, 2, 0, 2, 0, // bulk OUT
        ]
    }

    fn parameters() -> CdcNcmParameters {
        CdcNcmParameters {
            formats_supported: 1,
            ntb_in_max_size: 32_768,
            ndp_in_divisor: 4,
            ndp_in_payload_remainder: 0,
            ndp_in_alignment: 4,
            ntb_out_max_size: 32_768,
            ndp_out_divisor: 4,
            ndp_out_payload_remainder: 0,
            ndp_out_alignment: 4,
            ntb_out_max_datagrams: 56,
        }
    }

    #[test_case]
    fn parses_ax88179a_standard_ncm_configuration() {
        let config = parse_configuration(&ax88179a_ncm_configuration()).unwrap();
        assert_eq!(config.configuration_value, 2);
        assert_eq!(config.control_interface, 0);
        assert_eq!(config.data_interface, 1);
        assert_eq!(config.data_alternate_setting, 1);
        assert_eq!(config.notification_endpoint, 0x81);
        assert_eq!(config.notification_max_packet_size, 16);
        assert_eq!(config.notification_interval, 11);
        assert_eq!(config.bulk_in_endpoint, 0x82);
        assert_eq!(config.bulk_out_endpoint, 0x03);
        assert_eq!(config.bulk_in_max_packet_size, 512);
        assert_eq!(config.bulk_out_max_packet_size, 512);
        assert_eq!(config.mac_string_index, 5);
        assert_eq!(config.max_segment_size, 1514);
        assert_eq!(config.network_capabilities, 0x2b);
    }

    #[test_case]
    fn parses_ax88179a_mac_string() {
        let mac = parse_mac_address("F8E43B8C42CA").unwrap();
        assert_eq!(mac.as_bytes(), &[0xf8, 0xe4, 0x3b, 0x8c, 0x42, 0xca]);
        assert!(parse_mac_address("ff:ff:ff:ff:ff:ff").is_err());
        assert!(parse_mac_address("not-a-mac").is_err());
    }

    #[test_case]
    fn builds_legacy_and_extended_ntb_input_sizes() {
        assert_eq!(
            ntb_input_size_payload(16_384, None),
            16_384u32.to_le_bytes()
        );
        assert_eq!(
            ntb_input_size_payload(16_384, Some(64)),
            [0x00, 0x40, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00]
        );
    }

    #[test_case]
    fn selects_conservative_ntb_transfer_sizes() {
        let mut values = parameters();
        assert_eq!(values.receive_size(), NCM_DEFAULT_NTB_SIZE);
        assert_eq!(values.transmit_size(), NCM_DEFAULT_NTB_SIZE);

        values.ntb_out_max_size = 4_096;
        assert_eq!(values.transmit_size(), 4_096);
        values.ntb_out_max_size = 0;
        assert_eq!(values.transmit_size(), NCM_DEFAULT_NTB_SIZE);
    }

    #[test_case]
    fn ntb16_single_frame_round_trip() {
        let frame = vec![0x5a; 1514];
        let config = Ntb16TxConfig::new(parameters(), 1514, 512).unwrap();
        let ntb = build_ntb16(&frame, 7, config).unwrap();
        let packets = parse_ntb16(&ntb, 1514).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].as_slice(), frame.as_slice());
        assert_eq!(read_u16(&ntb, 6).unwrap(), 7);
        assert_ne!(ntb.len() % 512, 0);
    }

    #[test_case]
    fn rejects_truncated_ntb16_datagram() {
        let frame = vec![0x5a; 64];
        let config = Ntb16TxConfig::new(parameters(), 1514, 512).unwrap();
        let mut ntb = build_ntb16(&frame, 0, config).unwrap();
        let ndp = usize::from(read_u16(&ntb, 10).unwrap());
        write_u16(&mut ntb, ndp + 10, 0xffff).unwrap();
        assert!(parse_ntb16(&ntb, 1514).is_err());
    }

    #[test_case]
    fn accepts_sixty_four_datagrams_in_one_ntb16() {
        let frame = [0x5a; ETHERNET_HEADER_LENGTH];
        let ndp_offset = NTH16_LENGTH;
        let ndp_length = 8 + (NCM_MAX_DATAGRAMS_PER_NTB + 1) * 4;
        let payload_offset = ndp_offset + ndp_length;
        let block_length = payload_offset + NCM_MAX_DATAGRAMS_PER_NTB * frame.len();
        let mut ntb = vec![0u8; block_length];

        write_u32(&mut ntb, 0, NTH16_SIGNATURE).unwrap();
        write_u16(&mut ntb, 4, NTH16_LENGTH as u16).unwrap();
        write_u16(&mut ntb, 8, block_length as u16).unwrap();
        write_u16(&mut ntb, 10, ndp_offset as u16).unwrap();
        write_u32(&mut ntb, ndp_offset, NDP16_NO_CRC_SIGNATURE).unwrap();
        write_u16(&mut ntb, ndp_offset + 4, ndp_length as u16).unwrap();

        for index in 0..NCM_MAX_DATAGRAMS_PER_NTB {
            let entry_offset = ndp_offset + 8 + index * 4;
            let frame_offset = payload_offset + index * frame.len();
            write_u16(&mut ntb, entry_offset, frame_offset as u16).unwrap();
            write_u16(&mut ntb, entry_offset + 2, frame.len() as u16).unwrap();
            ntb[frame_offset..frame_offset + frame.len()].copy_from_slice(&frame);
        }

        assert_eq!(
            parse_ntb16(&ntb, ETHERNET_HEADER_LENGTH).unwrap().len(),
            NCM_MAX_DATAGRAMS_PER_NTB
        );
    }

    #[test_case]
    fn usb_ncm_regression_releases_rx_queue_lock_before_dispatching_packet() {
        let queue = IrqSpinLock::new(VecDeque::new());
        queue.lock().push_back(QueuedRxPacket {
            interface_name: String::from("usbnet-test"),
            packet: DevicePacket::with_data(vec![0; ETHERNET_HEADER_LENGTH]),
        });

        let mut dispatched = false;
        let processed = drain_queued_rx_packets(&queue, 1, |_| {
            dispatched = true;
            assert!(
                queue.try_lock().is_some(),
                "CDC-NCM RX queue lock leaked into packet dispatch"
            );
        });
        assert_eq!(processed, 1);
        assert!(dispatched);
    }

    #[test_case]
    fn usb_ncm_rx_worker_bounds_each_dispatch_batch() {
        let queue = IrqSpinLock::new(VecDeque::new());
        for _ in 0..2 {
            queue.lock().push_back(QueuedRxPacket {
                interface_name: String::from("usbnet-test"),
                packet: DevicePacket::with_data(vec![0; ETHERNET_HEADER_LENGTH]),
            });
        }

        let processed = drain_queued_rx_packets(&queue, 1, |_| {});
        assert_eq!(processed, 1);
        assert_eq!(queue.lock().len(), 1);
    }
}
