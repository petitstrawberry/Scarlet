use scarlet_hosted_runtime::{
    ram::{RamArena, host_page_size},
    run_gate::{RunGate, TaskId, WorkerId},
    worker::{Worker, identity},
    worker_context::Registers,
    worker_memory::Protection,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

fn worker_thread_count(pid: i32) -> i32 {
    let mut task: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = size_of_val(&task) as i32;
    let bytes =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTASKINFO, 0, (&raw mut task).cast(), size) };
    assert_eq!(bytes, size);
    task.pti_threadnum
}

fn wait_for_count(pid: i32, expected: i32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let count = worker_thread_count(pid);
        if count == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected} worker threads, found {count}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn execution_threads_follow_concurrency_and_retire_with_task_handles() {
    let mut worker = Worker::spawn(Path::new(env!("CARGO_BIN_EXE_scarlet"))).unwrap();
    // This test executable spawns only this child.
    let mut children = [0i32; 2];
    assert!(
        unsafe {
            libc::proc_listchildpids(
                libc::getpid(),
                children.as_mut_ptr().cast(),
                size_of_val(&children) as i32,
            )
        } > 0
    );
    let [pid, 0] = children else {
        panic!("expected one worker process")
    };
    let baseline = worker_thread_count(pid);
    let page = host_page_size().unwrap();
    let arena = RamArena::new(page as u64, page).unwrap();
    let text = arena.resolve(page as u64, page).unwrap();
    let program = [0xd400_00e1u32, 0x1400_0000]; // svc #7, b .
    unsafe {
        std::ptr::copy_nonoverlapping(program.as_ptr(), text.alias.as_ptr().cast(), program.len())
    };
    let base = 0x78_0000_0000;
    worker
        .map(
            base,
            page,
            text.backing,
            text.offset,
            Protection::ReadExecute(&[0..8]),
        )
        .unwrap();
    let gate = RunGate::new(WorkerId {
        address_space: 1,
        image_generation: 1,
    });
    let mut tasks: Vec<_> = (0..96)
        .map(|id| {
            let task = TaskId { id, generation: 1 };
            (task, worker.open_thread(task).unwrap())
        })
        .collect();
    assert!(worker.open_thread(tasks[0].0).is_err());
    assert_eq!(worker_thread_count(pid), baseline);
    let regs = Registers {
        pc: base as u64,
        ..Default::default()
    };
    for (task, thread) in &mut tasks {
        // Rotating vCPUs should reuse the one available executor.
        let permit = gate.issue(*task, task.id % 4).unwrap();
        let mut registers = regs;
        registers.x[18] = task.id as u64 + 100;
        thread.issue(permit, &registers, task.id as u64).unwrap();
        let stop = thread.receive_stop().unwrap();
        assert_eq!(stop.identity, identity(permit));
        assert_eq!(stop.guest.esr, 0x5600_0007);
        assert_eq!(stop.guest.tls, task.id as u64);
        assert_eq!(stop.guest.registers.x[18], registers.x[18]);
        assert!(gate.acknowledge_stop(permit));
    }
    assert_eq!(worker_thread_count(pid), baseline + 1);

    let (one, mut a) = tasks.pop().unwrap();
    let (two, mut b) = tasks.pop().unwrap();
    drop(tasks);
    // Deliver a newer permit first and interrupt the older one before RUN.
    // Its interrupt must survive even though this executor served the newer epoch.
    let first = gate.issue(one, 0).unwrap();
    let second = gate.issue(two, 1).unwrap();
    worker.interrupt(first).unwrap();
    b.issue(second, &regs, 0).unwrap();
    assert_eq!(b.receive_stop().unwrap().guest.esr, 0x5600_0007);
    assert!(gate.acknowledge_stop(second));
    a.issue(first, &regs, 0).unwrap();
    let cancelled = a.receive_stop().unwrap().guest;
    assert!(cancelled.interrupted);
    assert_eq!(cancelled.registers.pc, regs.pc);
    assert!(gate.acknowledge_stop(first));

    let spin = Registers {
        pc: regs.pc + 4,
        ..regs
    };
    let first = gate.issue(one, 0).unwrap();
    let second = gate.issue(two, 1).unwrap();
    a.issue(first, &spin, 0).unwrap();
    b.issue(second, &spin, 0).unwrap();
    assert_eq!(worker_thread_count(pid), baseline + 2);
    drop(a); // Interrupt and drain an abandoned run before freeing its stack.
    assert!(gate.acknowledge_stop(first));
    wait_for_count(pid, baseline + 1);
    worker.interrupt(second).unwrap();
    assert!(b.receive_stop().unwrap().guest.interrupted);
    assert!(gate.acknowledge_stop(second));

    let new_task = TaskId {
        generation: one.generation + 1,
        ..one
    };
    let mut replacement = worker.open_thread(new_task).unwrap();
    let new_run = gate.issue(new_task, 0).unwrap();
    worker.interrupt(first).unwrap();
    replacement.issue(new_run, &regs, 0).unwrap();
    assert_eq!(replacement.receive_stop().unwrap().guest.esr, 0x5600_0007);
    assert!(gate.acknowledge_stop(new_run));
    drop((replacement, b));
    wait_for_count(pid, baseline);
}
