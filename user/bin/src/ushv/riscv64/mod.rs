extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use scarlet_std::hypervisor::{Vcpu, VcpuExitReason, Vm, arch::reg};
use scarlet_std::println;

pub mod firmware;
pub mod machine;

use crate::device::DeviceEmulator;
use crate::devices::plic::{PlicConfig, PlicDevice};
use crate::devices::uart::Ns16550a;
use firmware::{Firmware, FirmwareAction, sbi::SbiFirmware};
use machine::{DtbGenerator, Machine, MachineConfig};

const GUEST_MEMORY_SIZE: u64 = 128 * 1024 * 1024;
const GUEST_ENTRY_POINT: u64 = 0x80000000;

pub fn run() -> i32 {
    println!("[ushv] Starting U-SHV Emulator");

    let args = parse_args();
    if args.is_empty() {
        println!("Usage: ushv <guest_image>");
        println!("  guest_image: Path to guest kernel binary");
        return 1;
    }

    let image_path = &args[0];
    println!("[ushv] Loading guest image: {}", image_path);

    let guest_image = match load_guest_image(image_path) {
        Some(img) => img,
        None => {
            println!("[ushv] Failed to load guest image");
            return 1;
        }
    };
    println!("[ushv] Image size: {} bytes", guest_image.len());

    println!("[ushv] Creating VM...");
    let vm = match Vm::create() {
        Ok(vm) => vm,
        Err(()) => {
            println!("[ushv] Failed to create VM");
            return 1;
        }
    };
    println!("[ushv] VM created with handle {}", vm.handle());

    let guest_phys_base = GUEST_ENTRY_POINT;
    let host_addr = allocate_guest_memory(GUEST_MEMORY_SIZE as usize);
    if host_addr == 0 {
        println!("[ushv] Failed to allocate guest memory");
        return 1;
    }

    unsafe {
        core::ptr::copy_nonoverlapping(
            guest_image.as_ptr(),
            host_addr as *mut u8,
            guest_image.len(),
        );
    }

    println!(
        "[ushv] Adding memory region: guest={:#x}, size={:#x}",
        guest_phys_base, GUEST_MEMORY_SIZE
    );
    if vm
        .add_memory_region(0, guest_phys_base, GUEST_MEMORY_SIZE, host_addr as u64)
        .is_err()
    {
        println!("[ushv] Failed to add memory region");
        return 1;
    }

    let mut machine = Machine::new(MachineConfig::qemu_virt());
    machine.build();
    println!(
        "[ushv] Built machine with {} devices",
        machine.devices().len()
    );

    let dtb_blob = match generate_dtb(&machine) {
        Some(blob) => blob,
        None => {
            println!("[ushv] Failed to generate DTB");
            return 1;
        }
    };
    println!("[ushv] Generated DTB: {} bytes", dtb_blob.len());

    let dtb_size_aligned = (dtb_blob.len() + 7) & !7;
    let guest_dtb_addr = GUEST_ENTRY_POINT + GUEST_MEMORY_SIZE - dtb_size_aligned as u64;
    let host_dtb_offset = (guest_dtb_addr - GUEST_ENTRY_POINT) as usize;

    unsafe {
        core::ptr::copy_nonoverlapping(
            dtb_blob.as_ptr(),
            (host_addr + host_dtb_offset) as *mut u8,
            dtb_blob.len(),
        );
    }
    println!("[ushv] DTB placed at guest address {:#x}", guest_dtb_addr);

    println!("[ushv] Creating vCPU 0...");
    let mut vcpu = match vm.create_vcpu(0) {
        Ok(vcpu) => vcpu,
        Err(()) => {
            println!("[ushv] Failed to create vCPU");
            return 1;
        }
    };
    println!("[ushv] vCPU created with handle {}", vcpu.handle());

    println!("[ushv] Setting entry point to {:#x}", GUEST_ENTRY_POINT);
    if vcpu.set_reg(reg::PC, GUEST_ENTRY_POINT).is_err() {
        println!("[ushv] Failed to set entry point");
        return 1;
    }

    let hartid: u64 = 0;
    if vcpu.set_reg(reg::A0, hartid).is_err() {
        println!("[ushv] Failed to set a0 (hartid)");
        return 1;
    }
    if vcpu.set_reg(reg::A1, guest_dtb_addr).is_err() {
        println!("[ushv] Failed to set a1 (dtb address)");
        return 1;
    }
    println!(
        "[ushv] Set boot registers: a0={}, a1={:#x}",
        hartid, guest_dtb_addr
    );

    let mut devices = DeviceEmulator::new();
    for dev_config in &machine.config().devices {
        match &dev_config.device_type {
            machine::DeviceType::Uart => {
                devices.register(Ns16550a::new(dev_config.base));
                println!("[ushv] Registered UART at {:#x}", dev_config.base);
            }
            machine::DeviceType::Plic {
                num_sources,
                num_contexts,
            } => {
                let plic = PlicDevice::new(PlicConfig {
                    base: dev_config.base,
                    num_sources: *num_sources,
                    num_contexts: *num_contexts,
                    num_priorities: 7,
                });
                println!("[ushv] Registered PLIC at {:#x}", dev_config.base);
                devices.register(plic);
            }
        }
    }

    let mut firmware = SbiFirmware::new();

    println!("[ushv] Starting vCPU run loop...");
    run_vcpu_loop(&mut vcpu, &mut devices, &mut firmware);

    println!("[ushv] VM terminated");
    0
}

fn generate_dtb(machine: &Machine) -> Option<Vec<u8>> {
    let generator = DtbGenerator::new(machine.config(), machine.devices());
    match generator.generate() {
        Ok(blob) => Some(blob),
        Err(_) => {
            println!("[ushv] DTB generation failed");
            None
        }
    }
}

fn run_vcpu_loop(vcpu: &mut Vcpu, devices: &mut DeviceEmulator, firmware: &mut dyn Firmware) {
    loop {
        let exit = match vcpu.run() {
            Ok(exit) => exit,
            Err(()) => {
                println!("[ushv] vCPU run failed");
                return;
            }
        };

        match exit.reason {
            VcpuExitReason::MmioRead => {
                let _result = devices.handle_mmio_read(exit.mmio.address, exit.mmio.size);
            }
            VcpuExitReason::MmioWrite => {
                devices.handle_mmio_write(exit.mmio.address, exit.mmio.size, exit.mmio.data);
            }
            VcpuExitReason::FirmwareCall => {
                if firmware.handle(vcpu) == FirmwareAction::Shutdown {
                    println!("[ushv] Guest requested shutdown");
                    return;
                }
            }
            VcpuExitReason::Hlt => {
                println!("[ushv] Guest halted");
            }
            VcpuExitReason::Shutdown => {
                println!("[ushv] Guest shutdown");
                return;
            }
            VcpuExitReason::FailEntry => {
                println!("[ushv] Guest failed entry: code={}", exit.fail_code);
                return;
            }
            VcpuExitReason::InternalError => {
                println!("[ushv] Internal error");
                return;
            }
            VcpuExitReason::Io => {
                println!("[ushv] I/O exit");
            }
            VcpuExitReason::Unknown => {
                println!("[ushv] Unknown exit reason");
            }
        }
    }
}

fn parse_args() -> Vec<String> {
    let all_args = scarlet_std::env::args_vec();
    if all_args.len() > 1 {
        all_args[1..].to_vec()
    } else {
        Vec::new()
    }
}

fn load_guest_image(path: &str) -> Option<Vec<u8>> {
    use scarlet_std::fs::File;
    use scarlet_std::io::SeekFrom;

    let mut file = File::open(path).ok()?;
    let size = file.seek(SeekFrom::End(0)).ok()? as usize;
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut buffer = vec![0u8; size];
    let stream = file.as_handle().as_stream().ok()?;
    stream.read_exact(&mut buffer).ok()?;
    Some(buffer)
}

fn allocate_guest_memory(size: usize) -> usize {
    use scarlet_std::syscall::{Syscall, syscall6};

    const PROT_READ: usize = 0x1;
    const PROT_WRITE: usize = 0x2;
    const MAP_ANONYMOUS: usize = 0x20;
    const MAP_PRIVATE: usize = 0x2;

    let addr = syscall6(
        Syscall::MemoryMap,
        0,
        0,
        size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        0,
    );
    if addr == usize::MAX { 0 } else { addr }
}
