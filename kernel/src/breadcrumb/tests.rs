use super::*;

#[test_case]
fn breadcrumb_preserves_complete_wide_context() {
    let slot = BreadcrumbSlot::new();
    assert_eq!(slot.snapshot().phase, NONE);
    slot.record(VMM_WRITE_HELD, 0x1_0000_0001, u64::MAX);
    let first = slot.snapshot();
    assert_eq!(
        (first.phase, first.aux, first.aux2),
        (VMM_WRITE_HELD, 0x1_0000_0001, u64::MAX)
    );
    slot.record(VMM_WRITE_HELD, 0x2_0000_0002, 0x3_0000_0003);
    let next = slot.snapshot();
    assert_ne!(first.sequence, next.sequence);
    assert_eq!((next.aux, next.aux2), (0x2_0000_0002, 0x3_0000_0003));
}
