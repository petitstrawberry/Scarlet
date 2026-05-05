use crate::abi::darwin::aarch64::DarwinAarch64Abi;
use crate::abi::darwin::error::*;
use crate::arch::Trapframe;
use crate::task::mytask;

// VM flags (from XNU mach/vm_statistics.h)
const VM_FLAGS_FIXED: i32 = 0x0;
const VM_FLAGS_ANYWHERE: i32 = 0x1;

// mach_msg options
const MACH_SEND_MSG: u32 = 0x01;
const MACH_RCV_MSG: u32 = 0x02;
const MACH_SEND_TIMEOUT: u32 = 0x10;
const MACH_RCV_TIMEOUT: u32 = 0x20;
const MACH_RCV_LARGE: u32 = 0x40;

// mach_msg return values
const MACH_MSG_SUCCESS: u32 = 0;
const MACH_SEND_INVALID_DEST: u32 = 0x10000003;
const MACH_SEND_TIMED_OUT: u32 = 0x10000004;
const MACH_SEND_INVALID_VOUCHER: u32 = 0x10000005;
const MACH_RCV_TIMED_OUT: u32 = 0x10004001;
const MACH_RCV_TOO_LARGE: u32 = 0x10004004;
const MACH_RCV_INVALID_NAME: u32 = 0x1000400c;

// Mach port right types
const MACH_PORT_RIGHT_SEND: u32 = 0;
const MACH_PORT_RIGHT_RECEIVE: u32 = 1;
const MACH_PORT_RIGHT_SEND_ONCE: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct MachMsgHeader {
    msgh_bits: u32,
    msgh_size: u32,
    msgh_remote_port: u32,
    msgh_local_port: u32,
    msgh_voucher_port: u32,
    msgh_id: u32,
}

pub fn sys_mach_task_self(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    trapframe.set_return_value(abi.mach_task_self());
    abi.mach_task_self()
}

pub fn sys_mach_msg2_trap(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let x0 = trapframe.regs.reg[0];
    let option = trapframe.regs.reg[1] as u32;
    let send_size = (trapframe.regs.reg[2] & 0xFFFFFFFF) as usize;

    let do_send = (option & MACH_SEND_MSG) != 0;
    let do_receive = (option & MACH_RCV_MSG) != 0;

    // Read the message header
    if !do_send || send_size == 0 {
        trapframe.set_return_value(MACH_RCV_TIMED_OUT as usize);
        return MACH_RCV_TIMED_OUT as usize;
    }

    let msg_bytes = read_user_bytes(task, x0, send_size.min(256));
    if msg_bytes.len() < core::mem::size_of::<MachMsgHeader>() {
        trapframe.set_return_value(MACH_SEND_INVALID_DEST as usize);
        return MACH_SEND_INVALID_DEST as usize;
    }

    let header = mach_msg_header_from_bytes(&msg_bytes);
    crate::println!(
        "[darwin] mach_msg2: id={} dest={:#x} local={:#x} size={} buf={:#x}",
        header.msgh_id, header.msgh_remote_port, header.msgh_local_port, header.msgh_size, x0
    );

    // Handle known MIG routines
    match header.msgh_id {
        200..=215 => {
            // host_info and related (subsystem mach_host, routines 0-15)
            // 200=host_info, 201=host_kernel_version, 202=_host_page_size,
            // 206=host_statistics, 207=host_request_notification, etc.
            let reply = handle_host_info(&header, &msg_bytes);
            if do_receive {
                write_user_bytes(task, x0, &reply);
            }
            trapframe.set_return_value(MACH_MSG_SUCCESS as usize);
            MACH_MSG_SUCCESS as usize
        }
        _ => {
            crate::println!(
                "[darwin] mach_msg2: unhandled MIG routine id={}",
                header.msgh_id
            );
            trapframe.set_return_value(MACH_RCV_TIMED_OUT as usize);
            MACH_RCV_TIMED_OUT as usize
        }
    }
}

/// MIG host_info (routine 200, reply 300) handler.
/// Wire format: Header(24) + NDR(8) + RetCode(4) + count(4) + data(count*4)
fn handle_host_info(request: &MachMsgHeader, msg_bytes: &[u8]) -> alloc::vec::Vec<u8> {
    let flavor = if msg_bytes.len() >= 36 {
        u32::from_le_bytes([msg_bytes[32], msg_bytes[33], msg_bytes[34], msg_bytes[35]])
    } else {
        0
    };
    let req_count = if msg_bytes.len() >= 40 {
        u32::from_le_bytes([msg_bytes[36], msg_bytes[37], msg_bytes[38], msg_bytes[39]])
    } else {
        0
    };

    crate::println!("[darwin] host_info: flavor={} count={}", flavor, req_count);

    let (ret_count, data) = match flavor {
        1 => build_host_basic_info(),
        5 => build_host_priority_info(),
        _ => {
            crate::println!("[darwin] host_info: unknown flavor {}", flavor);
            return build_mig_error_reply(request, 300, 4);
        }
    };

    let reply_size = 24 + 8 + 4 + 4 + data.len();
    let mut reply = alloc::vec![0u8; reply_size];

    write_u32_le(&mut reply, 0, 0x15);
    write_u32_le(&mut reply, 4, reply_size as u32);
    write_u32_le(&mut reply, 8, request.msgh_local_port);
    write_u32_le(&mut reply, 12, 0);
    write_u32_le(&mut reply, 16, 0);
    write_u32_le(&mut reply, 20, 300);
    reply[24] = 0; reply[25] = 0; reply[26] = 0; reply[27] = 0;
    reply[28] = 1; reply[29] = 0; reply[30] = 0; reply[31] = 0;
    write_u32_le(&mut reply, 32, 0);
    write_u32_le(&mut reply, 36, ret_count);
    reply[40..40 + data.len()].copy_from_slice(&data);

    reply
}

fn build_host_basic_info() -> (u32, alloc::vec::Vec<u8>) {
    let count = 12u32;
    let mut d = alloc::vec![0u8; count as usize * 4];
    write_u32_le(&mut d, 0, 1);
    write_u32_le(&mut d, 4, 1);
    write_u32_le(&mut d, 8, 0x80000000);
    write_u32_le(&mut d, 12, 0x0100000c);
    write_u32_le(&mut d, 16, 0);
    write_u32_le(&mut d, 20, 0);
    write_u32_le(&mut d, 24, 1);
    write_u32_le(&mut d, 28, 1);
    write_u32_le(&mut d, 32, 1);
    write_u32_le(&mut d, 36, 1);
    write_u64_le(&mut d, 40, 0x200000000);
    (count, d)
}

fn build_host_priority_info() -> (u32, alloc::vec::Vec<u8>) {
    let count = 8u32;
    let mut d = alloc::vec![0u8; count as usize * 4];
    write_u32_le(&mut d, 0, 80);
    write_u32_le(&mut d, 4, 80);
    write_u32_le(&mut d, 8, 64);
    write_u32_le(&mut d, 12, 31);
    write_u32_le(&mut d, 16, 0);
    write_u32_le(&mut d, 20, 0);
    write_u32_le(&mut d, 24, 0);
    write_u32_le(&mut d, 28, 79);
    (count, d)
}

fn build_mig_error_reply(request: &MachMsgHeader, reply_id: u32, ret_code: u32) -> alloc::vec::Vec<u8> {
    let mut reply = alloc::vec![0u8; 36];
    write_u32_le(&mut reply, 0, 0x15);
    write_u32_le(&mut reply, 4, 36);
    write_u32_le(&mut reply, 8, request.msgh_local_port);
    write_u32_le(&mut reply, 12, 0);
    write_u32_le(&mut reply, 16, 0);
    write_u32_le(&mut reply, 20, reply_id);
    reply[24] = 0; reply[25] = 0; reply[26] = 0; reply[27] = 0;
    reply[28] = 1; reply[29] = 0; reply[30] = 0; reply[31] = 0;
    write_u32_le(&mut reply, 32, ret_code);
    reply
}

fn write_u32_le(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
}

fn write_u64_le(buf: &mut [u8], offset: usize, val: u64) {
    buf[offset..offset + 8].copy_from_slice(&val.to_le_bytes());
}

pub fn sys_mach_msg_trap(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let msg_ptr = trapframe.get_arg(0);
    let option = trapframe.get_arg(1) as u32;
    let send_size = trapframe.get_arg(2) as usize;
    let rcv_size = trapframe.get_arg(3) as usize;
    let rcv_name = trapframe.get_arg(4) as u32;
    let timeout = trapframe.get_arg(5) as u32;
    let _notify = trapframe.get_arg(6);

    let do_send = (option & MACH_SEND_MSG) != 0;
    let do_receive = (option & MACH_RCV_MSG) != 0;
    let mut send_header = None;

    if do_send {
        let header_bytes = read_user_bytes(task, msg_ptr, core::mem::size_of::<MachMsgHeader>());
        if header_bytes.len() < core::mem::size_of::<MachMsgHeader>() {
            trapframe.set_return_value(MACH_SEND_INVALID_DEST as usize);
            return MACH_SEND_INVALID_DEST as usize;
        }

        let header = mach_msg_header_from_bytes(&header_bytes);
        if !is_valid_send_port(abi, header.msgh_remote_port) {
            trapframe.set_return_value(MACH_SEND_INVALID_DEST as usize);
            return MACH_SEND_INVALID_DEST as usize;
        }

        if (option & MACH_SEND_TIMEOUT) != 0 && timeout == 0 {
            trapframe.set_return_value(MACH_SEND_TIMED_OUT as usize);
            return MACH_SEND_TIMED_OUT as usize;
        }

        let message_len = header.msgh_size.min(send_size as u32) as usize;
        let message = read_user_bytes(task, msg_ptr, message_len);

        let event = crate::ipc::event::Event::direct_custom(
            task.get_id() as u32,
            alloc::string::String::from("darwin.mach"),
            header.msgh_remote_port,
            crate::ipc::event::EventPriority::Normal,
            true,
            crate::ipc::event::EventPayload::Bytes(message.clone()),
        );
        let _ = crate::ipc::event::EventManager::get_manager().send_event(event);
        abi.store_mach_message(header.msgh_remote_port, message);
        send_header = Some(header);
    }

    if do_receive {
        if !is_valid_receive_port(abi, rcv_name) {
            trapframe.set_return_value(MACH_RCV_INVALID_NAME as usize);
            return MACH_RCV_INVALID_NAME as usize;
        }

        let Some(message) = abi.take_mach_message(rcv_name) else {
            if (option & MACH_RCV_TIMEOUT) != 0 || timeout == 0 {
                trapframe.set_return_value(MACH_RCV_TIMED_OUT as usize);
                return MACH_RCV_TIMED_OUT as usize;
            }

            crate::sched::scheduler::get_scheduler().schedule(trapframe);
            trapframe.set_return_value(MACH_RCV_TIMED_OUT as usize);
            return MACH_RCV_TIMED_OUT as usize;
        };

        if message.data.len() > rcv_size {
            if (option & MACH_RCV_LARGE) != 0 {
                let mut header =
                    send_header.unwrap_or_else(|| mach_msg_header_from_message(&message.data));
                header.msgh_size = message.data.len() as u32;
                write_user_bytes(task, msg_ptr, mach_msg_header_as_bytes(&header));
            }

            trapframe.set_return_value(MACH_RCV_TOO_LARGE as usize);
            return MACH_RCV_TOO_LARGE as usize;
        }

        write_user_bytes(task, msg_ptr, &message.data);
    }

    trapframe.set_return_value(MACH_MSG_SUCCESS as usize);
    MACH_MSG_SUCCESS as usize
}

fn is_valid_send_port(_abi: &DarwinAarch64Abi, port_name: u32) -> bool {
    port_name != 0
}

fn is_valid_receive_port(_abi: &DarwinAarch64Abi, port_name: u32) -> bool {
    port_name != 0
}

fn mach_msg_header_from_message(data: &[u8]) -> MachMsgHeader {
    if data.len() >= core::mem::size_of::<MachMsgHeader>() {
        mach_msg_header_from_bytes(&data[..core::mem::size_of::<MachMsgHeader>()])
    } else {
        MachMsgHeader {
            msgh_bits: 0,
            msgh_size: data.len() as u32,
            msgh_remote_port: 0,
            msgh_local_port: 0,
            msgh_voucher_port: 0,
            msgh_id: 0,
        }
    }
}

fn mach_msg_header_from_bytes(bytes: &[u8]) -> MachMsgHeader {
    MachMsgHeader {
        msgh_bits: u32::from_ne_bytes(bytes[0..4].try_into().unwrap()),
        msgh_size: u32::from_ne_bytes(bytes[4..8].try_into().unwrap()),
        msgh_remote_port: u32::from_ne_bytes(bytes[8..12].try_into().unwrap()),
        msgh_local_port: u32::from_ne_bytes(bytes[12..16].try_into().unwrap()),
        msgh_voucher_port: u32::from_ne_bytes(bytes[16..20].try_into().unwrap()),
        msgh_id: u32::from_ne_bytes(bytes[20..24].try_into().unwrap()),
    }
}

fn mach_msg_header_as_bytes(header: &MachMsgHeader) -> &[u8] {
    unsafe {
        core::slice::from_raw_parts(
            (header as *const MachMsgHeader).cast::<u8>(),
            core::mem::size_of::<MachMsgHeader>(),
        )
    }
}

fn read_user_bytes(task: &crate::task::Task, vaddr: usize, len: usize) -> alloc::vec::Vec<u8> {
    let mut buffer = alloc::vec![0u8; len];
    let mut copied = 0;

    while copied < len {
        let current = vaddr + copied;
        let page_off = current & (crate::environment::PAGE_SIZE - 1);
        let chunk = core::cmp::min(crate::environment::PAGE_SIZE - page_off, len - copied);

        let Some(kaddr) = task.vm_manager.translate_to_kva(current) else {
            buffer.truncate(copied);
            break;
        };

        unsafe {
            core::ptr::copy_nonoverlapping(
                kaddr as *const u8,
                buffer.as_mut_ptr().add(copied),
                chunk,
            );
        }

        copied += chunk;
    }

    buffer
}

fn write_user_bytes(task: &crate::task::Task, vaddr: usize, data: &[u8]) {
    let mut written = 0;

    while written < data.len() {
        let current = vaddr + written;
        let page_off = current & (crate::environment::PAGE_SIZE - 1);
        let chunk = core::cmp::min(
            crate::environment::PAGE_SIZE - page_off,
            data.len() - written,
        );

        let Some(kaddr) = task.vm_manager.translate_to_kva(current) else {
            break;
        };

        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr().add(written), kaddr as *mut u8, chunk);
        }

        written += chunk;
    }
}

pub fn sys_mach_port_allocate(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _task_port = trapframe.get_arg(0);
    let right = trapframe.get_arg(1) as u32;
    let name_ptr = trapframe.get_arg(2);

    crate::println!("[darwin] mach_port_allocate: right={}", right);

    let port_name = match abi.allocate_mach_port(right) {
        Ok(name) => name,
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(ENOMEM);
            return usize::MAX;
        }
    };

    if name_ptr != 0 {
        if let Some(kaddr) = task.vm_manager.translate_to_kva(name_ptr) {
            unsafe {
                *(kaddr as *mut u32) = port_name;
            }
        }
    }

    trapframe.set_return_value(KERN_SUCCESS as usize);
    0
}

pub fn sys_mach_port_deallocate(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _task_port = trapframe.get_arg(0);
    let name = trapframe.get_arg(1) as u32;

    abi.deallocate_mach_port(name);

    trapframe.set_return_value(KERN_SUCCESS as usize);
    0
}

pub fn sys_vm_allocate(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _target_task = trapframe.get_arg(0);
    let addr_ptr = trapframe.get_arg(1);
    let size = trapframe.get_arg(2) as usize;
    let _flags = trapframe.get_arg(3) as i32;

    crate::println!(
        "[darwin] vm_allocate: task={:#x} addr_ptr={:#x} size={:#x} flags={:#x}",
        _target_task,
        addr_ptr,
        size,
        _flags
    );

    let aligned_size =
        (size + crate::environment::PAGE_SIZE - 1) & !(crate::environment::PAGE_SIZE - 1);

    let vaddr = match task
        .vm_manager
        .find_unmapped_area(aligned_size, crate::environment::PAGE_SIZE)
    {
        Some(addr) => addr,
        None => {
            crate::println!(
                "[darwin] vm_allocate: no space for size={:#x}",
                aligned_size
            );
            trapframe.set_return_value(KERN_NO_SPACE as usize);
            return usize::MAX;
        }
    };

    let num_pages = aligned_size / crate::environment::PAGE_SIZE;
    let pages = match crate::mem::page::ContiguousPages::new(num_pages) {
        Some(p) => p,
        None => {
            crate::println!("[darwin] vm_allocate: no pages for {} pages", num_pages);
            trapframe.set_return_value(KERN_RESOURCE_SHORTAGE as usize);
            return usize::MAX;
        }
    };

    let paddr = pages.as_paddr();
    let paddr_end = paddr + aligned_size;

    let mmap = crate::vm::vmem::VirtualMemoryMap::new(
        crate::vm::vmem::MemoryArea::new(paddr, paddr_end - 1),
        crate::vm::vmem::MemoryArea::new(vaddr, vaddr + aligned_size - 1),
        crate::vm::vmem::VirtualMemoryPermission::Read as usize
            | crate::vm::vmem::VirtualMemoryPermission::Write as usize
            | crate::vm::vmem::VirtualMemoryPermission::User as usize,
        false,
        None,
    );

    match task.vm_manager.add_memory_map(mmap) {
        Ok(()) => {
            if let Some(kva) = task.vm_manager.translate_to_kva(vaddr) {
                unsafe {
                    core::ptr::write_bytes(kva as *mut u8, 0, aligned_size);
                }
            }
            if addr_ptr != 0 {
                match task.vm_manager.translate_to_kva(addr_ptr) {
                    Some(kaddr) => {
                        unsafe {
                            *(kaddr as *mut usize) = vaddr;
                        }
                        crate::println!(
                            "[darwin] vm_allocate OK: wrote vaddr={:#x} to addr_ptr={:#x}",
                            vaddr,
                            addr_ptr
                        );
                    }
                    None => {
                        crate::println!(
                            "[darwin] vm_allocate WARN: addr_ptr={:#x} not mapped, can't write back vaddr={:#x}",
                            addr_ptr,
                            vaddr
                        );
                    }
                }
            }
            trapframe.set_return_value(KERN_SUCCESS as usize);
            core::mem::forget(pages);
            0
        }
        Err(e) => {
            crate::println!(
                "[darwin] vm_allocate FAIL: vaddr={:#x} size={:#x} paddr={:#x} err={}",
                vaddr,
                aligned_size,
                paddr,
                e
            );
            trapframe.set_return_value(KERN_FAILURE as usize);
            usize::MAX
        }
    }
}

pub fn sys_vm_map(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    static NEXT_MMAP_ADDR: spin::Mutex<usize> = spin::Mutex::new(0x300000000);
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _target_task = trapframe.get_arg(0);
    let addr_ptr = trapframe.get_arg(1);
    let size = trapframe.get_arg(2) as usize;
    let _mask = trapframe.get_arg(3);
    let flags = trapframe.get_arg(4) as i32;
    let _cur_protection = trapframe.get_arg(5);

    crate::println!(
        "[darwin] vm_map: addr_ptr={:#x} size={:#x} flags={:#x}",
        addr_ptr,
        size,
        flags,
    );

    if size == 0 {
        trapframe.set_return_value(KERN_INVALID_ARGUMENT as usize);
        return usize::MAX;
    }

    let aligned_size =
        (size + crate::environment::PAGE_SIZE - 1) & !(crate::environment::PAGE_SIZE - 1);

    let requested_addr = if addr_ptr != 0 {
        match task.vm_manager.translate_to_kva(addr_ptr) {
            Some(kaddr) => unsafe { *(kaddr as *const usize) },
            None => {
                crate::println!("[darwin] vm_map: addr_ptr {:#x} not mapped", addr_ptr);
                trapframe.set_return_value(KERN_INVALID_ARGUMENT as usize);
                return usize::MAX;
            }
        }
    } else {
        0
    };

    crate::println!(
        "[darwin] vm_map: requested_addr={:#x} flags_anywhere={}",
        requested_addr,
        flags & VM_FLAGS_ANYWHERE != 0,
    );

    let vaddr = if flags & VM_FLAGS_ANYWHERE != 0 || requested_addr == 0 {
        if requested_addr != 0 && task.vm_manager.search_memory_map(requested_addr).is_none() {
            let num_pages = aligned_size / crate::environment::PAGE_SIZE;
            if let Some(pages) = crate::mem::page::ContiguousPages::new(num_pages) {
                let paddr = pages.as_paddr();
                let mmap = crate::vm::vmem::VirtualMemoryMap::new(
                    crate::vm::vmem::MemoryArea::new(paddr, paddr + aligned_size - 1),
                    crate::vm::vmem::MemoryArea::new(
                        requested_addr,
                        requested_addr + aligned_size - 1,
                    ),
                    crate::vm::vmem::VirtualMemoryPermission::Read as usize
                        | crate::vm::vmem::VirtualMemoryPermission::Write as usize
                        | crate::vm::vmem::VirtualMemoryPermission::User as usize,
                    false,
                    None,
                );
                match task.vm_manager.add_memory_map_fixed(mmap) {
                    Ok(_vec) => {
                        core::mem::forget(pages);
                        crate::println!("[darwin] vm_map: using hint addr {:#x}", requested_addr);
                        if let Some(kva) = task.vm_manager.translate_to_kva(requested_addr) {
                            unsafe {
                                core::ptr::write_bytes(kva as *mut u8, 0, aligned_size);
                            }
                        }
                        if addr_ptr != 0 {
                            if let Some(kaddr) = task.vm_manager.translate_to_kva(addr_ptr) {
                                unsafe {
                                    *(kaddr as *mut usize) = requested_addr;
                                }
                            }
                        }
                        trapframe.set_return_value(KERN_SUCCESS as usize);
                        return 0;
                    }
                    Err(_) => {}
                }
            }
        }
        let mut next = NEXT_MMAP_ADDR.lock();
        let addr = *next;
        *next = addr + aligned_size;
        addr
    } else {
        let aligned = requested_addr & !(crate::environment::PAGE_SIZE - 1);
        crate::println!("[darwin] vm_map: FIXED at {:#x}", aligned);
        aligned
    };

    let num_pages = aligned_size / crate::environment::PAGE_SIZE;
    let pages = match crate::mem::page::ContiguousPages::new(num_pages) {
        Some(p) => p,
        None => {
            trapframe.set_return_value(KERN_RESOURCE_SHORTAGE as usize);
            return usize::MAX;
        }
    };

    let paddr = pages.as_paddr();

    let mmap = crate::vm::vmem::VirtualMemoryMap::new(
        crate::vm::vmem::MemoryArea::new(paddr, paddr + aligned_size - 1),
        crate::vm::vmem::MemoryArea::new(vaddr, vaddr + aligned_size - 1),
        crate::vm::vmem::VirtualMemoryPermission::Read as usize
            | crate::vm::vmem::VirtualMemoryPermission::Write as usize
            | crate::vm::vmem::VirtualMemoryPermission::User as usize,
        false,
        None,
    );

    let result = task.vm_manager.add_memory_map_fixed(mmap);

    match result {
        Ok(_vec) => {
            core::mem::forget(pages);
            if let Some(kva) = task.vm_manager.translate_to_kva(vaddr) {
                unsafe {
                    core::ptr::write_bytes(kva as *mut u8, 0, aligned_size);
                }
            }
            if addr_ptr != 0 {
                match task.vm_manager.translate_to_kva(addr_ptr) {
                    Some(kaddr) => {
                        unsafe {
                            *(kaddr as *mut usize) = vaddr;
                        }
                        crate::println!(
                            "[darwin] vm_map OK: wrote vaddr={:#x} to addr_ptr={:#x}",
                            vaddr,
                            addr_ptr
                        );
                    }
                    None => {
                        crate::println!(
                            "[darwin] vm_map WARN: addr_ptr={:#x} not mapped for write-back",
                            addr_ptr
                        );
                    }
                }
            }
            trapframe.set_return_value(KERN_SUCCESS as usize);
            0
        }
        Err(e) => {
            crate::println!(
                "[darwin] vm_map FAIL: vaddr={:#x} size={:#x} err={}",
                vaddr,
                aligned_size,
                e
            );
            trapframe.set_return_value(KERN_FAILURE as usize);
            usize::MAX
        }
    }
}

pub fn sys_vm_deallocate(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _target_task = trapframe.get_arg(0);
    let addr = trapframe.get_arg(1);
    let size = trapframe.get_arg(2);

    let _ = task.vm_manager.remove_memory_map_by_addr(addr);

    trapframe.set_return_value(KERN_SUCCESS as usize);
    0
}

pub fn sys_task_for_pid(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _target_task = trapframe.get_arg(0);
    let pid = trapframe.get_arg(1) as i32;
    let task_port_ptr = trapframe.get_arg(2);

    if pid as usize == task.get_id() {
        if task_port_ptr != 0 {
            if let Some(kaddr) = task.vm_manager.translate_to_kva(task_port_ptr) {
                unsafe {
                    *(kaddr as *mut u32) = abi.mach_task_self() as u32;
                }
            }
        }
        trapframe.set_return_value(KERN_SUCCESS as usize);
        0
    } else {
        trapframe.set_return_value(KERN_FAILURE as usize);
        usize::MAX
    }
}

pub fn sys_thread_create(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    crate::println!("[darwin] thread_create: stub (not implemented)");

    trapframe.set_return_value(KERN_FAILURE as usize);
    usize::MAX
}

pub fn sys_mach_timebase_info(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let info_ptr = trapframe.get_arg(0);

    // Apple Silicon: numer=1, denom=1 (nanosecond granularity)
    if info_ptr != 0 {
        if let Some(kaddr) = task.vm_manager.translate_to_kva(info_ptr) {
            unsafe {
                *(kaddr as *mut u32) = 1;
                *((kaddr as *mut u32).add(1)) = 1;
            }
        }
    }
    trapframe.set_return_value(KERN_SUCCESS as usize);
    0
}

pub fn sys_clock_get_time(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _clock_id = trapframe.get_arg(0);
    let time_ptr = trapframe.get_arg(1);

    if time_ptr != 0 {
        if let Some(kaddr) = task.vm_manager.translate_to_kva(time_ptr) {
            let now_us = crate::timer::get_time_us();
            unsafe {
                *(kaddr as *mut u64) = now_us / 1_000_000;
                *((kaddr as *mut u64).add(1)) = (now_us % 1_000_000) * 1000;
            }
        }
    }
    trapframe.set_return_value(KERN_SUCCESS as usize);
    0
}

pub fn sys_host_page_size(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _host_port = trapframe.get_arg(0);
    let size_ptr = trapframe.get_arg(1);

    if size_ptr != 0 {
        if let Some(kaddr) = task.vm_manager.translate_to_kva(size_ptr) {
            unsafe {
                *(kaddr as *mut u32) = crate::environment::PAGE_SIZE as u32;
            }
        }
    }
    trapframe.set_return_value(KERN_SUCCESS as usize);
    0
}

pub fn sys_vm_protect(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _target_task = trapframe.get_arg(0);
    let _address = trapframe.get_arg(1);
    let _size = trapframe.get_arg(2);
    let _set_maximum = trapframe.get_arg(3) as i32;
    let _new_protection = trapframe.get_arg(4) as i32;

    trapframe.set_return_value(KERN_SUCCESS as usize);
    0
}
