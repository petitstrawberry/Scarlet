use scarlet_hosted_runtime::{
    ram::{RamArena, host_page_size},
    run_gate::{RunGate, TaskId, WorkerId},
    worker::{Worker, identity},
    worker_context::Registers,
    worker_memory::Protection,
};
use std::path::Path;

#[test]
fn broker_disconnect_exits_worker_without_aborting() {
    use std::{
        io::{Read, Write},
        os::{fd::OwnedFd, unix::net::UnixStream},
        process::{Command, Stdio},
    };

    for (last_command, expected) in [(&[][..], 0), (&[1, 0][..], 1)] {
        let (mut broker, endpoint) = UnixStream::pair().unwrap();
        let mut worker = Command::new(env!("CARGO_BIN_EXE_scarlet"))
            .arg(scarlet_hosted_runtime::worker::WORKER_ARG)
            .stdout(Stdio::from(OwnedFd::from(endpoint)))
            .spawn()
            .unwrap();
        let mut ack = [0; 8];
        broker.read_exact(&mut ack).unwrap();
        broker.write_all(last_command).unwrap();
        drop(broker);
        assert_eq!(worker.wait().unwrap().code(), Some(expected));
    }
}

#[test]
fn worker_returns_native_syscall_and_resumes_the_same_context() {
    let page = host_page_size().unwrap();
    let arena = RamArena::new(page as u64, page * 2).unwrap();
    let code = arena.resolve(page as u64, page).unwrap();
    let stack = arena.resolve((2 * page) as u64, page).unwrap();
    let instructions = [
        0x5280_0540_u32, // mov w0, #42
        0xd400_2461,     // svc #0x123
        0x1100_0400,     // add w0, w0, #1
        0xd53b_d052,     // mrs x18, tpidr_el0
        0xd51b_d040,     // msr tpidr_el0, x0
        0xd53b_d05d,     // mrs x29, tpidr_el0
        0xd51b_d052,     // msr tpidr_el0, x18
        0xd53b_d05e,     // mrs x30, tpidr_el0
        0xd51b_d05f,     // msr tpidr_el0, xzr
        0xd53b_d05f,     // mrs xzr, tpidr_el0
        0xd400_2481,     // svc #0x124
    ];
    unsafe {
        std::ptr::copy_nonoverlapping(
            instructions.as_ptr(),
            code.alias.as_ptr().cast(),
            instructions.len(),
        );
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    let entry = 0x78_0000_0000;
    worker
        .map(
            entry,
            page,
            code.backing,
            code.offset,
            Protection::ReadExecute(&[0..size_of_val(&instructions)]),
        )
        .unwrap();
    worker
        .map(
            entry + page,
            page,
            stack.backing,
            stack.offset,
            Protection::ReadWrite,
        )
        .unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 1,
        image_generation: 2,
    });
    let task = TaskId {
        id: 1,
        generation: 3,
    };
    let first = gate.issue(task, 0).unwrap();
    let registers = Registers {
        pc: entry as u64,
        sp: (entry + page * 2) as u64,
        ..Default::default()
    };
    let mut thread = worker.open_thread(task).unwrap();
    thread.issue(first, &registers, 0).unwrap();
    let stopped = thread.receive_stop().unwrap();
    assert_eq!(stopped.identity, identity(first));
    assert!(gate.acknowledge_stop(first));
    assert_eq!(stopped.guest.esr, 0x5600_0123);
    assert_eq!(stopped.guest.registers.pc, entry as u64 + 8);
    assert_eq!(stopped.guest.registers.x[0], 42);
    let mut registers = stopped.guest.registers;
    registers.x[0] = 51;
    let second = gate.issue(task, 1).unwrap();
    thread.issue(second, &registers, 0x1234).unwrap();
    let stopped = thread.receive_stop().unwrap();
    assert_eq!(stopped.identity, identity(second));
    assert!(gate.acknowledge_stop(second));
    assert_eq!(stopped.guest.esr, 0x5600_0124);
    assert_eq!(stopped.guest.registers.x[0], 52);
    assert_eq!(stopped.guest.registers.x[18], 0x1234);
    assert_eq!(stopped.guest.registers.x[29], 52);
    assert_eq!(stopped.guest.registers.x[30], 0x1234);
    assert_eq!(stopped.guest.tls, 0);
    worker.unmap(entry + page).unwrap();
}

#[test]
fn two_guest_threads_make_independent_progress_in_one_worker() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    let page = host_page_size().unwrap();
    let arena = RamArena::new(page as u64, page * 2).unwrap();
    let code = arena.resolve(page as u64, page).unwrap();
    let data = arena.resolve((2 * page) as u64, page).unwrap();
    // Each participant publishes its arrival through x0, then spins on its
    // peer's word at x1 before entering Scarlet with a normal SVC.
    let instructions = [
        0x5280_0022_u32, // mov w2, #1
        0x889f_fc02,     // stlr w2, [x0]
        0x88df_fc23,     // ldar w3, [x1]
        0x34ff_ffe3,     // cbz w3, previous instruction
        0xd400_0021,     // svc #1
    ];
    unsafe {
        std::ptr::copy_nonoverlapping(
            instructions.as_ptr(),
            code.alias.as_ptr().cast(),
            instructions.len(),
        );
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    let entry = 0x78_0000_0000;
    let shared = entry + page;
    worker
        .map(
            entry,
            page,
            code.backing,
            code.offset,
            Protection::ReadExecute(&[0..20]),
        )
        .unwrap();
    worker
        .map(
            shared,
            page,
            data.backing,
            data.offset,
            Protection::ReadWrite,
        )
        .unwrap();
    let mut first = worker
        .open_thread(TaskId {
            id: 1,
            generation: 1,
        })
        .unwrap();
    let mut second = worker
        .open_thread(TaskId {
            id: 2,
            generation: 1,
        })
        .unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 5,
        image_generation: 1,
    });
    let mut regs = Registers {
        pc: entry as u64,
        sp: (shared + page) as u64,
        ..Default::default()
    };
    regs.x[0] = shared as u64;
    regs.x[1] = (shared + 4) as u64;
    let a = gate
        .issue(
            TaskId {
                id: 1,
                generation: 1,
            },
            0,
        )
        .unwrap();
    first.issue(a, &regs, 0xaaaa).unwrap();
    let wait_flag = |index: usize| {
        let word = unsafe { &*data.alias.as_ptr().cast::<AtomicU32>().add(index) };
        let deadline = Instant::now() + Duration::from_secs(15);
        while word.load(Ordering::Acquire) == 0 {
            assert!(Instant::now() < deadline, "guest {index} never ran");
            std::thread::yield_now();
        }
    };
    wait_flag(0);
    worker.interrupt(a).unwrap();
    let parked = first.receive_stop().unwrap();
    assert!(parked.guest.interrupted);
    assert_eq!(parked.identity, identity(a));
    assert!(gate.acknowledge_stop(a));
    let cancelled = gate.issue(a.task, 1).unwrap();
    worker.interrupt(cancelled).unwrap();
    first
        .issue(cancelled, &parked.guest.registers, parked.guest.tls)
        .unwrap();
    let before_run = first.receive_stop().unwrap();
    assert!(before_run.guest.interrupted);
    assert_eq!(before_run.guest.registers.pc, parked.guest.registers.pc);
    assert!(gate.acknowledge_stop(cancelled));
    let a = gate.issue(a.task, 0).unwrap();
    // Reenter at the beginning so the first word proves renewed guest entry.
    unsafe {
        (&*data.alias.as_ptr().cast::<AtomicU32>()).store(0, Ordering::Release);
    }
    first.issue(a, &regs, 0xaaaa).unwrap();
    wait_flag(0);
    worker.interrupt(cancelled).unwrap();
    let b = gate
        .issue(
            TaskId {
                id: 2,
                generation: 1,
            },
            1,
        )
        .unwrap();
    regs.x.swap(0, 1);
    second.issue(b, &regs, 0xbbbb).unwrap();
    wait_flag(1);
    for (mut thread, permit, tls) in [(second, b, 0xbbbb), (first, a, 0xaaaa)] {
        let stop = thread.receive_stop().unwrap();
        assert!(!stop.guest.interrupted);
        assert_eq!(stop.identity, identity(permit));
        assert_eq!(stop.guest.tls, tls);
        assert_eq!(stop.guest.esr, 0x5600_0001);
        assert!(gate.acknowledge_stop(permit));
    }
}

#[test]
fn guest_platform_register_survives_resume_and_reports_original_fault_pc() {
    let page = host_page_size().unwrap();
    let arena = RamArena::new(page as u64, 2 * page).unwrap();
    let text = arena.resolve(page as u64, page).unwrap();
    let data = arena.resolve(2 * page as u64, page).unwrap();
    let words = [
        0xaa00_03f2u32, // mov x18,x0
        0xb879_7a59,    // ldr w25,[x18,x25,lsl #2] (SWS CRC32)
        0x9100_1252,    // add x18,x18,#4
        0xd400_00e1,    // svc #7
        0xb940_0241,    // ldr w1,[x18]
        0x3500_0052,    // cbnz w18,success
        0xd420_0020,    // brk #1
        0xd400_0101,    // success: svc #8
        0xaa1d_03f2,    // mov x18,fp
        0xaa12_03fe,    // mov lr,x18
        0x2a12_03f2,    // mov w18,w18
        0xaa12_03fd,    // mov fp,x18
        0xaa1f_03f2,    // mov x18,xzr
        0xaa12_03ff,    // mov xzr,x18
        0xd400_0121,    // svc #9
    ];
    unsafe {
        std::ptr::copy_nonoverlapping(words.as_ptr(), text.alias.as_ptr().cast(), words.len());
        data.alias.as_ptr().cast::<u32>().add(1).write(0x12345678);
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    let entry = 0x78_0000_0000;
    let table = entry + page;
    worker
        .map(
            entry,
            page,
            text.backing,
            text.offset,
            Protection::ReadExecute(&[0..size_of_val(&words)]),
        )
        .unwrap();
    worker
        .map(
            table,
            page,
            data.backing,
            data.offset,
            Protection::ReadWrite,
        )
        .unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 11,
        image_generation: 1,
    });
    let id = TaskId {
        id: 1,
        generation: 1,
    };
    let mut thread = worker.open_thread(id).unwrap();
    let mut regs = Registers {
        pc: entry as u64,
        ..Default::default()
    };
    regs.x[0] = table as u64;
    regs.x[25] = 1;
    let mut run = |regs: &Registers, cpu| {
        let permit = gate.issue(id, cpu).unwrap();
        thread.issue(permit, regs, 0).unwrap();
        let stop = thread.receive_stop().unwrap();
        assert!(gate.acknowledge_stop(permit));
        stop.guest
    };
    let first = run(&regs, 0);
    assert_eq!(first.esr, 0x5600_0007);
    assert_eq!(first.registers.x[25], 0x12345678);
    assert_eq!(first.registers.x[0], table as u64);
    assert_eq!(first.registers.x[18], table as u64 + 4);
    let good = run(&first.registers, 1);
    assert_eq!(good.esr, 0x5600_0008);
    assert_eq!(good.registers.x[1], 0x12345678);
    let mut moves = good.registers;
    moves.x[29] = 0xfeed_face_89ab_cdef;
    moves.nzcv = 0xb000_0000;
    let moved = run(&moves, 1);
    assert_eq!(moved.esr, 0x5600_0009);
    assert_eq!(moved.registers.x[18], 0);
    assert_eq!(moved.registers.x[29], 0x89ab_cdef);
    assert_eq!(moved.registers.x[30], moves.x[29]);
    assert_eq!(moved.registers.nzcv, moves.nzcv);
    let mut bad = first.registers;
    bad.x[18] = 0x100;
    let fault = run(&bad, 0);
    assert_eq!(fault.far, 0x100);
    assert_eq!(fault.registers.pc, bad.pc);
    assert_eq!(fault.registers.x[18], 0x100);
    assert_eq!(fault.registers.x[0], table as u64);
}

#[test]
fn preempting_native_x18_updates_resumes_at_an_exact_guest_boundary() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    let page = host_page_size().unwrap();
    let arena = RamArena::new(page as u64, 2 * page).unwrap();
    let text = arena.resolve(page as u64, page).unwrap();
    let data = arena.resolve(2 * page as u64, page).unwrap();
    let words = [
        0xc89f_fc22u32, // stlr x2,[x1]
        0x9100_0652,    // add x18,x18,#1 (native step borrows x0)
        0x9100_0442,    // add x2,x2,#1
        0x17ff_fffd,    // b back to stlr
    ];
    unsafe {
        std::ptr::copy_nonoverlapping(words.as_ptr(), text.alias.as_ptr().cast(), words.len());
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    let entry = 0x78_0000_0000;
    worker
        .map(
            entry,
            page,
            text.backing,
            text.offset,
            Protection::ReadExecute(&[0..size_of_val(&words)]),
        )
        .unwrap();
    worker
        .map(
            entry + page,
            page,
            data.backing,
            data.offset,
            Protection::ReadWrite,
        )
        .unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 12,
        image_generation: 1,
    });
    let task = TaskId {
        id: 1,
        generation: 1,
    };
    let mut thread = worker.open_thread(task).unwrap();
    let mut regs = Registers {
        pc: entry as u64,
        nzcv: 0xa000_0000,
        ..Default::default()
    };
    regs.x[0] = 0xface_cafe;
    regs.x[1] = (entry + page) as u64;
    regs.q[18] = u128::MAX;
    let published = unsafe { &*data.alias.as_ptr().cast::<AtomicU64>() };
    for epoch in 0..64 {
        let permit = gate.issue(task, epoch % 2).unwrap();
        thread.issue(permit, &regs, 0x1234).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while published.load(Ordering::Acquire) <= regs.x[2] {
            assert!(Instant::now() < deadline, "guest stopped making progress");
            std::thread::yield_now();
        }
        worker.interrupt(permit).unwrap();
        let stopped = thread.receive_stop().unwrap();
        assert!(gate.acknowledge_stop(permit));
        assert!(stopped.guest.interrupted);
        assert_eq!(stopped.guest.tls, 0x1234);
        regs = stopped.guest.registers;
        let offset = regs.pc.checked_sub(entry as u64).unwrap();
        assert!(matches!(offset, 0 | 4 | 8 | 12));
        assert_eq!(regs.x[18], regs.x[2] + u64::from(offset == 8));
        assert_eq!(regs.x[0], 0xface_cafe);
        assert_eq!(regs.nzcv, 0xa000_0000);
        assert_eq!(regs.q[18], u128::MAX);
    }
}
