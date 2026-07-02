#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use scarlet::sync::Mutex;

use scarlet::device::events::InterruptCapableDevice;
use scarlet::device::gpio::GpioIrqTrigger;
use scarlet::device::input::event_device::EventDevice;
use scarlet::device::input::event_types::{EV_KEY, EV_REL, EV_SYN};
use scarlet::device::input::key_codes::BTN_LEFT;
use scarlet::device::input::key_values::{KEY_PRESS, KEY_RELEASE};
use scarlet::device::input::rel_codes::{REL_X, REL_Y};
use scarlet::device::input::syn_codes::SYN_REPORT;
use scarlet::device::manager::{DeviceManager, DriverPriority, probe_defer};
use scarlet::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use scarlet::device::spi::{SpiBus, SpiError, SpiTransfer};
use scarlet::early_println;
use scarlet::interrupt::{InterruptError, InterruptId, InterruptResult};
use scarlet::time::udelay;

const PACKET_SIZE: usize = 256;
const MAX_PAYLOAD: usize = 246;

const DEVICE_KEYBOARD: u8 = 0x01;
const DEVICE_TRACKPAD: u8 = 0x02;
const DEVICE_INFO: u8 = 0xd0;

const PACKET_READ: u8 = 0x20;
const PACKET_WRITE: u8 = 0x40;
const MSG_REPORT: u8 = 0x10;
const MSG_HEADER_SIZE: usize = 8;

const FLAG_WRITE: u8 = 0x40;

const BOOT_PACKET: [u8; 4] = [0xa0, 0x80, 0x00, 0x00];
const STATUS_OK: u32 = 0xd56827ac;
const SPI_DELAY_US: u64 = 200;
const SPI_READ_PRE_DELAY_US: u64 = 100;
const SPI_READ_CS_HOLD_US: u64 = 100;
const SPI_READ_POST_DELAY_US: u64 = 250;
const SPI_WRITE_STATUS_DELAY_US: u64 = 200;
const BOOT_WAIT_ATTEMPTS: usize = 100;
const BOOT_WAIT_STEP_US: u64 = 10_000;
const REQUEST_WAIT_ATTEMPTS: usize = 10;
const REQUEST_WAIT_STEP_US: u64 = 10_000;
const SPIHID_DESC_MAX: u16 = 512;
const MAX_IRQ_DRAIN: usize = 8;
const IRQ_LOG_LIMIT: u32 = 16;
const REP_DELAY_US: u64 = 250_000;
const REP_PERIOD_US: u64 = 33_000;
const SPI_HID_VERBOSE_TRACE: bool = false;
const PACKET_LOG_LIMIT: u32 = 32;
const REPORT_LOG_LIMIT: u32 = 16;

static IRQ_LOGS: AtomicU32 = AtomicU32::new(0);
static PACKET_LOGS: AtomicU32 = AtomicU32::new(0);
static REPORT_LOGS: AtomicU32 = AtomicU32::new(0);

const KEY_0: u16 = 11;
const KEY_1: u16 = 2;
const KEY_2: u16 = 3;
const KEY_3: u16 = 4;
const KEY_4: u16 = 5;
const KEY_5: u16 = 6;
const KEY_6: u16 = 7;
const KEY_7: u16 = 8;
const KEY_8: u16 = 9;
const KEY_9: u16 = 10;
const KEY_MINUS: u16 = 12;
const KEY_EQUAL: u16 = 13;

const KEY_A: u16 = 30;
const KEY_B: u16 = 48;
const KEY_C: u16 = 46;
const KEY_D: u16 = 32;
const KEY_E: u16 = 18;
const KEY_F: u16 = 33;
const KEY_G: u16 = 34;
const KEY_H: u16 = 35;
const KEY_I: u16 = 23;
const KEY_J: u16 = 36;
const KEY_K: u16 = 37;
const KEY_L: u16 = 38;
const KEY_M: u16 = 50;
const KEY_N: u16 = 49;
const KEY_O: u16 = 24;
const KEY_P: u16 = 25;
const KEY_Q: u16 = 16;
const KEY_R: u16 = 19;
const KEY_S: u16 = 31;
const KEY_T: u16 = 20;
const KEY_U: u16 = 22;
const KEY_V: u16 = 47;
const KEY_W: u16 = 17;
const KEY_X: u16 = 45;
const KEY_Y: u16 = 21;
const KEY_Z: u16 = 44;

const KEY_ENTER: u16 = 28;
const KEY_ESC: u16 = 1;
const KEY_BACKSPACE: u16 = 14;
const KEY_TAB: u16 = 15;
const KEY_SPACE: u16 = 57;
const KEY_LEFTBRACE: u16 = 26;
const KEY_RIGHTBRACE: u16 = 27;
const KEY_SEMICOLON: u16 = 39;
const KEY_APOSTROPHE: u16 = 40;
const KEY_GRAVE: u16 = 41;
const KEY_BACKSLASH: u16 = 43;
const KEY_COMMA: u16 = 51;
const KEY_DOT: u16 = 52;
const KEY_SLASH: u16 = 53;
const KEY_CAPSLOCK: u16 = 58;

const KEY_F1: u16 = 59;
const KEY_F2: u16 = 60;
const KEY_F3: u16 = 61;
const KEY_F4: u16 = 62;
const KEY_F5: u16 = 63;
const KEY_F6: u16 = 64;
const KEY_F7: u16 = 65;
const KEY_F8: u16 = 66;
const KEY_F9: u16 = 67;
const KEY_F10: u16 = 68;
const KEY_F11: u16 = 87;
const KEY_F12: u16 = 88;

const KEY_LEFTCTRL: u16 = 29;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_LEFTALT: u16 = 56;
const KEY_LEFTMETA: u16 = 125;
const KEY_RIGHTCTRL: u16 = 97;
const KEY_RIGHTSHIFT: u16 = 54;
const KEY_RIGHTALT: u16 = 100;
const KEY_RIGHTMETA: u16 = 126;

const KEY_RIGHT: u16 = 106;
const KEY_LEFT: u16 = 105;
const KEY_DOWN: u16 = 108;
const KEY_UP: u16 = 103;

fn hid_usage_to_key(usage: u8) -> Option<u16> {
    match usage {
        0x04 => Some(KEY_A),
        0x05 => Some(KEY_B),
        0x06 => Some(KEY_C),
        0x07 => Some(KEY_D),
        0x08 => Some(KEY_E),
        0x09 => Some(KEY_F),
        0x0a => Some(KEY_G),
        0x0b => Some(KEY_H),
        0x0c => Some(KEY_I),
        0x0d => Some(KEY_J),
        0x0e => Some(KEY_K),
        0x0f => Some(KEY_L),
        0x10 => Some(KEY_M),
        0x11 => Some(KEY_N),
        0x12 => Some(KEY_O),
        0x13 => Some(KEY_P),
        0x14 => Some(KEY_Q),
        0x15 => Some(KEY_R),
        0x16 => Some(KEY_S),
        0x17 => Some(KEY_T),
        0x18 => Some(KEY_U),
        0x19 => Some(KEY_V),
        0x1a => Some(KEY_W),
        0x1b => Some(KEY_X),
        0x1c => Some(KEY_Y),
        0x1d => Some(KEY_Z),
        0x1e => Some(KEY_1),
        0x1f => Some(KEY_2),
        0x20 => Some(KEY_3),
        0x21 => Some(KEY_4),
        0x22 => Some(KEY_5),
        0x23 => Some(KEY_6),
        0x24 => Some(KEY_7),
        0x25 => Some(KEY_8),
        0x26 => Some(KEY_9),
        0x27 => Some(KEY_0),
        0x28 => Some(KEY_ENTER),
        0x29 => Some(KEY_ESC),
        0x2a => Some(KEY_BACKSPACE),
        0x2b => Some(KEY_TAB),
        0x2c => Some(KEY_SPACE),
        0x2d => Some(KEY_MINUS),
        0x2e => Some(KEY_EQUAL),
        0x2f => Some(KEY_LEFTBRACE),
        0x30 => Some(KEY_RIGHTBRACE),
        0x31 => Some(KEY_BACKSLASH),
        0x33 => Some(KEY_SEMICOLON),
        0x34 => Some(KEY_APOSTROPHE),
        0x35 => Some(KEY_GRAVE),
        0x36 => Some(KEY_COMMA),
        0x37 => Some(KEY_DOT),
        0x38 => Some(KEY_SLASH),
        0x39 => Some(KEY_CAPSLOCK),
        0x3a => Some(KEY_F1),
        0x3b => Some(KEY_F2),
        0x3c => Some(KEY_F3),
        0x3d => Some(KEY_F4),
        0x3e => Some(KEY_F5),
        0x3f => Some(KEY_F6),
        0x40 => Some(KEY_F7),
        0x41 => Some(KEY_F8),
        0x42 => Some(KEY_F9),
        0x43 => Some(KEY_F10),
        0x44 => Some(KEY_F11),
        0x45 => Some(KEY_F12),
        0x4f => Some(KEY_RIGHT),
        0x50 => Some(KEY_LEFT),
        0x51 => Some(KEY_DOWN),
        0x52 => Some(KEY_UP),
        0xe0 => Some(KEY_LEFTCTRL),
        0xe1 => Some(KEY_LEFTSHIFT),
        0xe2 => Some(KEY_LEFTALT),
        0xe3 => Some(KEY_LEFTMETA),
        0xe4 => Some(KEY_RIGHTCTRL),
        0xe5 => Some(KEY_RIGHTSHIFT),
        0xe6 => Some(KEY_RIGHTALT),
        0xe7 => Some(KEY_RIGHTMETA),
        _ => None,
    }
}

fn irq_trigger_from_dt_flags(flags: u32) -> GpioIrqTrigger {
    match flags {
        0x01 => GpioIrqTrigger::RisingEdge,
        0x02 => GpioIrqTrigger::FallingEdge,
        0x04 => GpioIrqTrigger::HighLevel,
        0x08 => GpioIrqTrigger::LowLevel,
        _ => GpioIrqTrigger::FallingEdge,
    }
}

pub struct AppleSpiHidTransport {
    spi_bus: Arc<dyn SpiBus>,
    cs: u8,
    max_freq: u32,
    keyboard_event: Arc<EventDevice>,
    trackpad_event: Arc<EventDevice>,
    spien_gpio: (u32, Option<Arc<dyn scarlet::device::gpio::GpioController>>),
    irq_gpio: (u32, Option<Arc<dyn scarlet::device::gpio::GpioController>>),
    irq_id: Option<InterruptId>,
    irq_trigger: GpioIrqTrigger,
    msg_id: Mutex<u8>,
    booted: Mutex<bool>,
    ready: Mutex<bool>,
    last_modifiers: Mutex<u8>,
    last_keys: Mutex<[u8; 6]>,
    last_buttons: Mutex<u8>,
    last_touch: Mutex<Option<(i16, i16)>>,
    key_press_time: Mutex<u64>,
    last_repeat_time: Mutex<u64>,
}

impl AppleSpiHidTransport {
    fn new(
        spi_bus: Arc<dyn SpiBus>,
        cs: u8,
        max_freq: u32,
        keyboard_event: Arc<EventDevice>,
        trackpad_event: Arc<EventDevice>,
        spien_gpio: (u32, Option<Arc<dyn scarlet::device::gpio::GpioController>>),
        irq_gpio: (u32, Option<Arc<dyn scarlet::device::gpio::GpioController>>),
        irq_trigger: GpioIrqTrigger,
    ) -> Self {
        Self {
            spi_bus,
            cs,
            max_freq,
            keyboard_event,
            trackpad_event,
            spien_gpio,
            irq_gpio,
            irq_id: None,
            irq_trigger,
            msg_id: Mutex::new(0),
            booted: Mutex::new(false),
            ready: Mutex::new(false),
            last_modifiers: Mutex::new(0),
            last_keys: Mutex::new([0; 6]),
            last_buttons: Mutex::new(0),
            last_touch: Mutex::new(None),
            key_press_time: Mutex::new(0),
            last_repeat_time: Mutex::new(0),
        }
    }

    fn build_packet(
        flags: u8,
        device_id: u8,
        offset: u16,
        remain: u16,
        length: u16,
        data: &[u8],
    ) -> [u8; PACKET_SIZE] {
        let mut packet = [0u8; PACKET_SIZE];
        let payload_len = core::cmp::min(data.len(), MAX_PAYLOAD);
        packet[0] = flags;
        packet[1] = device_id;
        packet[2..4].copy_from_slice(&offset.to_le_bytes());
        packet[4..6].copy_from_slice(&remain.to_le_bytes());
        packet[6..8].copy_from_slice(&length.to_le_bytes());
        packet[8..8 + payload_len].copy_from_slice(&data[..payload_len]);
        let crc = Self::crc16(&packet[..254]);
        packet[254..256].copy_from_slice(&crc.to_le_bytes());
        packet
    }

    fn crc16(data: &[u8]) -> u16 {
        let mut crc = 0u16;
        for byte in data {
            crc ^= *byte as u16;
            for _ in 0..8 {
                if (crc & 1) != 0 {
                    crc = (crc >> 1) ^ 0xa001;
                } else {
                    crc >>= 1;
                }
            }
        }
        crc
    }

    fn send_command_packet(&self, packet: &mut [u8; PACKET_SIZE]) -> Result<(), SpiError> {
        let crc = Self::crc16(&packet[..254]);
        packet[254..256].copy_from_slice(&crc.to_le_bytes());
        let mut write = SpiTransfer::write(self.cs, packet);
        write.speed_hz = self.max_freq;
        write.delay_after_us = SPI_WRITE_STATUS_DELAY_US;
        let mut read_status = SpiTransfer::read(self.cs, 4);
        read_status.speed_hz = self.max_freq;
        let mut transfers = [write, read_status];
        self.spi_bus.transfer(&mut transfers)?;
        udelay(SPI_DELAY_US);

        let status = u32::from_le_bytes([
            transfers[1].data[0],
            transfers[1].data[1],
            transfers[1].data[2],
            transfers[1].data[3],
        ]);
        if status != STATUS_OK {
            early_println!(
                "apple-spi-hid: status mismatch after write: {:#010x}",
                status
            );
            return Err(SpiError::NoResponse);
        }

        Ok(())
    }

    fn recv_packet(&self) -> Result<[u8; PACKET_SIZE], SpiError> {
        let mut seg = SpiTransfer::read(self.cs, PACKET_SIZE);
        seg.speed_hz = self.max_freq;
        seg.delay_before_us = SPI_READ_PRE_DELAY_US;
        seg.delay_after_us = SPI_READ_CS_HOLD_US;
        if let Err(err) = self.spi_bus.transfer(core::slice::from_mut(&mut seg)) {
            if PACKET_LOGS.fetch_add(1, Ordering::Relaxed) < PACKET_LOG_LIMIT {
                early_println!(
                    "apple-spi-hid: read packet transfer failed: {:?} irq_value={:?} irq_active={} bus_speed={}",
                    err,
                    self.irq_line_value(),
                    self.irq_line_active(),
                    self.spi_bus.bus_speed()
                );
            }
            return Err(err);
        }
        udelay(SPI_READ_POST_DELAY_US);
        let mut packet = [0u8; PACKET_SIZE];
        packet.copy_from_slice(&seg.data);
        Ok(packet)
    }

    fn recv_packet_message(&self) -> Result<Option<(u8, u8, Vec<u8>)>, SpiError> {
        let mut message = Vec::new();
        let mut flags = 0;
        let mut device = 0;
        loop {
            let packet = self.recv_packet()?;
            let expected_crc = Self::crc16(&packet[..254]);
            let packet_crc = u16::from_le_bytes([packet[254], packet[255]]);
            if SPI_HID_VERBOSE_TRACE
                && PACKET_LOGS.fetch_add(1, Ordering::Relaxed) < PACKET_LOG_LIMIT
            {
                early_println!(
                    "apple-spi-hid: packet flags={:#x} dev={:#x} offset={} remain={} len={} crc_ok={} data={:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                    packet[0],
                    packet[1],
                    u16::from_le_bytes([packet[2], packet[3]]),
                    u16::from_le_bytes([packet[4], packet[5]]),
                    u16::from_le_bytes([packet[6], packet[7]]),
                    expected_crc == packet_crc,
                    packet[8],
                    packet[9],
                    packet[10],
                    packet[11],
                    packet[12],
                    packet[13],
                    packet[14],
                    packet[15]
                );
            }
            if expected_crc != packet_crc {
                early_println!(
                    "apple-spi-hid: packet crc mismatch flags={:#x} dev={:#x} expected={:#06x} got={:#06x}",
                    packet[0],
                    packet[1],
                    expected_crc,
                    packet_crc
                );
                return Err(SpiError::BusError);
            }

            if message.is_empty() {
                flags = packet[0];
                device = packet[1];
            } else if flags != packet[0] || device != packet[1] {
                return Err(SpiError::BusError);
            }

            let length = u16::from_le_bytes([packet[6], packet[7]]) as usize;
            let remain = u16::from_le_bytes([packet[4], packet[5]]) as usize;
            let offset = u16::from_le_bytes([packet[2], packet[3]]) as usize;
            if length > MAX_PAYLOAD {
                return Err(SpiError::InvalidArg);
            }
            if offset != message.len() {
                return Err(SpiError::BusError);
            }

            message.extend_from_slice(&packet[8..8 + length]);
            if remain == 0 {
                break;
            }
        }

        if message.is_empty() {
            Ok(None)
        } else {
            Ok(Some((flags, device, message)))
        }
    }

    fn verify_message_crc(message: &[u8]) -> bool {
        if message.len() < 2 {
            return false;
        }

        let data_len = message.len() - 2;
        let expected = Self::crc16(&message[..data_len]);
        let found = u16::from_le_bytes([message[data_len], message[data_len + 1]]);
        expected == found
    }

    fn next_msg_id(&self) -> u8 {
        let mut msg_id = self.msg_id.lock();
        let id = *msg_id;
        *msg_id = (*msg_id).wrapping_add(1);
        id
    }

    fn mark_booted(&self) {
        let mut booted = self.booted.lock();
        if !*booted {
            early_println!("apple-spi-hid: boot packet received");
            *booted = true;
        }
    }

    fn is_booted(&self) -> bool {
        *self.booted.lock()
    }

    fn is_ready(&self) -> bool {
        *self.ready.lock()
    }

    fn disable_device_irq(&self) {
        if let Some(ref gpio) = self.irq_gpio.1 {
            gpio.disable_irq(self.irq_gpio.0);
        }
    }

    fn enable_device_irq(&self) {
        if let Some(ref gpio) = self.irq_gpio.1 {
            gpio.enable_irq(self.irq_gpio.0, self.irq_trigger);
        }
    }

    fn irq_line_active(&self) -> bool {
        let Some(ref gpio) = self.irq_gpio.1 else {
            return false;
        };

        let value = gpio.get_value(self.irq_gpio.0);
        match self.irq_trigger {
            GpioIrqTrigger::HighLevel => value,
            GpioIrqTrigger::LowLevel => !value,
            GpioIrqTrigger::RisingEdge | GpioIrqTrigger::FallingEdge => false,
        }
    }

    fn irq_line_value(&self) -> Option<bool> {
        self.irq_gpio
            .1
            .as_ref()
            .map(|gpio| gpio.get_value(self.irq_gpio.0))
    }

    fn wait_for_irq_line(&self, attempts: usize, step_us: u64) -> bool {
        for _ in 0..attempts {
            if self.irq_line_active() {
                return true;
            }
            udelay(step_us);
        }

        false
    }

    fn wait_for_boot_packet(&self) -> Result<(), SpiError> {
        for _ in 0..BOOT_WAIT_ATTEMPTS {
            if !self.wait_for_irq_line(1, BOOT_WAIT_STEP_US) {
                continue;
            }

            let Some((flags, device, message)) = self.recv_packet_message()? else {
                udelay(BOOT_WAIT_STEP_US);
                continue;
            };

            if message == BOOT_PACKET {
                self.mark_booted();
                return Ok(());
            }

            if PACKET_LOGS.fetch_add(1, Ordering::Relaxed) < PACKET_LOG_LIMIT {
                early_println!(
                    "apple-spi-hid: unexpected packet while waiting boot flags={:#x} dev={:#x} len={}",
                    flags,
                    device,
                    message.len()
                );
            }
            udelay(BOOT_WAIT_STEP_US);
        }

        Err(SpiError::Timeout)
    }

    fn send_request(
        &self,
        target: u8,
        unknown0: u8,
        unknown1: u8,
        unknown2: u8,
        response_len: u16,
        payload: &[u8],
    ) -> Result<Vec<u8>, SpiError> {
        let message_len = MSG_HEADER_SIZE + payload.len() + 2;
        if message_len > MAX_PAYLOAD {
            return Err(SpiError::InvalidArg);
        }

        let mut message = alloc::vec![0u8; message_len];
        message[0] = unknown0;
        message[1] = unknown1;
        message[2] = unknown2;
        message[3] = self.next_msg_id();
        message[4..6].copy_from_slice(&response_len.to_le_bytes());
        message[6..8].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        message[MSG_HEADER_SIZE..MSG_HEADER_SIZE + payload.len()].copy_from_slice(payload);

        let crc = Self::crc16(&message[..MSG_HEADER_SIZE + payload.len()]);
        message[MSG_HEADER_SIZE + payload.len()..message_len].copy_from_slice(&crc.to_le_bytes());

        let mut packet = Self::build_packet(FLAG_WRITE, target, 0, 0, message_len as u16, &message);
        self.send_command_packet(&mut packet)?;

        for _ in 0..REQUEST_WAIT_ATTEMPTS {
            if !self.wait_for_irq_line(1, REQUEST_WAIT_STEP_US) {
                continue;
            }

            if let Some((flags, device, response)) = self.recv_packet_message()? {
                if response == BOOT_PACKET {
                    self.mark_booted();
                    continue;
                }
                if flags != PACKET_WRITE || device != target {
                    continue;
                }
                if response.len() < MSG_HEADER_SIZE + 2 || !Self::verify_message_crc(&response) {
                    return Err(SpiError::BusError);
                }

                let response_payload_len = u16::from_le_bytes([response[6], response[7]]) as usize;
                if response_payload_len + MSG_HEADER_SIZE + 2 > response.len() {
                    return Err(SpiError::InvalidArg);
                }
                if response[0] == unknown0 && response[1] == unknown1 && response[2] == unknown2 {
                    return Ok(
                        response[MSG_HEADER_SIZE..MSG_HEADER_SIZE + response_payload_len].to_vec(),
                    );
                }
            }
            udelay(REQUEST_WAIT_STEP_US);
        }

        Err(SpiError::Timeout)
    }

    fn power_on(&self) -> Result<(), &'static str> {
        if let Some(ref gpio) = self.spien_gpio.1 {
            let pin = self.spien_gpio.0;
            early_println!(
                "apple-spi-hid: spien before reset pin={} value={}",
                pin,
                gpio.get_value(pin)
            );
            gpio.set_direction_output(pin, true);
            early_println!(
                "apple-spi-hid: spien asserted pin={} value={}",
                pin,
                gpio.get_value(pin)
            );
            udelay(5_000);
            gpio.set_value(pin, false);
            early_println!(
                "apple-spi-hid: spien deasserted pin={} value={}",
                pin,
                gpio.get_value(pin)
            );
            udelay(5_000);
            gpio.set_value(pin, true);
            early_println!(
                "apple-spi-hid: spien reasserted pin={} value={}",
                pin,
                gpio.get_value(pin)
            );
        } else {
            return Err("apple-spi-hid: no GPIO controller for SPIEN");
        }
        udelay(50_000);

        Ok(())
    }

    fn finish_initialization(&self) -> Result<(), SpiError> {
        if self.is_ready() {
            return Ok(());
        }
        if !self.is_booted() {
            return Err(SpiError::NoResponse);
        }

        self.finish_initialization_requests()?;
        *self.ready.lock() = true;
        early_println!("apple-spi-hid: initialized");
        Ok(())
    }

    fn finish_initialization_requests(&self) -> Result<(), SpiError> {
        let device_info = self.send_request(DEVICE_INFO, 0x20, 0x01, DEVICE_INFO, 0, &[])?;
        let num_devices = if device_info.len() >= 6 {
            u16::from_le_bytes([device_info[4], device_info[5]]) as usize
        } else {
            3
        };
        let num_devices = core::cmp::min(num_devices, 3);

        for device_id in 0..num_devices {
            let _ = self.send_request(
                DEVICE_INFO,
                0x20,
                0x02,
                device_id as u8,
                SPIHID_DESC_MAX,
                &[],
            );
        }

        for device_id in 1..num_devices {
            let _ = self.send_request(
                DEVICE_INFO,
                0x20,
                0x10,
                device_id as u8,
                SPIHID_DESC_MAX,
                &[],
            );
        }

        let _ = self.send_request(DEVICE_TRACKPAD, 0x52, 0x02, 0x00, 0, &[0x02, 0x01]);

        Ok(())
    }

    fn recv_report(&self) -> Result<Option<(u8, Vec<u8>)>, SpiError> {
        let Some((flags, device_id, message)) = self.recv_packet_message()? else {
            return Ok(None);
        };

        if flags != PACKET_READ {
            return Ok(None);
        }
        if message == BOOT_PACKET {
            self.mark_booted();
            return Ok(None);
        }
        if message.len() < MSG_HEADER_SIZE {
            return Err(SpiError::BusError);
        }
        if !Self::verify_message_crc(&message) {
            return Err(SpiError::BusError);
        }
        if message[0] != MSG_REPORT {
            return Ok(None);
        }

        let payload_len = u16::from_le_bytes([message[6], message[7]]) as usize;
        if payload_len + MSG_HEADER_SIZE + 2 > message.len() {
            return Err(SpiError::InvalidArg);
        }

        Ok(Some((
            device_id,
            message[MSG_HEADER_SIZE..MSG_HEADER_SIZE + payload_len].to_vec(),
        )))
    }

    fn emit_key_changes(&self, new_modifiers: u8, new_keys: [u8; 6]) {
        let mut old_modifiers = self.last_modifiers.lock();
        let mut old_keys = self.last_keys.lock();

        for (bit, usage) in [
            (0x01, 0xe0),
            (0x02, 0xe1),
            (0x04, 0xe2),
            (0x08, 0xe3),
            (0x10, 0xe4),
            (0x20, 0xe5),
            (0x40, 0xe6),
            (0x80, 0xe7),
        ] {
            let was_pressed = (*old_modifiers & bit) != 0;
            let is_pressed = (new_modifiers & bit) != 0;
            if was_pressed != is_pressed {
                if let Some(code) = hid_usage_to_key(usage) {
                    self.keyboard_event.push_event(
                        EV_KEY,
                        code,
                        if is_pressed { KEY_PRESS } else { KEY_RELEASE },
                    );
                }
            }
        }

        for usage in *old_keys {
            if usage != 0 && !new_keys.contains(&usage) {
                if let Some(code) = hid_usage_to_key(usage) {
                    self.keyboard_event.push_event(EV_KEY, code, KEY_RELEASE);
                }
            }
        }

        for usage in new_keys {
            if usage != 0 && !old_keys.contains(&usage) {
                if let Some(code) = hid_usage_to_key(usage) {
                    self.keyboard_event.push_event(EV_KEY, code, KEY_PRESS);
                }
            }
        }

        *old_modifiers = new_modifiers;
        *old_keys = new_keys;
        *self.key_press_time.lock() = scarlet::time::current_time();
        *self.last_repeat_time.lock() = 0;
    }

    fn maybe_repeat_keys(&self) {
        let modifiers = *self.last_modifiers.lock();
        let keys = self.last_keys.lock();

        let any_pressed = modifiers != 0 || keys.iter().any(|k| *k != 0);
        if !any_pressed {
            return;
        }

        let now = scarlet::time::current_time();
        let press_time = *self.key_press_time.lock();
        let mut last_repeat = self.last_repeat_time.lock();

        let next_repeat = if *last_repeat == 0 {
            press_time + REP_DELAY_US
        } else {
            *last_repeat + REP_PERIOD_US
        };

        if now < next_repeat {
            return;
        }

        for (bit, usage) in [
            (0x01u8, 0xe0u8),
            (0x02, 0xe1),
            (0x04, 0xe2),
            (0x08, 0xe3),
            (0x10, 0xe4),
            (0x20, 0xe5),
            (0x40, 0xe6),
            (0x80, 0xe7),
        ] {
            if modifiers & bit != 0 {
                if let Some(code) = hid_usage_to_key(usage) {
                    self.keyboard_event.push_event(EV_KEY, code, 2);
                }
            }
        }

        for usage in keys.iter() {
            if *usage != 0 {
                if let Some(code) = hid_usage_to_key(*usage) {
                    self.keyboard_event.push_event(EV_KEY, code, 2);
                }
            }
        }

        self.keyboard_event.push_event(EV_SYN, SYN_REPORT, 0);
        *last_repeat = now;
    }

    fn handle_keyboard_report(&self, report: &[u8]) {
        if report.len() < 10 {
            return;
        }

        let mut keys = [0u8; 6];
        keys.copy_from_slice(&report[3..9]);
        let modifiers = report[1];
        self.emit_key_changes(modifiers, keys);
        self.keyboard_event.push_event(EV_SYN, SYN_REPORT, 0);
    }

    fn handle_trackpad_report(&self, report: &[u8]) {
        // M1 SPI HID trackpad report format (from Asahi hid-magicmouse.c):
        //   tp_mouse_report (8 bytes): report_id, buttons, rel_x, rel_y, pad[4]
        //   tp_header (38 bytes): unknown[22], num_fingers, buttons, unknown3[14]
        //   tp_finger × N (30 bytes each)
        const TP_MOUSE_SIZE: usize = 8;
        const TP_HEADER_SIZE: usize = 38;
        const TP_FINGER_SIZE: usize = 30;
        const FINGER_ABS_X: usize = 4;
        const FINGER_ABS_Y: usize = 6;
        const FINGER_TOUCH_MAJOR: usize = 18;
        const HEADER_NUM_FINGERS: usize = TP_MOUSE_SIZE + 22;
        const HEADER_BUTTONS: usize = TP_MOUSE_SIZE + 23;
        const MOUSE_BUTTONS: usize = 1;
        const TOUCH_SCALE: i32 = 12;

        let base = TP_MOUSE_SIZE + TP_HEADER_SIZE;

        if report.len() < base {
            return;
        }

        let clicked = report[MOUSE_BUTTONS] != 0 || report[HEADER_BUTTONS] != 0;
        let buttons: u8 = if clicked { 0x01 } else { 0x00 };
        let finger_count = report[HEADER_NUM_FINGERS] as usize;

        let mut dx = 0i32;
        let mut dy = 0i32;

        if finger_count > 0 && report.len() >= base + TP_FINGER_SIZE {
            let finger = &report[base..base + TP_FINGER_SIZE];
            let touch_major =
                i16::from_le_bytes([finger[FINGER_TOUCH_MAJOR], finger[FINGER_TOUCH_MAJOR + 1]]);

            if SPI_HID_VERBOSE_TRACE
                && REPORT_LOGS.fetch_add(1, Ordering::Relaxed) < REPORT_LOG_LIMIT
            {
                let abs_x = i16::from_le_bytes([finger[FINGER_ABS_X], finger[FINGER_ABS_X + 1]]);
                let abs_y = i16::from_le_bytes([finger[FINGER_ABS_Y], finger[FINGER_ABS_Y + 1]]);
                early_println!(
                    "tp: nf={} tm={} ax={} ay={} f0={:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                    finger_count,
                    touch_major,
                    abs_x,
                    abs_y,
                    finger[0],
                    finger[1],
                    finger[2],
                    finger[3],
                    finger[4],
                    finger[5],
                    finger[6],
                    finger[7],
                    finger[8],
                    finger[9],
                    finger[10],
                    finger[11]
                );
            }

            if touch_major > 0 {
                let abs_x = i16::from_le_bytes([finger[FINGER_ABS_X], finger[FINGER_ABS_X + 1]]);
                let abs_y = i16::from_le_bytes([finger[FINGER_ABS_Y], finger[FINGER_ABS_Y + 1]]);
                let inverted_y = -abs_y;

                let mut last = self.last_touch.lock();
                if let Some((lx, ly)) = *last {
                    dx = (abs_x as i32 - lx as i32) / TOUCH_SCALE;
                    dy = (inverted_y as i32 - ly as i32) / TOUCH_SCALE;
                }
                *last = Some((abs_x, inverted_y));
            } else {
                let mut last = self.last_touch.lock();
                *last = None;
            }
        } else {
            let mut last = self.last_touch.lock();
            *last = None;
        }

        let mut old_buttons = self.last_buttons.lock();
        let was_pressed = (*old_buttons & 0x01) != 0;
        let is_pressed = (buttons & 0x01) != 0;
        if was_pressed != is_pressed {
            self.trackpad_event.push_event(
                EV_KEY,
                BTN_LEFT,
                if is_pressed { KEY_PRESS } else { KEY_RELEASE },
            );
        }

        if dx != 0 {
            self.trackpad_event.push_event(EV_REL, REL_X, dx);
        }
        if dy != 0 {
            self.trackpad_event.push_event(EV_REL, REL_Y, dy);
        }

        *old_buttons = buttons;
        self.trackpad_event.push_event(EV_SYN, SYN_REPORT, 0);
    }

    fn handle_input_report(&self, device_id: u8, report: &[u8]) {
        if SPI_HID_VERBOSE_TRACE && REPORT_LOGS.fetch_add(1, Ordering::Relaxed) < REPORT_LOG_LIMIT {
            early_println!(
                "apple-spi-hid: input report dev={} len={} data={:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                device_id,
                report.len(),
                report.first().copied().unwrap_or(0),
                report.get(1).copied().unwrap_or(0),
                report.get(2).copied().unwrap_or(0),
                report.get(3).copied().unwrap_or(0),
                report.get(4).copied().unwrap_or(0),
                report.get(5).copied().unwrap_or(0),
                report.get(6).copied().unwrap_or(0),
                report.get(7).copied().unwrap_or(0)
            );
        }
        match device_id {
            DEVICE_KEYBOARD => self.handle_keyboard_report(report),
            DEVICE_TRACKPAD => self.handle_trackpad_report(report),
            _ => {}
        }
    }

    fn service_pending_reads(&self) -> Result<(), SpiError> {
        for _ in 0..MAX_IRQ_DRAIN {
            if !self.irq_line_active() {
                break;
            }

            if let Some((device_id, report)) = self.recv_report()? {
                self.handle_input_report(device_id, &report);
            }
        }

        Ok(())
    }
}

impl InterruptCapableDevice for AppleSpiHidTransport {
    fn handle_interrupt(&self) -> InterruptResult<()> {
        let ready = self.is_ready();
        if SPI_HID_VERBOSE_TRACE && IRQ_LOGS.fetch_add(1, Ordering::Relaxed) < IRQ_LOG_LIMIT {
            early_println!(
                "apple-spi-hid: irq value={:?} active={} booted={} ready={}",
                self.irq_line_value(),
                self.irq_line_active(),
                self.is_booted(),
                ready
            );
        }
        if !ready {
            self.disable_device_irq();
            return Ok(());
        }

        self.service_pending_reads()
            .map_err(|_| InterruptError::HardwareError)?;
        Ok(())
    }

    fn interrupt_id(&self) -> Option<InterruptId> {
        self.irq_id
    }
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let cs = device
        .property("reg")
        .and_then(|p| p.as_usize())
        .map(|v| v as u8)
        .ok_or("apple-spi-hid: missing chip-select reg property")?;

    let max_freq = device
        .property("spi-max-frequency")
        .and_then(|p| p.as_usize())
        .map(|v| v as u32)
        .unwrap_or(8_000_000);

    let parent_ph = device
        .parent_phandle()
        .ok_or("apple-spi-hid: no parent phandle")?;

    let spi_bus = match DeviceManager::get_manager().get_spi_bus(parent_ph) {
        Some(bus) => bus,
        None => {
            early_println!("[apple-spi-hid] SPI bus not yet registered, deferring");
            return probe_defer();
        }
    };

    spi_bus
        .set_bus_speed(max_freq)
        .map_err(|_| "apple-spi-hid: failed to set bus speed")?;

    let spien_gpio = match device.property("spien-gpios") {
        Some(property) => {
            let data = property.value();
            if data.len() < 8 {
                (0, None)
            } else {
                let phandle = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                let pin = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                let gpio = DeviceManager::get_manager().get_gpio_controller(phandle);
                match gpio {
                    Some(controller) => (pin, Some(controller)),
                    None => {
                        early_println!(
                            "[apple-spi-hid] SPIEN GPIO controller {} not yet registered, deferring",
                            phandle
                        );
                        return probe_defer();
                    }
                }
            }
        }
        None => (0, None),
    };

    let mut irq_trigger = GpioIrqTrigger::FallingEdge;
    let irq_gpio = match device.property("interrupts-extended") {
        Some(property) => {
            let data = property.value();
            if data.len() < 12 {
                (0, None)
            } else {
                let phandle = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                let pin = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                let flags = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
                irq_trigger = irq_trigger_from_dt_flags(flags);
                let gpio = DeviceManager::get_manager().get_gpio_controller(phandle);
                match gpio {
                    Some(controller) => (pin, Some(controller)),
                    None => {
                        early_println!(
                            "[apple-spi-hid] IRQ GPIO controller {} not yet registered, deferring",
                            phandle
                        );
                        return probe_defer();
                    }
                }
            }
        }
        None => (0, None),
    };

    let keyboard_event = Arc::new(EventDevice::new("keyboard"));
    let trackpad_event = Arc::new(EventDevice::new("mouse"));

    let transport = Arc::new(AppleSpiHidTransport::new(
        spi_bus,
        cs,
        max_freq,
        keyboard_event.clone(),
        trackpad_event.clone(),
        spien_gpio,
        irq_gpio,
        irq_trigger,
    ));

    let Some(irq_gpio) = transport.irq_gpio.1.as_ref().cloned() else {
        return Err("apple-spi-hid: no IRQ GPIO controller");
    };
    let irq_pin = transport.irq_gpio.0;
    irq_gpio.set_direction_input(irq_pin);
    if !irq_gpio.request_irq(irq_pin, irq_trigger, transport.clone()) {
        early_println!(
            "[apple-spi-hid] failed to register IRQ on pin {}, deferring",
            irq_pin
        );
        return probe_defer();
    }
    irq_gpio.disable_irq(irq_pin);

    early_println!(
        "apple-spi-hid: waiting for boot packet with IRQ masked cs={} max_freq={} spien_pin={} irq_pin={} irq_trigger={:?} irq_value={} irq_active={}",
        cs,
        max_freq,
        transport.spien_gpio.0,
        irq_pin,
        irq_trigger,
        irq_gpio.get_value(irq_pin),
        transport.irq_line_active()
    );
    if let Err(err) = transport.power_on() {
        irq_gpio.free_irq(irq_pin);
        return Err(err);
    }

    early_println!(
        "apple-spi-hid: power-on done irq_value={} irq_active={}",
        irq_gpio.get_value(irq_pin),
        transport.irq_line_active()
    );

    if let Err(err) = transport.wait_for_boot_packet() {
        early_println!("apple-spi-hid: boot packet not received: {:?}", err);
        irq_gpio.free_irq(irq_pin);
        return Err("apple-spi-hid: boot packet not received");
    }

    if let Err(err) = transport.finish_initialization() {
        early_println!("apple-spi-hid: initialization failed: {:?}", err);
        irq_gpio.free_irq(irq_pin);
        return Err("apple-spi-hid: initialization failed");
    }

    DeviceManager::get_manager()
        .register_device_with_name(keyboard_event.get_name().to_string(), keyboard_event);
    DeviceManager::get_manager()
        .register_device_with_name(trackpad_event.get_name().to_string(), trackpad_event);

    {
        let mut reg = REPEAT_REGISTRY.lock();
        *reg = Some(transport.clone());
    }
    ensure_repeat_worker_started();

    transport.enable_device_irq();
    early_println!("apple-spi-hid: runtime IRQ enabled");
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_apple_spi_hid_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-spi-hid",
        probe_fn,
        remove_fn,
        alloc::vec!["apple,spi-hid-transport"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Standard);
}

scarlet::driver_initcall!(register_apple_spi_hid_driver);

static REPEAT_REGISTRY: Mutex<Option<Arc<AppleSpiHidTransport>>> = Mutex::new(None);
static REPEAT_WORKER_STARTED: AtomicBool = AtomicBool::new(false);

fn ensure_repeat_worker_started() {
    if REPEAT_WORKER_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    let task = scarlet::task::new_kernel_task("spi-hid-repeat".to_string(), 1, repeat_worker_entry);
    task.init();
    scarlet::sched::scheduler::add_task(task, scarlet::arch::get_cpu().get_cpuid());
}

fn repeat_worker_entry() {
    const REPEAT_TICKS: u64 = 1;
    loop {
        if let Some(transport) = REPEAT_REGISTRY.lock().as_ref() {
            transport.maybe_repeat_keys();
        }

        if let Some(task) = scarlet::task::mytask() {
            task.sleep(task.get_trapframe(), REPEAT_TICKS);
        } else {
            scarlet::arch::instruction::idle();
        }
    }
}

#[used]
static SCARLET_DRIVER_APPLE_SPI_HID_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
