use scarlet_hosted_runtime::{
    ram::{RamArena, host_page_size},
    run_gate::{RunGate, TaskId, WorkerId},
    worker::Worker,
    worker_context::Registers,
    worker_memory::Protection,
};
use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[test]
fn canonical_x18_loop_fault_and_mid_loop_resume() {
    let page = host_page_size().unwrap();
    let base = 0x78_0000_0000usize;
    let arena = RamArena::new(page as u64, page * 3).unwrap();
    let text = arena.resolve(page as u64, page).unwrap();
    let data = arena.resolve(2 * page as u64, page).unwrap();
    let tail = arena.resolve(3 * page as u64, page).unwrap();
    let words = [
        0x9100_0652u32, // add x18,x18,#1
        0xf800_8532,    // str x18,[x9],#8
        0xf100_056b,    // subs x11,x11,#1
        0x54ff_ffa1,    // b.ne loop
        0xd400_0021,    // svc #1
    ];
    unsafe {
        text.alias
            .as_ptr()
            .cast::<u32>()
            .copy_from_nonoverlapping(words.as_ptr(), words.len());
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    worker
        .map(
            base,
            page,
            text.backing,
            text.offset,
            Protection::ReadExecute(&[0..size_of_val(&words)]),
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
    worker
        .map(
            base + 2 * page,
            page,
            tail.backing,
            tail.offset,
            Protection::ReadOnly,
        )
        .unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 81,
        image_generation: 1,
    });
    let task = TaskId {
        id: 1,
        generation: 1,
    };
    let mut thread = worker.open_thread(task).unwrap();
    let mut run = |r: &Registers| {
        let permit = gate.issue(task, 0).unwrap();
        thread.issue(permit, r, 0x1234).unwrap();
        let stopped = thread.receive_stop().unwrap();
        assert!(gate.acknowledge_stop(permit));
        assert_eq!(stopped.guest.tls, 0x1234);
        stopped
    };
    let mut r = Registers {
        pc: base as u64,
        ..Default::default()
    };
    for (i, x) in r.x.iter_mut().enumerate() {
        *x = 0x19ab_0000 + i as u64;
    }
    r.x[9] = (base + 2 * page - 8) as u64;
    r.x[11] = 2;
    let initial = r;
    let fault = run(&r);
    assert_eq!(fault.fault_instruction, Some(words[1]));
    assert_eq!(fault.guest.esr >> 26, 0x24);
    assert_eq!(fault.guest.far, (base + 2 * page) as u64);
    r = fault.guest.registers;
    assert_eq!(r.pc, base as u64 + 4);
    let mut expected = initial.x;
    expected[9] += 8;
    expected[11] = 1;
    expected[18] += 2;
    assert_eq!(r.x, expected);
    assert_eq!(r.nzcv, 0x2000_0000);
    assert_eq!(
        unsafe { data.alias.as_ptr().add(page - 8).cast::<u64>().read() },
        initial.x[18] + 1
    );
    assert_eq!(unsafe { tail.alias.as_ptr().cast::<u64>().read() }, 0);
    worker.unmap(base + 2 * page).unwrap();
    worker
        .map(
            base + 2 * page,
            page,
            tail.backing,
            tail.offset,
            Protection::ReadWrite,
        )
        .unwrap();
    let end = run(&r).guest;
    assert_eq!(end.esr, 0x5600_0001);
    expected[9] += 8;
    expected[11] = 0;
    assert_eq!(end.registers.x, expected);
    assert_eq!(end.registers.nzcv, 0x6000_0000);
    assert_eq!(
        unsafe { tail.alias.as_ptr().cast::<u64>().read() },
        initial.x[18] + 2
    );
}

#[test]
fn canonical_x18_loop_preemption_preserves_every_guest_boundary() {
    let page = host_page_size().unwrap();
    let base = 0x78_0000_0000usize;
    let arena = RamArena::new(page as u64, page * 2).unwrap();
    let text = arena.resolve(page as u64, page).unwrap();
    let data = arena.resolve(2 * page as u64, page).unwrap();
    let words = [
        0x9100_0652u32,
        0x9100_0442,
        0xf900_0022,
        0xf100_056b,
        0x54ff_ff81,
        0xd400_0021,
    ];
    // ADD x18; ADD x2; STR x2,[x1]; SUBS x11; B.NE loop; SVC #1
    unsafe {
        text.alias
            .as_ptr()
            .cast::<u32>()
            .copy_from_nonoverlapping(words.as_ptr(), words.len());
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    worker
        .map(
            base,
            page,
            text.backing,
            text.offset,
            Protection::ReadExecute(&[0..size_of_val(&words)]),
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
        address_space: 82,
        image_generation: 1,
    });
    let task = TaskId {
        id: 1,
        generation: 1,
    };
    let mut thread = worker.open_thread(task).unwrap();
    let mut regs = Registers {
        pc: base as u64,
        ..Default::default()
    };
    let count = 1u64 << 55;
    regs.x[11] = count;
    regs.x[1] = (base + page) as u64;
    regs.x[0] = 0x8822;
    regs.q[18] = u128::MAX;
    let progress = unsafe { AtomicU64::from_ptr(data.alias.as_ptr().cast()) };
    for _ in 0..32 {
        let permit = gate.issue(task, 0).unwrap();
        thread.issue(permit, &regs, 0).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while progress.load(Ordering::Acquire) < regs.x[2] + 128 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        worker.interrupt(permit).unwrap();
        let stop = thread.receive_stop().unwrap().guest;
        assert!(gate.acknowledge_stop(permit));
        assert!(stop.interrupted);
        regs = stop.registers;
        let offset = regs.pc - base as u64;
        assert!(matches!(offset, 0 | 4 | 8 | 12 | 16));
        assert_eq!(regs.x[18], regs.x[2] + u64::from(offset == 4));
        assert_eq!(
            regs.x[2] + regs.x[11],
            count + u64::from(matches!(offset, 8 | 12))
        );
        assert_eq!(regs.x[0], 0x8822);
        assert_eq!(regs.q[18], u128::MAX);
    }
}
