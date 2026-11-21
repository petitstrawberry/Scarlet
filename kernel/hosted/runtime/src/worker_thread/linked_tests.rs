use super::*;
use crate::{
    ram::{RamArena, host_page_size},
    worker_memory::Protection,
};
use std::{
    process::Command,
    sync::atomic::AtomicU64,
    time::{Duration, Instant},
};

#[test]
fn repeated_crossings_preserve_faults_interrupts_and_republication() {
    const CHILD: &str = "SCARLET_LINKED_CALL_TEST_CHILD";
    let Some(mode) = std::env::var_os(CHILD) else {
        for mode in ["fetch", "native"] {
            assert!(Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "worker_thread::linked_tests::repeated_crossings_preserve_faults_interrupts_and_republication"])
                .env(CHILD, mode).status().unwrap().success(), "{mode} text");
        }
        return;
    };

    let page = host_page_size().unwrap();
    let arena = RamArena::new(page as u64, page * 3).unwrap();
    let base = 0x78_0000_0000;
    let target = base + page;
    let data = target + page;
    let mut maps = BTreeMap::new();
    let install = |maps: &mut BTreeMap<_, _>, va: usize, words: &[u32], prot: Protection<'_>| {
        maps.remove(&va);
        let ram = arena.resolve((page + va - base) as u64, page).unwrap();
        unsafe {
            ram.alias
                .as_ptr()
                .cast::<u32>()
                .copy_from_nonoverlapping(words.as_ptr(), words.len());
            maps.insert(
                va,
                Mapping::install(va, page, ram.backing, ram.offset, prot).unwrap(),
            );
        }
    };
    let ram = arena.resolve((page * 3) as u64, page).unwrap();
    install(&mut maps, data, &[], Protection::ReadWrite);
    let interrupt = Arc::new(Interrupt::default());
    let mut thread = Thread::new(interrupt.clone()).unwrap();
    let call = 0x9400_0000 | (page as u32 / 4);
    let caller = [call, 0xb5ff_ffe3, 0xd400_00e1]; // bl target; cbnz x3,caller; svc #7
    install(&mut maps, base, &caller, Protection::ReadExecute(&[0..12]));
    let body = [
        0x9100_0652, // add x18,x18,#1
        0xf900_00b2, // str x18,[x5]
        0xd65f_03c0, // ret
    ];
    install(
        &mut maps,
        target,
        &body,
        if mode == "native" {
            Protection::ReadExecute(&[0..12])
        } else {
            Protection::ExecuteOnFetch(&[0..12])
        },
    );
    let mut regs = Registers {
        pc: base as u64,
        ..Default::default()
    };
    regs.x[3] = 1;
    regs.x[5] = data as u64;
    regs.x[18] = 0xfeed;

    let count = unsafe { &*ram.alias.as_ptr().cast::<AtomicU64>() };
    let stop = std::thread::scope(|scope| {
        scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while count.load(Ordering::Acquire) < 0xfeed + 100 && Instant::now() < deadline {
                std::thread::yield_now();
            }
            interrupt.request(1).unwrap();
        });
        thread.run(1, regs, 0, &maps).unwrap()
    });
    assert!(stop.interrupted);
    assert!(count.load(Ordering::Acquire) >= 0xfeed + 100);
    assert!(
        [base, base + 4, target, target + 4, target + 8].contains(&(stop.registers.pc as usize))
    );
    assert_eq!(stop.registers.x[30], (base + 4) as u64);
    let committed = count.load(Ordering::Acquire);
    assert_eq!(
        stop.registers.x[18],
        committed + u64::from(stop.registers.pc == (target + 4) as u64)
    );
    assert!(
        unsafe {
            (*thread.control)
                .crossings
                .entry(target as u64, (base + 4) as u64)
        }
        .is_some()
    );

    let mut retry = stop.registers;
    retry.x[3] = 0;
    assert_eq!(thread.run(2, retry, 0, &maps).unwrap().esr, 0x5600_0007);
    regs.x[5] = target as u64; // both the native and on-fetch guest mappings reject stores
    let fault = thread.run(3, regs, 0, &maps).unwrap();
    assert_eq!(fault.esr >> 26, 0x24);
    assert_eq!(fault.far, target as u64);
    assert_eq!(fault.registers.pc, (target + 4) as u64);
    assert_eq!(fault.registers.x[18], 0xfeed + 1);
    assert_eq!(fault.registers.x[30], (base + 4) as u64);

    // Repeat with a direct B. Both the new syscall and
    // the preserved (preexisting) guest link must be visible.
    let tail = [call & !(1 << 31), 0xb5ff_ffe3, 0xd400_0121];
    install(&mut maps, base, &tail, Protection::ReadExecute(&[0..12]));
    let counter = [0xd100_0463, 0xd65f_03c0]; // sub x3,x3,#1; ret
    install(
        &mut maps,
        target,
        &counter,
        Protection::ExecuteOnFetch(&[0..8]),
    );
    regs.x[3] = 100;
    regs.x[30] = (base + 4) as u64;
    let tail_stop = thread.run(4, regs, 0, &maps).unwrap();
    assert_eq!(tail_stop.esr, 0x5600_0009);
    assert_eq!(tail_stop.registers.x[3], 0);
    assert!(
        unsafe {
            (*thread.control)
                .crossings
                .entry(target as u64, (base + 4) as u64)
        }
        .is_some()
    );

    // Replace only the caller. A stale pair would keep decrementing x3.
    let replacement = [tail[0], 0xd400_0161, 0xd400_0181];
    install(
        &mut maps,
        base,
        &replacement,
        Protection::ReadExecute(&[0..12]),
    );
    let once = thread.run(5, regs, 0, &maps).unwrap();
    assert_eq!(once.esr, 0x5600_000b);
    assert_eq!(once.registers.x[3], 99);

    let tail = [tail[0], tail[1], tail[2], 0xd400_0181];
    install(&mut maps, base, &tail, Protection::ReadExecute(&[0..16]));
    assert_eq!(thread.run(6, regs, 0, &maps).unwrap().esr, 0x5600_0009);
    // Replace only the callee. Its last iteration writes a new LR after
    // entering the linked copy. RET must observe this canonical address.
    let changed_lr = [
        counter[0],
        0xb400_0043,
        0xd65f_03c0,
        0xaa09_03fe,
        0xd65f_03c0,
    ];
    install(
        &mut maps,
        target,
        &changed_lr,
        Protection::ExecuteOnFetch(&[0..20]),
    );
    regs.x[9] = (base + 12) as u64;
    let changed = thread.run(7, regs, 0, &maps).unwrap();
    assert_eq!(changed.esr, 0x5600_000c);
    assert_eq!(changed.registers.x[30], regs.x[9]);

    // An external caller needs no learned link for an unconditional thunk.
    // Jump over an inaccessible guest quarter into another admitted range.
    let mut thunk = vec![0u32; 2049];
    thunk[0] = 0x1400_0800;
    thunk[2048] = 0xd65f_03c0;
    install(
        &mut maps,
        target,
        &thunk,
        Protection::ExecuteOnFetch(&[0..4, 8192..8196]),
    );
    let mapping = &maps[&target];
    let imported = mapping.shadow_entry(target).unwrap().unwrap();
    assert_eq!(mapping.cached_shadow_entry(target), Some(imported));
    assert_eq!(
        mapping.original_shadow_pc(imported),
        Some((target + 8192) as u64)
    );
    assert!(mapping.shadow_entry(target + 4096).unwrap().is_none());
    // Retiring the body while retaining the thunk must propagate its exact
    // instruction abort on the canonical target.
    install(
        &mut maps,
        target,
        &thunk,
        Protection::ExecuteOnFetch(&[0..4]),
    );
    regs.pc = target as u64;
    let denied = thread.run(8, regs, 0, &maps).unwrap();
    assert_eq!(denied.esr >> 26, 0x20);
    assert_eq!(denied.registers.pc, (target + 8192) as u64);
    assert_eq!(denied.far, (target + 8192) as u64);
}
