extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use scarlet_std::hypervisor::{Vcpu, VcpuExitReason, Vm, arch::reg};
use scarlet_std::println;
use scarlet_std::sync::Mutex;

pub mod firmware;
pub mod timer;

use crate::device::IrqLine;
use crate::devices::plic::{PlicConfig, PlicDevice};
use crate::devices::uart::Ns16550a;
use crate::machine::{DtbGenerator, Machine, MachineConfig, VcpuIrqSink};
use firmware::{Firmware, FirmwareAction, sbi::SbiFirmware};
use timer::{TimerState, start_timer_thread, start_uart_thread};

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

    let mut machine = Machine::new(MachineConfig::qemu_virt());
    let guest_memory_size = machine.config().memory_size;
    let guest_phys_base = GUEST_ENTRY_POINT;
    let host_addr = allocate_guest_memory(guest_memory_size as usize);
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
        guest_phys_base, guest_memory_size
    );
    if vm
        .add_memory_region(0, guest_phys_base, guest_memory_size, host_addr as u64)
        .is_err()
    {
        println!("[ushv] Failed to add memory region");
        return 1;
    }

    println!("[ushv] Creating vCPU 0...");
    let vcpu = match vm.create_vcpu(0) {
        Ok(vcpu) => vcpu,
        Err(()) => {
            println!("[ushv] Failed to create vCPU");
            return 1;
        }
    };
    println!("[ushv] vCPU created with handle {}", vcpu.handle());

    let vcpu = Arc::new(vcpu);
    machine.set_vcpu_handle(vcpu.handle());

    let plic = PlicDevice::new(PlicConfig {
        base: 0x0C000000,
        num_sources: 128,
        num_contexts: 2,
        num_priorities: 7,
    });

    let vcpu_irq = IrqLine::new(Arc::new(VcpuIrqSink::new(vcpu.handle())));
    plic.set_irq_out(1, vcpu_irq);

    println!("[ushv] Registered PLIC at 0x0C000000");

    let uart = Ns16550a::new(0x10000000);
    uart.set_irq_out(plic.get_irq_in(10));
    println!("[ushv] Registered UART at 0x10000000");

    let uart_for_thread = uart.clone_inner();

    machine.register(plic);
    machine.register(uart);

    println!(
        "[ushv] Built machine with {} devices",
        machine.devices().len()
    );

    let machine = Arc::new(machine);

    let dtb_blob = match generate_dtb(&machine) {
        Some(blob) => blob,
        None => {
            println!("[ushv] Failed to generate DTB");
            return 1;
        }
    };
    println!("[ushv] Generated DTB: {} bytes", dtb_blob.len());

    let dtb_size_aligned = (dtb_blob.len() + 7) & !7;
    let guest_dtb_addr = GUEST_ENTRY_POINT + guest_memory_size - dtb_size_aligned as u64;
    let host_dtb_offset = (guest_dtb_addr - GUEST_ENTRY_POINT) as usize;

    unsafe {
        core::ptr::copy_nonoverlapping(
            dtb_blob.as_ptr(),
            (host_addr + host_dtb_offset) as *mut u8,
            dtb_blob.len(),
        );
    }
    println!("[ushv] DTB placed at guest address {:#x}", guest_dtb_addr);

    let timer_state = Arc::new(Mutex::new(TimerState::new()));
    timer_state.lock().set_vcpu_handle(vcpu.handle());
    start_timer_thread(Arc::clone(&timer_state));
    println!("[ushv] Timer thread started");

    start_uart_thread(uart_for_thread, Arc::clone(&vcpu));
    println!("[ushv] UART thread started");

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

    let mut firmware = SbiFirmware::new();
    firmware.set_timer_state(Arc::clone(&timer_state));

    println!("[ushv] Starting vCPU run loop...");
    run_vcpu_loop(&vcpu, &machine, &mut firmware);

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

fn run_vcpu_loop(vcpu: &Vcpu, machine: &Machine, firmware: &mut dyn Firmware) {
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
                let result = machine.handle_mmio_read(exit.mmio.address, exit.mmio.size);
                let masked = mask_mmio_value(result, exit.mmio.size);
                if exit.mmio.reg != 0 && vcpu.set_reg(exit.mmio.reg as u32, masked).is_err() {
                    println!(
                        "[ushv] Failed to write MMIO read result to reg x{}",
                        exit.mmio.reg
                    );
                    return;
                }
            }
            VcpuExitReason::MmioWrite => {
                machine.handle_mmio_write(exit.mmio.address, exit.mmio.size, exit.mmio.data);
            }
            VcpuExitReason::FirmwareCall => {
                if firmware.handle(vcpu) == FirmwareAction::Shutdown {
                    println!("[ushv] Guest requested shutdown");
                    return;
                }
            }
            VcpuExitReason::VirtualInstruction => {
                // println!("[ushv] Virtual instruction at epc={:#x}", exit.epc);
            }
            VcpuExitReason::IllegalInstruction => {
                println!("[ushv] Illegal instruction at epc={:#x}", exit.epc);
                return;
            }
            VcpuExitReason::Breakpoint => {
                println!("[ushv] Breakpoint at epc={:#x}", exit.epc);
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
                // println!("[ushv] I/O exit");
            }
            VcpuExitReason::Unknown => {
                println!("[ushv] Unknown exit reason");
            }
        }
    }
}

fn mask_mmio_value(value: u64, size: u8) -> u64 {
    match size {
        1 => value & 0xff,
        2 => value & 0xffff,
        4 => value & 0xffff_ffff,
        _ => value,
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
