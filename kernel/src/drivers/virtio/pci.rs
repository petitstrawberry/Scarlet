//! VirtIO PCI transport support.
//!
//! This module discovers modern VirtIO PCI capabilities and binds PCI block
//! and GPU devices to the existing VirtIO device implementations.

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::sync::Arc;
use alloc::vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::device::Device;
use crate::device::gpu::{GpuCharDevice, GpuDevice};
use crate::device::graphics::GraphicsDevice;
use crate::device::manager::{DeviceManager, DriverPriority};
use crate::device::pci::config::{self, PciConfig, capability, command, status, vendor};
use crate::device::pci::device::PciDeviceInfo;
use crate::device::pci::driver::{PciDeviceDriver, PciDeviceId};
use crate::driver_initcall;
use crate::drivers::block::virtio_blk::VirtioBlockDevice;
use crate::drivers::graphics::virtio_gpu::VirtioGpuDevice;
use crate::drivers::network::virtio_net::VirtioNetDevice;
use crate::drivers::virtio::{next_block_device_name, next_net_device_name};
use crate::drivers::virtio_snd::{VirtioSndDevice, register_audio_device};
use crate::drivers::virtio_video::VirtioVideoDevice;
use crate::interrupt::{InterruptId, InterruptManager};
use crate::vm;
use crate::{early_println, println};

const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

const VIRTIO_PCI_CAP_ID: usize = 0x00;
const VIRTIO_PCI_CAP_NEXT: usize = 0x01;
const VIRTIO_PCI_CAP_LEN: usize = 0x02;
const VIRTIO_PCI_CAP_CFG_TYPE: usize = 0x03;
const VIRTIO_PCI_CAP_BAR: usize = 0x04;
const VIRTIO_PCI_CAP_OFFSET: usize = 0x08;
const VIRTIO_PCI_CAP_LENGTH: usize = 0x0c;
const VIRTIO_PCI_NOTIFY_CAP_MULTIPLIER: usize = 0x10;

const VIRTIO_PCI_TRANSITIONAL_NET_DEVICE_ID: u16 = 0x1000;
const VIRTIO_PCI_TRANSITIONAL_BLOCK_DEVICE_ID: u16 = 0x1001;
const VIRTIO_PCI_MODERN_BLOCK_DEVICE_ID: u16 = 0x1042;
const VIRTIO_PCI_TRANSITIONAL_GPU_DEVICE_ID: u16 = 0x1010;
const VIRTIO_PCI_MODERN_GPU_DEVICE_ID: u16 = 0x1050;
const VIRTIO_PCI_MODERN_SOUND_DEVICE_ID: u16 = 0x1059;
const VIRTIO_PCI_MODERN_VIDEO_DECODER_DEVICE_ID: u16 = 0x105f;

static GPU_COUNTER: AtomicUsize = AtomicUsize::new(0);
/// Mapped register blocks for a VirtIO PCI function.
#[derive(Debug, Clone, Copy)]
pub struct VirtioPciTransport {
    /// VirtIO common configuration structure.
    pub common_cfg: usize,
    /// Virtqueue notification region.
    pub notify_cfg: usize,
    /// ISR status byte.
    pub isr_cfg: usize,
    /// Device-specific configuration region.
    pub device_cfg: usize,
    /// Queue notification offset multiplier.
    pub notify_off_multiplier: u32,
}

impl VirtioPciTransport {
    /// Calculate the notify address for a selected queue.
    ///
    /// # Arguments
    ///
    /// * `queue_idx` - Virtqueue index
    ///
    /// # Returns
    ///
    /// The mapped notification address if it can be computed.
    pub fn notify_addr(&self, queue_idx: usize) -> Option<usize> {
        let queue_notify_off = unsafe {
            crate::arch::mmio::write16(self.common_cfg + 0x16, queue_idx as u16);
            crate::arch::mmio::read16(self.common_cfg + 0x1e)
        };
        let byte_offset =
            usize::from(queue_notify_off).checked_mul(self.notify_off_multiplier as usize)?;
        self.notify_cfg.checked_add(byte_offset)
    }
}

#[derive(Debug, Clone, Copy)]
struct VirtioPciCap {
    cfg_type: u8,
    bar: u8,
    offset: u32,
    length: u32,
    notify_off_multiplier: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
struct MappedCap {
    vaddr: usize,
    notify_off_multiplier: Option<u32>,
}

fn map_cap(device: &PciDeviceInfo, cap: VirtioPciCap) -> Result<MappedCap, &'static str> {
    if cap.length == 0 {
        return Err("VirtIO PCI capability has zero length");
    }

    let bar = device
        .mmio_bar(cap.bar as usize)
        .ok_or("VirtIO PCI capability references missing or unassigned MMIO BAR")?;
    let cap_end = u64::from(cap.offset)
        .checked_add(u64::from(cap.length))
        .ok_or("VirtIO PCI capability length overflow")?;
    if cap_end > bar.size {
        return Err("VirtIO PCI capability exceeds BAR size");
    }

    let paddr = bar
        .base
        .checked_add(u64::from(cap.offset))
        .and_then(|addr| usize::try_from(addr).ok())
        .ok_or("VirtIO PCI capability address overflow")?;
    let vaddr = vm::ioremap(paddr, cap.length as usize)?;

    early_println!(
        "[virtio-pci] cap type={} bar={} base={:#x} offset={:#x} len={:#x} vaddr={:#x}",
        cap.cfg_type,
        cap.bar,
        bar.base,
        cap.offset,
        cap.length,
        vaddr
    );

    Ok(MappedCap {
        vaddr,
        notify_off_multiplier: cap.notify_off_multiplier,
    })
}

fn parse_virtio_pci_caps(
    config: &PciConfig,
    device: &PciDeviceInfo,
) -> Result<VirtioPciTransport, &'static str> {
    let addr = device.address();
    let pci_status = config.read_u16(&addr, config::offset::STATUS);
    if pci_status & status::CAPABILITIES_LIST == 0 {
        return Err("VirtIO PCI device has no capability list");
    }

    let mut common = None;
    let mut notify = None;
    let mut isr = None;
    let mut device_cfg = None;
    let mut cap_offset =
        config.read_u8(&addr, config::offset::CAPABILITIES_POINTER) as usize & !0x3;

    for _ in 0..64 {
        if cap_offset == 0 {
            break;
        }
        if !(0x40..0x100).contains(&cap_offset) {
            return Err("Invalid PCI capability pointer");
        }

        let cap_id = config.read_u8(&addr, cap_offset + VIRTIO_PCI_CAP_ID);
        let next = config.read_u8(&addr, cap_offset + VIRTIO_PCI_CAP_NEXT) as usize & !0x3;

        if cap_id == capability::VENDOR_SPECIFIC {
            let cap_len = config.read_u8(&addr, cap_offset + VIRTIO_PCI_CAP_LEN);
            if cap_len >= 16 {
                let cap = VirtioPciCap {
                    cfg_type: config.read_u8(&addr, cap_offset + VIRTIO_PCI_CAP_CFG_TYPE),
                    bar: config.read_u8(&addr, cap_offset + VIRTIO_PCI_CAP_BAR),
                    offset: config.read_u32(&addr, cap_offset + VIRTIO_PCI_CAP_OFFSET),
                    length: config.read_u32(&addr, cap_offset + VIRTIO_PCI_CAP_LENGTH),
                    notify_off_multiplier: if cap_len >= 20 {
                        Some(config.read_u32(&addr, cap_offset + VIRTIO_PCI_NOTIFY_CAP_MULTIPLIER))
                    } else {
                        None
                    },
                };

                match cap.cfg_type {
                    VIRTIO_PCI_CAP_COMMON_CFG => common = Some(map_cap(device, cap)?.vaddr),
                    VIRTIO_PCI_CAP_NOTIFY_CFG => notify = Some(map_cap(device, cap)?),
                    VIRTIO_PCI_CAP_ISR_CFG => isr = Some(map_cap(device, cap)?.vaddr),
                    VIRTIO_PCI_CAP_DEVICE_CFG => device_cfg = Some(map_cap(device, cap)?.vaddr),
                    _ => {}
                }
            }
        }

        cap_offset = next;
    }

    let notify = notify.ok_or("VirtIO PCI notify capability missing")?;

    Ok(VirtioPciTransport {
        common_cfg: common.ok_or("VirtIO PCI common capability missing")?,
        notify_cfg: notify.vaddr,
        isr_cfg: isr.ok_or("VirtIO PCI ISR capability missing")?,
        device_cfg: device_cfg.ok_or("VirtIO PCI device capability missing")?,
        notify_off_multiplier: notify.notify_off_multiplier.unwrap_or(0),
    })
}

fn enable_pci_device(config: &PciConfig, device: &PciDeviceInfo) {
    let addr = device.address();
    let command_bits = config.read_u16(&addr, config::offset::COMMAND);
    config.write_u16(
        &addr,
        config::offset::COMMAND,
        (command_bits | command::MEMORY_SPACE | command::BUS_MASTER) & !command::INTERRUPT_DISABLE,
    );
}

fn register_legacy_intx(
    device: &PciDeviceInfo,
    handler: Arc<dyn crate::device::events::InterruptCapableDevice>,
) -> Option<InterruptId> {
    let interrupt_pin = device.interrupt_pin();
    let interrupt_id = device.routed_irq().or_else(|| {
        let line = device.interrupt_line();
        (line != 0 && line != 0xff).then_some(line as InterruptId)
    })?;

    if interrupt_pin == 0 {
        return None;
    }

    let manager = InterruptManager::global();
    if let Err(e) = manager.register_interrupt_device(interrupt_id, handler) {
        early_println!(
            "[virtio-pci] Failed to register INTx IRQ {} for {:02x}:{:02x}.{}: {:?}",
            interrupt_id,
            device.address().bus,
            device.address().device,
            device.address().function,
            e
        );
        return None;
    }
    if let Err(e) =
        manager.enable_external_interrupt(interrupt_id, crate::arch::get_cpu().get_cpuid() as u32)
    {
        early_println!(
            "[virtio-pci] Failed to enable INTx IRQ {} for {:02x}:{:02x}.{}: {:?}",
            interrupt_id,
            device.address().bus,
            device.address().device,
            device.address().function,
            e
        );
        return None;
    }

    early_println!(
        "[virtio-pci] Registered INTx IRQ {} pin {} for {:02x}:{:02x}.{}",
        interrupt_id,
        interrupt_pin,
        device.address().bus,
        device.address().device,
        device.address().function
    );
    Some(interrupt_id)
}

fn probe_virtio_pci(device: &PciDeviceInfo) -> Result<(), &'static str> {
    println!(
        "[virtio-pci] Probing device {:04x}:{:04x}",
        device.vendor_id(),
        device.device_id()
    );

    let config = PciConfig::new(device.ecam_vaddr());
    enable_pci_device(&config, device);

    let transport = parse_virtio_pci_caps(&config, device)?;

    match device.device_id() {
        VIRTIO_PCI_TRANSITIONAL_NET_DEVICE_ID => {
            let name = next_net_device_name();
            let dev = Arc::new(VirtioNetDevice::new_pci(transport));
            dev.register_interface(&name);

            if let Some(interrupt_id) = register_legacy_intx(device, dev.clone()) {
                if let Err(e) = dev.enable_interrupts(interrupt_id) {
                    early_println!("[virtio-pci] Failed to enable net INTx: {}", e);
                }
            } else {
                early_println!(
                    "[virtio-pci] No usable INTx routing for net device {}",
                    name
                );
            }

            let registered: Arc<dyn Device> = dev;
            DeviceManager::get_manager().register_device_with_name(name.clone(), registered);
            println!("[virtio-pci] Registered net device {}", name);
            Ok(())
        }
        VIRTIO_PCI_TRANSITIONAL_BLOCK_DEVICE_ID | VIRTIO_PCI_MODERN_BLOCK_DEVICE_ID => {
            let name = next_block_device_name();
            let dev: Arc<dyn Device> = Arc::new(VirtioBlockDevice::new_pci(transport));
            DeviceManager::get_manager().register_device_with_name(name.clone(), dev);
            println!("[virtio-pci] Registered block device {}", name);
            Ok(())
        }
        VIRTIO_PCI_TRANSITIONAL_GPU_DEVICE_ID | VIRTIO_PCI_MODERN_GPU_DEVICE_ID => {
            let id = GPU_COUNTER.fetch_add(1, Ordering::SeqCst);
            let gpu_name = format!("gpu{}", id);
            let dev = Arc::new(VirtioGpuDevice::new_pci(transport));
            let graphics_dev: Arc<dyn Device> = dev.clone();
            DeviceManager::get_manager().register_device(graphics_dev);

            let gpu_backend: Arc<dyn GpuDevice> = dev.clone();
            let display: Arc<dyn GraphicsDevice> = dev.clone();
            let gpu_char_dev: Arc<dyn Device> = Arc::new(GpuCharDevice::new(gpu_backend, display));
            DeviceManager::get_manager().register_device_with_name(gpu_name.clone(), gpu_char_dev);
            println!("[virtio-pci] Registered GPU device {}", gpu_name);
            Ok(())
        }
        VIRTIO_PCI_MODERN_SOUND_DEVICE_ID => {
            let backend = Arc::new(VirtioSndDevice::new_pci(transport));
            let name = register_audio_device(backend);
            println!("[virtio-pci] Registered sound device {}", name);
            Ok(())
        }
        VIRTIO_PCI_MODERN_VIDEO_DECODER_DEVICE_ID => {
            let dev: Arc<dyn Device> = Arc::new(VirtioVideoDevice::new_pci(transport));
            DeviceManager::get_manager().register_device_with_name(format!("vvideo0"), dev);
            println!("[virtio-pci] Registered video decoder device vvideo0");
            Ok(())
        }
        _ => Err("Unsupported VirtIO PCI device"),
    }
}

fn remove_virtio_pci(_device: &PciDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let id_table = vec![
        PciDeviceId::new(vendor::REDHAT, VIRTIO_PCI_TRANSITIONAL_NET_DEVICE_ID),
        PciDeviceId::new(vendor::REDHAT, VIRTIO_PCI_TRANSITIONAL_BLOCK_DEVICE_ID),
        PciDeviceId::new(vendor::REDHAT, VIRTIO_PCI_MODERN_BLOCK_DEVICE_ID),
        PciDeviceId::new(vendor::REDHAT, VIRTIO_PCI_TRANSITIONAL_GPU_DEVICE_ID),
        PciDeviceId::new(vendor::REDHAT, VIRTIO_PCI_MODERN_GPU_DEVICE_ID),
        PciDeviceId::new(vendor::REDHAT, VIRTIO_PCI_MODERN_SOUND_DEVICE_ID),
        PciDeviceId::new(vendor::REDHAT, VIRTIO_PCI_MODERN_VIDEO_DECODER_DEVICE_ID),
    ];
    let driver = PciDeviceDriver::new("virtio-pci", id_table, probe_virtio_pci, remove_virtio_pci);
    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Standard);
}

driver_initcall!(register_driver);
