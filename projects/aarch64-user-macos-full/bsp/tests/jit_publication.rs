use scarlet_hosted_runtime::{
    ram::{RamArena, host_page_size},
    run_gate::{RunGate, TaskId, WorkerId},
    worker::Worker,
    worker_context::Registers,
    worker_memory::Protection,
};
use std::path::Path;

#[test]
fn only_fetched_instructions_are_translated_and_republication_replaces_them() {
    let page = host_page_size().unwrap();
    let arena = RamArena::new(page as u64, page * 2).unwrap();
    let code = arena.resolve(page as u64, page).unwrap();
    let caller = arena.resolve((2 * page) as u64, page).unwrap();
    let base = 0x78_0000_0000;
    let offset = page / 4;
    let entry = base + offset;
    let words = [
        0x1400_0002u32, // b forward over literal
        0xd400_0021,    // svc-shaped data
        0x18ff_ffe0,    // ldr w0, literal
        0xb940_0022,    // ldr w2,[x1] from a non-executable quarter
        0xd400_00e1,    // svc #7
        0xd42a_6000,    // application brk #0x5300, same word as a translated SVC
    ];
    let source = unsafe { code.alias.as_ptr().add(offset).cast::<u32>() };
    unsafe {
        source.copy_from_nonoverlapping(words.as_ptr(), words.len());
        caller.alias.as_ptr().cast::<u32>().write(0xd61f_0000); // br x0
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    worker
        .map(
            base + page,
            page,
            caller.backing,
            caller.offset,
            Protection::ReadExecute(&[0..4]),
        )
        .unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 18,
        image_generation: 1,
    });
    let task = TaskId {
        id: 1,
        generation: 1,
    };
    let mut thread = worker.open_thread(task).unwrap();
    let mut run = |pc: usize| {
        let permit = gate.issue(task, 0).unwrap();
        let mut regs = Registers {
            pc: pc as u64,
            ..Default::default()
        };
        regs.x[0] = entry as u64;
        regs.x[1] = base as u64;
        thread.issue(permit, &regs, 0).unwrap();
        let stop = thread.receive_stop().unwrap();
        assert!(gate.acknowledge_stop(permit));
        assert!(!stop.guest.interrupted);
        stop.guest
    };
    let rx = [offset..offset + words.len() * 4];
    worker
        .map(
            base,
            page,
            code.backing,
            code.offset,
            Protection::ReadExecuteOnFetch(&rx),
        )
        .unwrap();
    unsafe {
        code.alias.as_ptr().cast::<u32>().write(123);
    }
    let first = run(base + page);
    assert_eq!(first.esr, 0x5600_0007);
    assert_eq!(first.registers.pc, entry as u64 + 20);
    assert_eq!(first.registers.x[0], u64::from(words[1]));
    assert_eq!(first.registers.x[2], 123);
    // A publisher-side alias cannot change instruction provenance in an
    // already admitted generation. The data view above remains coherent.
    unsafe {
        source.add(4).write(0xd400_0101);
    } // svc #8 in backing only
    assert_eq!(run(entry + 16).esr, 0x5600_0007);
    let brk = run(entry + 20);
    assert_eq!(brk.esr >> 26, 0x3c);
    assert_eq!(brk.esr & 0xffff, 0x5300);
    assert_eq!(brk.registers.pc, entry as u64 + 20);
    let nx = run(base);
    assert!(matches!(nx.esr >> 26, 0x20 | 0x21));
    assert_eq!(nx.registers.pc, base as u64);

    worker.unmap(base).unwrap();
    worker
        .map(base, page, code.backing, code.offset, Protection::ReadWrite)
        .unwrap();
    unsafe {
        source.add(4).write(0xd400_0121);
    } // svc #9
    worker.unmap(base).unwrap();
    worker
        .map(
            base,
            page,
            code.backing,
            code.offset,
            Protection::ReadExecuteOnFetch(&rx),
        )
        .unwrap();
    assert_eq!(run(base + page).esr, 0x5600_0009);

    worker.unmap(base).unwrap();
    worker
        .map(
            base,
            page,
            code.backing,
            code.offset,
            Protection::ExecuteOnFetch(&rx),
        )
        .unwrap();
    let denied = run(entry + 12); // ldr w2,[x1]
    assert!(matches!(denied.esr >> 26, 0x24 | 0x25));
    assert_eq!(denied.registers.pc, entry as u64 + 12);
    assert_eq!(denied.far, base as u64);
}

#[test]
fn branch_only_generated_loop_can_be_parked_and_invalidated() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    let page = host_page_size().unwrap();
    let arena = RamArena::new(page as u64, page * 2).unwrap();
    let code = arena.resolve(page as u64, page).unwrap();
    let data = arena.resolve((2 * page) as u64, page).unwrap();
    let base = 0x78_0000_0000;
    let source = code.alias.as_ptr().cast::<u32>();
    unsafe {
        source.write(0x889f_fc01); // stlr w1,[x0]
        source.add(1).write(0x1400_0000); // b .
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    let rx = [0..8];
    worker
        .map(
            base,
            page,
            code.backing,
            code.offset,
            Protection::ReadExecuteOnFetch(&rx),
        )
        .unwrap();
    worker
        .map(
            base + page,
            page,
            data.backing,
            data.offset,
            Protection::ReadWrite,
        )
        .unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 19,
        image_generation: 1,
    });
    let task = TaskId {
        id: 1,
        generation: 1,
    };
    let mut regs = Registers {
        pc: base as u64,
        ..Default::default()
    };
    regs.x[0] = (base + page) as u64;
    regs.x[1] = 1;
    let mut thread = worker.open_thread(task).unwrap();
    let permit = gate.issue(task, 0).unwrap();
    thread.issue(permit, &regs, 0).unwrap();
    let flag = unsafe { &*data.alias.as_ptr().cast::<AtomicU32>() };
    let deadline = Instant::now() + Duration::from_secs(5);
    while flag.load(Ordering::Acquire) == 0 {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let barrier = gate.close_for_mapping().unwrap();
    worker.interrupt(permit).unwrap();
    let stopped = thread.receive_stop().unwrap();
    assert!(gate.acknowledge_stop(permit));
    assert!(stopped.guest.interrupted);
    assert_eq!(stopped.guest.registers.pc, base as u64 + 4);
    let parked = barrier.wait().unwrap();
    worker.unmap(base).unwrap();
    unsafe {
        source.add(1).write(0xd400_0141);
    } // svc #10 at the suspended PC
    worker
        .map(
            base,
            page,
            code.backing,
            code.offset,
            Protection::ReadExecuteOnFetch(&rx),
        )
        .unwrap();
    parked
        .finish_after_mapping_ack(scarlet_hosted_runtime::run_gate::MappingOutcome::Committed)
        .unwrap();
    let permit = gate.issue(task, 1).unwrap();
    thread.issue(permit, &stopped.guest.registers, 0).unwrap();
    let resumed = thread.receive_stop().unwrap();
    assert!(gate.acknowledge_stop(permit));
    assert_eq!(resumed.guest.esr, 0x5600_000a);
    assert_eq!(resumed.guest.registers.pc, base as u64 + 8);
}

#[test]
fn generated_cas_preserves_the_monitor_flags_and_store_fault_pc() {
    let page = host_page_size().unwrap();
    let arena = RamArena::new(page as u64, page * 2).unwrap();
    let code = arena.resolve(page as u64, page).unwrap();
    let data = arena.resolve((2 * page) as u64, page).unwrap();
    let base = 0x78_0000_0000;
    let entry = base + page / 4;
    let words = [
        0x885f_fc40u32, // ldaxr w0,[x2] from /init's __aarch64_cas4_acq
        0x6b10_001f,    // cmp w0,w16
        0x5400_0061,    // b.ne done
        0x8811_7c41,    // stxr w17,w1,[x2]
        0x35ff_ff91,    // cbnz w17, retry
        0xd400_0181,    // done: svc #12
        0x8811_7c41,    // orphan stxr after loss of a reservation
        0xd400_01a1,    // svc #13
    ];
    let cell = data.alias.as_ptr().cast::<u32>();
    unsafe {
        code.alias
            .as_ptr()
            .add(page / 4)
            .cast::<u32>()
            .copy_from_nonoverlapping(words.as_ptr(), words.len());
        cell.write(10);
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    worker
        .map(
            base,
            page,
            code.backing,
            code.offset,
            Protection::ExecuteOnFetch(&[page / 4..page / 4 + size_of_val(&words)]),
        )
        .unwrap();
    worker
        .map(
            base + page,
            page,
            data.backing,
            data.offset,
            Protection::ReadOnly,
        )
        .unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 20,
        image_generation: 1,
    });
    let task = TaskId {
        id: 1,
        generation: 1,
    };
    let mut thread = worker.open_thread(task).unwrap();
    let mut run = |regs: &Registers| {
        let permit = gate.issue(task, 0).unwrap();
        thread.issue(permit, regs, 0).unwrap();
        let stopped = thread.receive_stop().unwrap();
        assert!(gate.acknowledge_stop(permit));
        assert!(!stopped.guest.interrupted);
        if stopped.guest.esr >> 26 == 0x24 {
            assert_eq!(stopped.fault_instruction, Some(0x8811_7c41));
            assert_eq!(
                stopped.exclusive.as_ref().map(|w| w.initial.pc),
                Some(entry as u64)
            );
        }
        stopped.guest
    };
    let mut regs = Registers {
        pc: entry as u64,
        ..Default::default()
    };
    regs.x[1] = 90;
    regs.x[2] = (base + page) as u64;
    regs.x[16] = 11;
    regs.x[17] = 123;
    let mismatch = run(&regs);
    assert_eq!(mismatch.esr, 0x5600_000c);
    assert_eq!(mismatch.registers.x[0], 10);
    assert_eq!(mismatch.registers.x[17], 123);
    assert_eq!(mismatch.registers.nzcv, 0x8000_0000);
    regs.x[16] = 10;
    let fault = run(&regs);
    assert!(matches!(fault.esr >> 26, 0x24 | 0x25));
    assert_eq!(fault.registers.pc, entry as u64 + 12);
    assert_eq!(fault.registers.nzcv, 0x6000_0000);
    assert_eq!(fault.far, regs.x[2]);
    assert_eq!(unsafe { cell.read() }, 10);
    worker.unmap(base + page).unwrap();
    worker
        .map(
            base + page,
            page,
            data.backing,
            data.offset,
            Protection::ReadWrite,
        )
        .unwrap();
    let mut lost = fault.registers;
    lost.pc = entry as u64 + 24;
    let lost = run(&lost);
    assert_eq!(lost.esr, 0x5600_000d);
    assert_ne!(lost.registers.x[17], 0);
    assert_eq!(unsafe { cell.read() }, 10);
    let retried = run(&fault.registers);
    assert_eq!(retried.esr, 0x5600_000c);
    assert_eq!(retried.registers.x[17], 0);
    assert_eq!(unsafe { cell.read() }, 90);
}

#[test]
fn concurrent_generated_exclusive_increments_are_atomic() {
    let page = host_page_size().unwrap();
    let arena = RamArena::new(page as u64, page * 2).unwrap();
    let code = arena.resolve(page as u64, page).unwrap();
    let data = arena.resolve((2 * page) as u64, page).unwrap();
    let base = 0x78_0000_0000;
    let words = [
        0x885f_feabu32, // ldaxr w11,[x21]
        0x1100_056b,    // add w11,w11,#1
        0x8809_feab,    // stlxr w9,w11,[x21]
        0x35ff_ffa9,    // cbnz w9, retry
        0x7100_0484,    // subs w4,w4,#1
        0x54ff_ff61,    // b.ne retry
        0xd400_01c1,    // svc #14
    ];
    unsafe {
        code.alias
            .as_ptr()
            .cast::<u32>()
            .copy_from_nonoverlapping(words.as_ptr(), words.len());
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    worker
        .map(
            base,
            page,
            code.backing,
            code.offset,
            Protection::ReadExecuteOnFetch(&[0..size_of_val(&words)]),
        )
        .unwrap();
    worker
        .map(
            base + page,
            page,
            data.backing,
            data.offset,
            Protection::ReadWrite,
        )
        .unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 21,
        image_generation: 1,
    });
    let mut regs = Registers {
        pc: base as u64,
        ..Default::default()
    };
    regs.x[21] = (base + page) as u64;
    regs.x[4] = 2048;
    let mut running = Vec::new();
    for cpu in 0..2 {
        let task = TaskId {
            id: cpu + 1,
            generation: 1,
        };
        let mut thread = worker.open_thread(task).unwrap();
        let permit = gate.issue(task, cpu).unwrap();
        thread.issue(permit, &regs, 0).unwrap();
        running.push((permit, thread));
    }
    // Race both native worker threads with exactly the primitive used by
    // the kernel's mediated path, through the kernel alias of their RAM.
    let cell = unsafe { core::sync::atomic::AtomicU32::from_ptr(data.alias.as_ptr().cast()) };
    let load = scarlet_hosted_runtime::exclusive::Access::decode(words[0]).unwrap();
    let store = scarlet_hosted_runtime::exclusive::Access::decode(words[2]).unwrap();
    for _ in 0..2048 {
        loop {
            let sample = u64::from(cell.load(core::sync::atomic::Ordering::Acquire));
            // SAFETY: the aligned shared word stays mapped until both workers exit.
            let status = unsafe {
                scarlet_hosted_runtime::exclusive::commit(
                    data.alias.as_ptr(),
                    load,
                    store,
                    sample,
                    sample + 1,
                )
            };
            if status == Some(0) {
                break;
            }
        }
    }
    for (permit, mut thread) in running {
        let stop = thread.receive_stop().unwrap();
        assert!(gate.acknowledge_stop(permit));
        assert_eq!(stop.guest.esr, 0x5600_000e);
    }
    assert_eq!(unsafe { data.alias.as_ptr().cast::<u32>().read() }, 6144);
}

#[test]
fn fetched_store_loop_preserves_writeback_and_the_faulting_pc() {
    let page = host_page_size().unwrap();
    let count = 52 * 1024 * 1024;
    let arena = RamArena::new(page as u64, page + count).unwrap();
    let code = arena.resolve(page as u64, page).unwrap();
    let data = arena.resolve((2 * page) as u64, count).unwrap();
    let base = 0x78_0000_0000;
    let start = base + page;
    let end = start + count;
    // Actual librustc_driver memset: str x11,[x9],#8; cmp x9,x8; b.lo loop.
    let words = [0xf800_852bu32, 0xeb08_013f, 0x54ff_ffc3, 0xd400_01e1];
    unsafe {
        code.alias
            .as_ptr()
            .cast::<u32>()
            .copy_from_nonoverlapping(words.as_ptr(), words.len());
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    worker
        .map(
            base,
            page,
            code.backing,
            code.offset,
            Protection::ExecuteOnFetch(&[0..size_of_val(&words)]),
        )
        .unwrap();
    worker
        .map(
            start,
            count,
            data.backing,
            data.offset,
            Protection::ReadWrite,
        )
        .unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 22,
        image_generation: 1,
    });
    let task = TaskId {
        id: 1,
        generation: 1,
    };
    let mut thread = worker.open_thread(task).unwrap();
    let mut run = |from: usize, to: usize, value: u64| {
        let mut regs = Registers {
            pc: base as u64,
            ..Default::default()
        };
        regs.x[8] = to as u64;
        regs.x[9] = from as u64;
        regs.x[11] = value;
        let permit = gate.issue(task, 0).unwrap();
        thread.issue(permit, &regs, 0).unwrap();
        let stop = thread.receive_stop().unwrap();
        assert!(gate.acknowledge_stop(permit));
        stop
    };
    let fill = 0xcafe_2309_7777_1234;
    let completed = run(start, end, fill);
    assert_eq!(completed.guest.esr, 0x5600_000f);
    assert_eq!(completed.guest.registers.x[9], end as u64);
    assert_eq!(completed.guest.registers.nzcv, 0x6000_0000);
    let values =
        || unsafe { std::slice::from_raw_parts(data.alias.as_ptr().cast::<u64>(), count / 8) };
    assert!(values().iter().all(|&v| v == fill));
    worker
        .map(end, page, code.backing, code.offset, Protection::ReadOnly)
        .unwrap();
    let stopped = run(end - 32, end + 8, 9);
    assert_eq!(stopped.guest.esr >> 26, 0x24);
    assert_eq!(stopped.guest.far, end as u64);
    assert_eq!(stopped.guest.registers.pc, base as u64);
    assert_eq!(stopped.guest.registers.x[9], end as u64);
    assert_eq!(stopped.fault_instruction, Some(words[0]));
    assert_eq!(&values()[count / 8 - 4..], &[9; 4]);
}

#[test]
fn preempted_fetched_loop_reports_the_matching_x18_and_pc() {
    use std::{
        sync::atomic::{AtomicU32, Ordering},
        time::{Duration, Instant},
    };
    let page = host_page_size().unwrap();
    let arena = RamArena::new(page as u64, page * 2).unwrap();
    let code = arena.resolve(page as u64, page).unwrap();
    let data = arena.resolve((2 * page) as u64, page).unwrap();
    let base = 0x78_0000_0000;
    // add x18,x18,#1; sub x12,x12,#1; str w18,[x0]; cbnz x12,loop; svc #15
    let words = [
        0x9100_0652u32,
        0xd100_058c,
        0xb900_0012,
        0xb5ff_ffac,
        0xd400_01e1,
    ];
    unsafe {
        code.alias
            .as_ptr()
            .cast::<u32>()
            .copy_from_nonoverlapping(words.as_ptr(), words.len());
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    worker
        .map(
            base,
            page,
            code.backing,
            code.offset,
            Protection::ExecuteOnFetch(&[0..size_of_val(&words)]),
        )
        .unwrap();
    worker
        .map(
            base + page,
            page,
            data.backing,
            data.offset,
            Protection::ReadWrite,
        )
        .unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 23,
        image_generation: 1,
    });
    let task = TaskId {
        id: 1,
        generation: 1,
    };
    let mut thread = worker.open_thread(task).unwrap();
    let count = 1u64 << 48;
    let mut regs = Registers {
        pc: base as u64,
        ..Default::default()
    };
    regs.x[12] = count;
    regs.x[0] = (base + page) as u64;
    let permit = gate.issue(task, 0).unwrap();
    thread.issue(permit, &regs, 0).unwrap();
    let progress = unsafe { AtomicU32::from_ptr(data.alias.as_ptr().cast()) };
    let deadline = Instant::now() + Duration::from_secs(5);
    while progress.load(Ordering::Acquire) < 4096 {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    worker.interrupt(permit).unwrap();
    let stop = thread.receive_stop().unwrap().guest;
    assert!(gate.acknowledge_stop(permit));
    assert!(stop.interrupted);
    let mut regs = stop.registers;
    let offset = regs.pc - base as u64;
    assert!(matches!(offset, 0 | 4 | 8 | 12));
    assert_eq!(regs.x[18] + regs.x[12], count + u64::from(offset == 4));
    let value = regs.x[18];
    regs.x[12] = 2;
    let permit = gate.issue(task, 0).unwrap();
    thread.issue(permit, &regs, 0).unwrap();
    let stop = thread.receive_stop().unwrap().guest;
    assert!(gate.acknowledge_stop(permit));
    assert_eq!(stop.esr, 0x5600_000f);
    assert_eq!(stop.registers.x[12], 0);
    assert_eq!(stop.registers.x[18], value + 2 - u64::from(offset == 4));
}
