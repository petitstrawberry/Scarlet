use super::*;
use crate::{
    ram::{RamArena, host_page_size},
    worker_context::Registers,
    worker_memory::{Mapping, Protection},
    worker_thread::{Interrupt, Thread},
};
use std::{
    collections::BTreeMap,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

#[test]
fn interior_entries_keep_stride_and_exits_have_guest_pcs() {
    let code = [
        0xb400_0042, // cbz x2,+8
        0x1400_0005, // b beyond the interval
        0x3700_0043, // tbnz w3,#0,+8
        0xb400_0032, // cbz x18,+4
        0x17ff_fffc, // b -16
        0x9400_0001, // bl beyond the interval
    ];
    let shadows = Shadows::new(code.len());
    assert_eq!(shadows.cached_entry(0), None);
    let start = shadows.entry(&code, 0, 0).unwrap().unwrap();
    for i in 0..=code.len() {
        let pc = start + i as u64 * 4;
        assert_eq!(shadows.original_offset(pc), Some(i * 4));
        if i < code.len() {
            assert_eq!(
                shadows.cached_entry(i),
                matches!(i, 0 | 2 | 4).then_some(pc)
            );
        }
    }
    assert_eq!(shadows.original_offset(start + 1), None);
    assert_eq!(
        shadows.original_offset(start + (code.len() + 1) as u64 * 4),
        None
    );
    #[cfg(target_os = "macos")]
    assert_eq!(
        unsafe {
            libc::mprotect(
                start as *mut _,
                host_page_size().unwrap(),
                libc::PROT_READ | libc::PROT_WRITE,
            )
        },
        -1
    );

    // A fallthrough at the final slot is attributed to the first canonical
    // PC of the next tile, even before that tile has been built.
    let source = vec![0x9100_0400; WINDOW + 1];
    let tiles = Shadows::new(source.len());
    let last = tiles.entry(&source, WINDOW - 1, 0).unwrap().unwrap();
    assert_eq!(tiles.original_offset(last + 4), Some(WINDOW * 4));
    assert_eq!(tiles.cached_entry(WINDOW), None);
    let next = tiles.entry(&source, WINDOW, 0).unwrap().unwrap();
    assert_eq!(tiles.original_offset(next), Some(WINDOW * 4));

    let x18_loop = [0x9100_0652, 0xf100_0463, 0x54ff_ffc1]; // add x18; subs x3; b.ne
    let window = Window::build(&x18_loop, 0).unwrap();
    assert!(window.entry(0).is_none());
    assert!(window.entry(1).is_some());
    assert!(window.entry(2).is_none());
}

#[test]
fn veneer_stops_track_retirement_and_skip_literals() {
    // Use the second tile of an interval with an unaligned page offset.
    let base = 0x78_0000_0004u64;
    let mut source = vec![0xd503_201f; WINDOW];
    source.extend([
        0x9400_0002, // bl +8
        0x1000_0012, // adr x18,.
        0x97ff_fffe, // bl -8
        0x9000_0012, // adrp x18,.
        0x97ff_fffb, // bl into the previous tile
        0xf0ff_ffe3, // adrp x3,.-4096
        0x7000_0004, // adr x4,.+3 (fallthrough to EXIT)
    ]);
    let shadows = Shadows::new(source.len());
    let entry = shadows.entry(&source, WINDOW, base).unwrap().unwrap();
    for i in [1, 3, 4] {
        assert_eq!(shadows.cached_entry(WINDOW + i), None);
    }
    let tile_base = base + (WINDOW * 4) as u64;
    for (i, next, rd, value) in [
        (0, 2, 30, tile_base + 4),
        (2, 0, 30, tile_base + 12),
        (5, 6, 3, (tile_base & !4095) - 4096),
        (6, 7, 4, tile_base + 27),
    ] {
        let slot = entry + i as u64 * 4;
        let word = unsafe { (slot as *const u32).read() };
        assert_eq!(word >> 26, 5); // b veneer
        let ldr_pc = slot.wrapping_add_signed(super::super::signed(word, 26) * 4);
        let words = unsafe { std::slice::from_raw_parts(ldr_pc as *const u32, 4) };
        assert_eq!(words[0], 0x5800_0040 | rd);
        assert_eq!(u64::from(words[2]) | u64::from(words[3]) << 32, value);
        let resume = (ldr_pc + 4).wrapping_add_signed(super::super::signed(words[1], 26) * 4);
        assert_eq!(resume, entry + next as u64 * 4);
        for pc in [slot, ldr_pc] {
            assert_eq!(shadows.original_offset(pc), Some((WINDOW + i) * 4));
        }
        for pc in [ldr_pc + 4, resume] {
            assert_eq!(shadows.original_offset(pc), Some((WINDOW + next) * 4));
        }
        assert_eq!(shadows.original_offset(ldr_pc + 8), None);
        assert_eq!(shadows.original_offset(ldr_pc + 12), None);
    }
}

#[test]
fn vector_instructions_execute_directly_with_guest_data_addresses() {
    let words = [
        0x4ea0_8652, // add v18.4s,v18.4s,v0.4s
        0x4c40_7020, // ld1 {v0.16b},[x1]
        0x9e66_0012, // fmov x18,d0
        0x4c40_7240, // ld1 {v0.16b},[x18]
        0x5800_0000, // ldr x0, literal
    ];
    let window = Window::build(&words, 0).unwrap();
    for (index, word) in words.into_iter().enumerate() {
        if index < 2 {
            let entry = window.entry(index).unwrap();
            assert_eq!(unsafe { (entry as *const u32).read() }, word);
        } else {
            assert!(window.entry(index).is_none());
        }
    }
}

#[test]
fn direct_shadow_entry_recovers_foreign_faults_and_interrupts() {
    const CHILD: &str = "SCARLET_NATIVE_SHADOW_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "fetched_instruction::shadow::tests::direct_shadow_entry_recovers_foreign_faults_and_interrupts",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success(), "shadow worker exited with {status}");
        return;
    }

    let page = host_page_size().unwrap();
    let arena = RamArena::new(page as u64, page * 3).unwrap();
    let base = 0x78_0000_0000;
    let caller = base + page;
    let data = caller + page;
    let offset = page / 4;
    let entry = base + offset;
    let words = [
        0xb400_0063u32, // cbz x3, index 3
        0xf100_0463,    // subs x3,x3,#1
        0x54ff_ffe1,    // b.ne index 1
        0x3700_0042,    // tbnz w2,#0,index 5
        0x9100_04c6,    // add x6,x6,#1
        0xf800_84a4,    // str x4,[x5],#8
        0xd65f_03c0,    // ret to the canonical caller
        0xb900_00a4,    // str w4,[x5] (notify the interrupting thread)
        0x9100_04e7,    // add x7,x7,#1
        0x17ff_ffff,    // b index 8
        0x1400_0001,    // b into the NX hole
        0,              // NX hole at index 11
        0x70ff_ffe9,    // adr x9,.-1
        0xf0ff_ffea,    // adrp x10,.-4096
        0x9400_0002,    // bl index 16
        0xd400_0121,    // svc #9 (return from callee)
        0xaa1e_03eb,    // mov x11,x30
        0xd65f_03c0,    // ret
    ];
    let source = arena.resolve(page as u64, page).unwrap();
    let text = arena.resolve(2 * page as u64, page).unwrap();
    let ram = arena.resolve(3 * page as u64, page).unwrap();
    let mut mappings = BTreeMap::new();
    let fetch = [
        offset..offset + 44,
        offset + 48..offset + size_of_val(&words),
    ];
    unsafe {
        source
            .alias
            .as_ptr()
            .add(offset)
            .cast::<u32>()
            .copy_from_nonoverlapping(words.as_ptr(), words.len());
        let stub = [0xd63f_0200u32, 0xd400_00e1]; // blr x16; svc #7
        text.alias
            .as_ptr()
            .cast::<u32>()
            .copy_from_nonoverlapping(stub.as_ptr(), stub.len());
        mappings.insert(
            base,
            Mapping::install(
                base,
                page,
                source.backing,
                source.offset,
                Protection::ExecuteOnFetch(&fetch),
            )
            .unwrap(),
        );
        mappings.insert(
            caller,
            Mapping::install(
                caller,
                page,
                text.backing,
                text.offset,
                Protection::ReadExecute(&[0..8]),
            )
            .unwrap(),
        );
        mappings.insert(
            data,
            Mapping::install(data, page, ram.backing, ram.offset, Protection::ReadWrite).unwrap(),
        );
    }
    let interrupt = Arc::new(Interrupt::default());
    let mut thread = Thread::new(interrupt.clone()).unwrap();
    let mut regs = Registers {
        pc: caller as u64,
        ..Default::default()
    };
    regs.x[2] = 1;
    regs.x[3] = 900;
    regs.x[4] = 42;
    regs.x[5] = base as u64; // canonical guest view denies the store
    regs.x[16] = mappings[&base].shadow_entry(entry).unwrap().unwrap();
    regs.x[18] = 0xbeef;

    let fault = thread.run(1, regs, 0, &mappings).unwrap();
    assert_eq!(fault.esr >> 26, 0x24);
    assert_eq!(fault.registers.pc, (entry + 20) as u64);
    assert_eq!(fault.far, base as u64);
    assert_eq!(fault.registers.x[3], 0);
    assert_eq!(fault.registers.x[5], base as u64);
    assert_eq!(fault.registers.x[6], 0);
    assert_eq!(fault.registers.x[18], 0xbeef);
    assert_eq!(fault.registers.x[30], (caller + 4) as u64);

    let mut retry = fault.registers;
    retry.x[5] = data as u64;
    let svc = thread.run(2, retry, 0, &mappings).unwrap();
    assert_eq!(svc.esr, 0x5600_0007);
    assert_eq!(svc.registers.pc, (caller + 8) as u64);
    assert_eq!(svc.registers.x[5], (data + 8) as u64);
    assert_eq!(unsafe { ram.alias.as_ptr().cast::<u64>().read() }, 42);

    // Enter from canonical text through its actual NX fault. The signal
    // handler can now resume directly into the already published tile.
    assert!(mappings[&base].cached_shadow_entry(entry + 28).is_some());
    regs.x[16] = (entry + 28) as u64;
    regs.x[5] = (data + 8) as u64;
    let ready = unsafe { &*ram.alias.as_ptr().add(8).cast::<AtomicU32>() };
    let stop = std::thread::scope(|scope| {
        scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while ready.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
                std::thread::yield_now();
            }
            interrupt.request(3).unwrap();
        });
        thread.run(3, regs, 0, &mappings).unwrap()
    });
    assert!(stop.interrupted);
    assert_eq!(ready.load(Ordering::Acquire), 42);
    assert!([entry as u64 + 32, entry as u64 + 36].contains(&stop.registers.pc));
    assert_eq!(stop.registers.x[18], 0xbeef);
    assert_eq!(stop.registers.x[30], (caller + 4) as u64);

    regs.pc = (entry + 40) as u64;
    let denied = thread.run(4, regs, 0, &mappings).unwrap();
    assert_eq!(denied.registers.pc, (entry + 44) as u64);
    assert_eq!(denied.esr >> 26, 0x20);

    regs.pc = (entry + 48) as u64;
    let call = thread.run(5, regs, 0, &mappings).unwrap();
    assert_eq!(call.esr, 0x5600_0009);
    assert_eq!(call.registers.pc, (entry + 64) as u64);
    assert_eq!(call.registers.x[9], (entry + 47) as u64);
    assert_eq!(call.registers.x[10], ((entry + 52) as u64 & !4095) - 4096);
    assert_eq!(call.registers.x[11], (entry + 60) as u64);
    assert_eq!(call.registers.x[30], (entry + 60) as u64);
    assert_eq!(call.registers.x[18], 0xbeef);
}
