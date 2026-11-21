//! Worker-side checks complementary to hosted_quarter_permissions.rs.
//! Mixed/holey pages use ExecuteOnFetch: canonical data accesses must abort
//! for the kernel to mediate, not silently read a private instruction shadow.
use scarlet_hosted_runtime::{
    ram::{RamArena, host_page_size},
    run_gate::{RunGate, TaskId, WorkerId},
    worker::{Stop, Worker, WorkerThread},
    worker_context::Registers,
    worker_memory::Protection,
};
use std::path::Path;

const BASE: usize = 0x78_0000_0000;
const Q: usize = 4096;

fn run(thread: &mut WorkerThread, gate: &RunGate, regs: Registers) -> Stop {
    let permit = gate
        .issue(
            TaskId {
                id: 1,
                generation: 1,
            },
            0,
        )
        .unwrap();
    thread.issue(permit, &regs, 0x1234).unwrap();
    let stop = thread.receive_stop().unwrap();
    assert!(gate.acknowledge_stop(permit));
    assert!(!stop.guest.interrupted);
    assert_eq!(stop.guest.tls, 0x1234);
    stop
}

#[test]
fn all_quarters_reject_direct_and_indirect_entry_outside_executable_intervals() {
    let page = host_page_size().unwrap();
    assert_eq!(page, 4 * Q);
    let arena = RamArena::new(page as u64, 2 * page).unwrap();
    let data = arena.resolve(page as u64, page).unwrap();
    let caller = arena.resolve((2 * page) as u64, page).unwrap();
    for q in 0..4 {
        unsafe {
            let words = data.alias.as_ptr().add(q * Q).cast::<u32>();
            words.write(0x1000_0001); // adr x1, . (must expose guest PC)
            words.add(1).write(0xd400_00e1); // svc #7
            let target = ((q + 1) % 4) * Q;
            let delta = ((target as i64 - (q * Q + 16) as i64) / 4) as u32;
            words.add(4).write(0x1400_0000 | (delta & 0x03ff_ffff));
            words.add(Q / 4 - 1).write(0xd503_201f); // NOP at interval end
        }
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 81,
        image_generation: 1,
    });
    let mut thread = worker
        .open_thread(TaskId {
            id: 1,
            generation: 1,
        })
        .unwrap();
    for executable in 0..4 {
        let range = executable * Q..(executable + 1) * Q;
        worker
            .map(
                BASE,
                page,
                data.backing,
                data.offset,
                Protection::ExecuteOnFetch(&[range]),
            )
            .unwrap();
        for target in 0..4 {
            // Exercise native immediate B, register BR and RET, not just a
            // host-issued PC. None may bypass the guest fetch permission.
            let target_pc = BASE + target * Q;
            let caller_pc = BASE + page;
            let displacement = ((target_pc as i64 - caller_pc as i64) / 4) as u32;
            for branch in [
                0x1400_0000 | (displacement & 0x03ff_ffff),
                0x9400_0000 | (displacement & 0x03ff_ffff),
                0xd61f_0000,
                0xd63f_0000,
                0xd65f_0000,
            ] {
                unsafe { caller.alias.as_ptr().cast::<u32>().write(branch) };
                worker
                    .map(
                        caller_pc,
                        page,
                        caller.backing,
                        caller.offset,
                        Protection::ReadExecute(&[0..4]),
                    )
                    .unwrap();
                let mut regs = Registers {
                    pc: caller_pc as u64,
                    ..Default::default()
                };
                regs.x[0] = target_pc as u64;
                let stop = run(&mut thread, &gate, regs).guest;
                if target == executable {
                    assert_eq!(stop.esr, 0x5600_0007);
                    assert_eq!(stop.registers.pc, target_pc as u64 + 8);
                    assert_eq!(stop.registers.x[1], target_pc as u64);
                    if branch >> 26 == 0x25 || branch == 0xd63f_0000 {
                        assert_eq!(stop.registers.x[30], caller_pc as u64 + 4);
                    }
                } else {
                    assert!(
                        matches!(stop.esr >> 26, 0x20 | 0x21),
                        "q={target}, X={executable}, branch={branch:#x}"
                    );
                    assert_eq!(stop.registers.pc, target_pc as u64);
                    assert_eq!(stop.far, target_pc as u64);
                }
                worker.unmap(caller_pc).unwrap();
            }
        }
        // Branching out of a shadow and falling through its last guest word
        // must both recheck canonical fetch permissions, including q3 -> hole.
        for (pc, denied) in [
            (
                BASE + executable * Q + 16,
                BASE + ((executable + 1) % 4) * Q,
            ),
            (BASE + (executable + 1) * Q - 4, BASE + (executable + 1) * Q),
        ] {
            let stop = run(
                &mut thread,
                &gate,
                Registers {
                    pc: pc as u64,
                    ..Default::default()
                },
            )
            .guest;
            assert!(matches!(stop.esr >> 26, 0x20 | 0x21));
            assert_eq!(stop.registers.pc, denied as u64);
            assert_eq!(stop.far, denied as u64);
        }
        worker.unmap(BASE).unwrap();
    }
}

#[test]
fn batched_quarter_faults_preserve_canonical_pc_registers_and_original_word() {
    let page = host_page_size().unwrap();
    assert_eq!(page, 4 * Q);
    let arena = RamArena::new(page as u64, page).unwrap();
    let code = arena.resolve(page as u64, page).unwrap();
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 82,
        image_generation: 1,
    });
    let mut thread = worker
        .open_thread(TaskId {
            id: 1,
            generation: 1,
        })
        .unwrap();
    for quarter in 0..4 {
        // x18 update retires before the failing access; post-index writeback
        // must not retire. All target quarters are inaccessible canonical data.
        for access in [0xf840_8522u32, 0xf800_8522] {
            // ldr/str x2,[x9],#8
            let words = [0x9100_0652, access, 0xd400_00e1];
            unsafe {
                code.alias
                    .as_ptr()
                    .add(quarter * Q)
                    .cast::<u32>()
                    .copy_from_nonoverlapping(words.as_ptr(), words.len())
            };
            let range = quarter * Q..(quarter + 1) * Q;
            worker
                .map(
                    BASE,
                    page,
                    code.backing,
                    code.offset,
                    Protection::ExecuteOnFetch(&[range]),
                )
                .unwrap();
            for target in 0..4 {
                let entry = BASE + quarter * Q;
                let mut regs = Registers {
                    pc: entry as u64,
                    nzcv: 0xa000_0000,
                    ..Default::default()
                };
                regs.x[0] = 0xfeed;
                regs.x[2] = 0xcafe;
                regs.x[9] = (BASE + target * Q + 128) as u64;
                regs.x[18] = 41;
                let stopped = run(&mut thread, &gate, regs);
                let stop = stopped.guest;
                assert_eq!(stop.esr >> 26, 0x24);
                assert_eq!(stop.registers.pc, entry as u64 + 4);
                assert_eq!(stop.far, regs.x[9]);
                assert_eq!(stop.registers.x[9], regs.x[9]);
                assert_eq!(stop.registers.x[18], 42);
                assert_eq!(stop.registers.x[0], regs.x[0]);
                assert_eq!(stop.registers.x[2], regs.x[2]);
                assert_eq!(stop.registers.nzcv, regs.nzcv);
                assert_eq!(stopped.fault_instruction, Some(access));
            }
            worker.unmap(BASE).unwrap();
        }
    }
}

#[test]
fn every_quarter_preempts_and_resumes_with_exact_pc_and_x18() {
    verify_quarter_preemption(18);
}

#[test]
fn direct_shadow_preempts_and_resumes_at_each_quarter() {
    verify_quarter_preemption(2);
}

fn verify_quarter_preemption(accumulator: usize) {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };
    let page = host_page_size().unwrap();
    assert_eq!(page, 4 * Q);
    let arena = RamArena::new(page as u64, 2 * page).unwrap();
    let code = arena.resolve(page as u64, page).unwrap();
    let data = arena.resolve((2 * page) as u64, page).unwrap();
    // x2 stays in the direct shadow; x18 exercises the mediated fallback.
    // add Xn,Xn,#1; sub x12,x12,#1; str Xn,[x0]; cbnz x12,loop; svc #7
    let words = [
        0x9100_0400u32 | (accumulator as u32) << 5 | accumulator as u32,
        0xd100_058c,
        0xf900_0000 | accumulator as u32,
        0xb5ff_ffac,
        0xd400_00e1,
    ];
    for q in 0..4 {
        unsafe {
            code.alias
                .as_ptr()
                .add(q * Q)
                .cast::<u32>()
                .copy_from_nonoverlapping(words.as_ptr(), words.len())
        };
    }
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    worker
        .map(
            BASE + page,
            page,
            data.backing,
            data.offset,
            Protection::ReadWrite,
        )
        .unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 83,
        image_generation: 1,
    });
    let task = TaskId {
        id: 1,
        generation: 1,
    };
    let mut thread = worker.open_thread(task).unwrap();
    let progress = unsafe { AtomicU64::from_ptr(data.alias.as_ptr().cast()) };
    for q in 0..4 {
        worker
            .map(
                BASE,
                page,
                code.backing,
                code.offset,
                Protection::ExecuteOnFetch(&[q * Q..(q + 1) * Q]),
            )
            .unwrap();
        let entry = (BASE + q * Q) as u64;
        progress.store(0, Ordering::Release);
        let count = 1u64 << 48;
        let mut regs = Registers {
            pc: entry,
            nzcv: 0xa000_0000,
            ..Default::default()
        };
        regs.x[0] = (BASE + page) as u64;
        regs.x[12] = count;
        regs.x[1] = 0xfeed_cafe;
        regs.q[18] = u128::MAX;
        let permit = gate.issue(task, q).unwrap();
        thread.issue(permit, &regs, 0x1234).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while progress.load(Ordering::Acquire) < 4096 {
            assert!(Instant::now() < deadline, "quarter {q}: no progress");
            std::thread::yield_now();
        }
        worker.interrupt(permit).unwrap();
        let stopped = thread.receive_stop().unwrap().guest;
        assert!(gate.acknowledge_stop(permit));
        assert!(stopped.interrupted);
        assert_eq!(stopped.tls, 0x1234);
        let mut resumed = stopped.registers;
        let offset = resumed.pc.checked_sub(entry).expect("noncanonical PC");
        assert!(matches!(offset, 0 | 4 | 8 | 12));
        assert_eq!(
            resumed.x[accumulator] + resumed.x[12],
            count + u64::from(offset == 4)
        );
        assert_eq!(resumed.x[1], regs.x[1]);
        assert_eq!(resumed.q[18], regs.q[18]);
        assert_eq!(resumed.nzcv, regs.nzcv);
        let value = resumed.x[accumulator];
        resumed.x[12] = 2;
        let finished = run(&mut thread, &gate, resumed).guest;
        assert_eq!(finished.esr, 0x5600_0007);
        assert_eq!(finished.registers.pc, entry + 20);
        assert_eq!(
            finished.registers.x[accumulator],
            value + 2 - u64::from(offset == 4)
        );
        worker.unmap(BASE).unwrap();
    }
}
