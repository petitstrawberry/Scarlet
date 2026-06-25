#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::device::events::InterruptCapableDevice;
use scarlet::device::gpio::GpioIrqTrigger;
use scarlet::device::input::event_device::EventDevice;
use scarlet::device::input::event_types::{EV_KEY, EV_REL, EV_SYN};
use scarlet::device::input::key_codes::{BTN_LEFT, BTN_RIGHT};
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

const DEVICE_MGMT: u8 = 0x00;
const DEVICE_KEYBOARD: u8 = 0x01;
const DEVICE_TRACKPAD: u8 = 0x02;
const DEVICE_INFO: u8 = 0xd0;

const FLAG_WRITE: u8 = 0x40;
const FLAG_READ: u8 = 0x20;

const BOOT_CMD: [u8; 4] = [0xa0, 0x80, 0x00, 0x00];
const STATUS_OK: u32 = 0xd56827ac;
const SPI_DELAY_US: u64 = 200;

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

pub struct AppleSpiHidTransport {
    spi_bus: Arc<dyn SpiBus>,
    cs: u8,
    device_id: u8,
    event_device: Arc<EventDevice>,
    spien_gpio: (u32, Option<Arc<dyn scarlet::device::gpio::GpioController>>),
    irq_gpio: (u32, Option<Arc<dyn scarlet::device::gpio::GpioController>>),
    irq_id: Option<InterruptId>,
    last_modifiers: Mutex<u8>,
    last_keys: Mutex<[u8; 6]>,
    last_buttons: Mutex<u8>,
}

impl AppleSpiHidTransport {
    fn new(
        spi_bus: Arc<dyn SpiBus>,
        cs: u8,
        device_id: u8,
        event_device: Arc<EventDevice>,
        spien_gpio: (u32, Option<Arc<dyn scarlet::device::gpio::GpioController>>),
        irq_gpio: (u32, Option<Arc<dyn scarlet::device::gpio::GpioController>>),
    ) -> Self {
        Self {
            spi_bus,
            cs,
            device_id,
            event_device,
            spien_gpio,
            irq_gpio,
            irq_id: None,
            last_modifiers: Mutex::new(0),
            last_keys: Mutex::new([0; 6]),
            last_buttons: Mutex::new(0),
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
        let crc = Self::crc16_ccitt(&packet[..254]);
        packet[254..256].copy_from_slice(&crc.to_le_bytes());
        packet
    }

    fn crc16_ccitt(data: &[u8]) -> u16 {
        let mut crc = 0xffffu16;
        for byte in data {
            crc ^= (*byte as u16) << 8;
            for _ in 0..8 {
                if (crc & 0x8000) != 0 {
                    crc = (crc << 1) ^ 0x1021;
                } else {
                    crc <<= 1;
                }
            }
        }
        crc
    }

    fn send_packet(&self, packet: &mut [u8; PACKET_SIZE]) -> Result<(), SpiError> {
        let crc = Self::crc16_ccitt(&packet[..254]);
        packet[254..256].copy_from_slice(&crc.to_le_bytes());
        let mut seg = SpiTransfer::write(self.cs, packet);
        self.spi_bus.transfer(core::slice::from_mut(&mut seg))?;
        udelay(SPI_DELAY_US);
        Ok(())
    }

    fn recv_packet(&self) -> Result<[u8; PACKET_SIZE], SpiError> {
        let mut seg = SpiTransfer::read(self.cs, PACKET_SIZE);
        self.spi_bus.transfer(core::slice::from_mut(&mut seg))?;
        udelay(SPI_DELAY_US);
        let mut packet = [0u8; PACKET_SIZE];
        packet.copy_from_slice(&seg.data);
        Ok(packet)
    }

    fn send_message(&self, device_id: u8, data: &[u8]) -> Result<(), SpiError> {
        if data.is_empty() {
            let mut packet = Self::build_packet(FLAG_WRITE, device_id, 0, 0, 0, &[]);
            return self.send_packet(&mut packet);
        }

        let mut offset = 0usize;
        while offset < data.len() {
            let chunk_len = core::cmp::min(MAX_PAYLOAD, data.len() - offset);
            let remain = data.len() - offset - chunk_len;
            let mut packet = Self::build_packet(
                FLAG_WRITE,
                device_id,
                offset as u16,
                remain as u16,
                chunk_len as u16,
                &data[offset..offset + chunk_len],
            );
            self.send_packet(&mut packet)?;
            offset += chunk_len;
        }

        Ok(())
    }

    fn recv_message(&self) -> Result<Vec<u8>, SpiError> {
        let mut message = Vec::new();
        loop {
            let packet = self.recv_packet()?;
            let expected_crc = Self::crc16_ccitt(&packet[..254]);
            let packet_crc = u16::from_le_bytes([packet[254], packet[255]]);
            if expected_crc != packet_crc {
                return Err(SpiError::BusError);
            }

            let length = u16::from_le_bytes([packet[6], packet[7]]) as usize;
            let remain = u16::from_le_bytes([packet[4], packet[5]]) as usize;
            if length > MAX_PAYLOAD {
                return Err(SpiError::InvalidArg);
            }

            message.extend_from_slice(&packet[8..8 + length]);
            if remain == 0 {
                break;
            }
        }
        Ok(message)
    }

    fn request_read(&self, device_id: u8) -> Result<Vec<u8>, SpiError> {
        let mut packet = Self::build_packet(FLAG_READ, device_id, 0, 0, 0, &[]);
        self.send_packet(&mut packet)?;
        self.recv_message()
    }

    fn initialize(&self) -> Result<(), &'static str> {
        if let Some(ref gpio) = self.spien_gpio.1 {
            let pin = self.spien_gpio.0;
            gpio.set_direction_output(pin, true);
            gpio.set_value(pin, true);
        } else {
            early_println!("apple-spi-hid: no GPIO controller for SPIEN, skipping power-on");
        }
        udelay(100_000);

        if let Some(ref gpio) = self.irq_gpio.1 {
            gpio.set_direction_input(self.irq_gpio.0);
        }

        self.send_message(DEVICE_MGMT, &BOOT_CMD)
            .map_err(|_| "apple-spi-hid: boot command send failed")?;
        let _boot_resp = self
            .recv_message()
            .map_err(|_| "apple-spi-hid: boot response read failed")?;

        let info = self
            .request_read(DEVICE_INFO)
            .map_err(|_| "apple-spi-hid: device info read failed")?;
        if info.len() >= 4 {
            let status = u32::from_le_bytes([info[0], info[1], info[2], info[3]]);
            if status != STATUS_OK {
                return Err("apple-spi-hid: device status not ready");
            }
        }

        let _ = self.request_read(DEVICE_KEYBOARD);
        let _ = self.request_read(DEVICE_TRACKPAD);
        Ok(())
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
                    self.event_device.push_event(
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
                    self.event_device.push_event(EV_KEY, code, KEY_RELEASE);
                }
            }
        }

        for usage in new_keys {
            if usage != 0 && !old_keys.contains(&usage) {
                if let Some(code) = hid_usage_to_key(usage) {
                    self.event_device.push_event(EV_KEY, code, KEY_PRESS);
                }
            }
        }

        *old_modifiers = new_modifiers;
        *old_keys = new_keys;
    }

    fn handle_keyboard_report(&self, report: &[u8]) {
        if report.len() < 8 {
            return;
        }

        let modifiers = report[0];
        let mut keys = [0u8; 6];
        keys.copy_from_slice(&report[2..8]);
        self.emit_key_changes(modifiers, keys);
        self.event_device.push_event(EV_SYN, SYN_REPORT, 0);
    }

    fn handle_trackpad_report(&self, report: &[u8]) {
        if report.len() < 3 {
            return;
        }

        let buttons = report[0];
        let dx = report[1] as i8;
        let dy = report[2] as i8;

        let mut old_buttons = self.last_buttons.lock();
        for (mask, code) in [(0x01, BTN_LEFT), (0x02, BTN_RIGHT)] {
            let was_pressed = (*old_buttons & mask) != 0;
            let is_pressed = (buttons & mask) != 0;
            if was_pressed != is_pressed {
                self.event_device.push_event(
                    EV_KEY,
                    code,
                    if is_pressed { KEY_PRESS } else { KEY_RELEASE },
                );
            }
        }

        if dx != 0 {
            self.event_device.push_event(EV_REL, REL_X, dx as i32);
        }
        if dy != 0 {
            self.event_device.push_event(EV_REL, REL_Y, dy as i32);
        }

        *old_buttons = buttons;
        self.event_device.push_event(EV_SYN, SYN_REPORT, 0);
    }

    fn handle_input_report(&self, report: &[u8]) {
        match self.device_id {
            DEVICE_KEYBOARD => self.handle_keyboard_report(report),
            DEVICE_TRACKPAD => self.handle_trackpad_report(report),
            _ => {}
        }
    }
}

impl InterruptCapableDevice for AppleSpiHidTransport {
    fn handle_interrupt(&self) -> InterruptResult<()> {
        let report = self
            .recv_message()
            .map_err(|_| InterruptError::HardwareError)?;
        self.handle_input_report(&report);
        Ok(())
    }

    fn interrupt_id(&self) -> Option<InterruptId> {
        self.irq_id
    }
}

static HID_REGISTRY: Mutex<Vec<Arc<AppleSpiHidTransport>>> = Mutex::new(Vec::new());

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

    let irq_gpio = match device.property("interrupts-extended") {
        Some(property) => {
            let data = property.value();
            if data.len() < 12 {
                (0, None)
            } else {
                let phandle = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                let pin = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
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

    let keyboard = Arc::new(AppleSpiHidTransport::new(
        spi_bus.clone(),
        cs,
        DEVICE_KEYBOARD,
        keyboard_event.clone(),
        spien_gpio.clone(),
        irq_gpio.clone(),
    ));
    let trackpad = Arc::new(AppleSpiHidTransport::new(
        spi_bus,
        cs,
        DEVICE_TRACKPAD,
        trackpad_event.clone(),
        spien_gpio,
        irq_gpio,
    ));

    keyboard.initialize()?;

    if let Some(ref gpio) = keyboard.irq_gpio.1 {
        let pin = keyboard.irq_gpio.0;
        if !gpio.request_irq(pin, GpioIrqTrigger::FallingEdge, keyboard.clone()) {
            early_println!(
                "[apple-spi-hid] failed to register keyboard IRQ on pin {}, deferring",
                pin
            );
            return probe_defer();
        }
        if !gpio.request_irq(pin, GpioIrqTrigger::FallingEdge, trackpad.clone()) {
            early_println!(
                "[apple-spi-hid] failed to register trackpad IRQ on pin {}, deferring",
                pin
            );
            return probe_defer();
        }
    }

    DeviceManager::get_manager()
        .register_device_with_name(keyboard_event.get_name().to_string(), keyboard_event);
    DeviceManager::get_manager()
        .register_device_with_name(trackpad_event.get_name().to_string(), trackpad_event);

    let mut registry = HID_REGISTRY.lock();
    registry.push(keyboard);
    registry.push(trackpad);
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

#[used]
static SCARLET_DRIVER_APPLE_SPI_HID_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
