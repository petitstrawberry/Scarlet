//! No guest execution: distinguish arena address space from resident backing.
use scarlet_hosted_runtime::ram::{RamArena, host_page_size};

fn resident_bytes(address: usize, length: usize) -> usize {
    let page = host_page_size().unwrap();
    let mut residency = vec![0u8; length / page];
    assert_eq!(
        unsafe {
            libc::mincore(
                address as *mut libc::c_void,
                length,
                residency.as_mut_ptr().cast(),
            )
        },
        0,
        "mincore: {}",
        std::io::Error::last_os_error()
    );
    residency.iter().filter(|&&state| state & 1 != 0).count() * page
}

#[test]
fn eight_gib_arena_is_demand_resident_and_init_touches_metadata_only() {
    const BYTES: usize = 8 * 1024 * 1024 * 1024;
    const BASE: u64 = 0x10000;
    let page = host_page_size().unwrap();
    let arena = RamArena::new(BASE, BYTES).unwrap();
    let alias = arena.resolve(BASE, BYTES).unwrap().alias.as_ptr();
    let fresh = resident_bytes(alias as usize, BYTES);
    assert_eq!(fresh, 0);
    unsafe {
        alias.write_volatile(1);
        alias.add(BYTES - 1).write_volatile(1);
    }
    let touched = resident_bytes(alias as usize, BYTES);
    assert_eq!(touched, 2 * page);
    drop(arena);

    let _platform = scarlet::hosted::prepare(4, BYTES).unwrap();
    let initialized = resident_bytes(scarlet::vm::phys_to_virt(BASE), BYTES);
    // Current PMM has a 24-byte descriptor per 4 KiB page: 48 MiB at 8 GiB.
    // Leave room for small startup allocations while rejecting eager RAM touch.
    assert!(initialized < 128 * 1024 * 1024, "resident={initialized}");
    println!(
        "arena virtual={BYTES} fresh_resident={fresh} two_pages_resident={touched} initialized_resident={initialized}"
    );
}
