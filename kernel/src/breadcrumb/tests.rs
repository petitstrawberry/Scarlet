use core::cell::Cell;

use super::*;

#[test_case]
fn snapshot_retries_when_same_phase_writer_changes_generation() {
    // Given: one complete VW breadcrumb and a writer that will publish the
    // next VW breadcrumb after the reader has sampled the old aux value.
    let slot = BreadcrumbSlot::new();
    slot.record(VMM_WRITE_HELD, 0x5241, 0x1111);
    let writer_ran = Cell::new(false);

    // When: the second record lands in the middle of the first read.
    let snapshot = slot
        .snapshot_with_probe(|| {
            if !writer_ran.replace(true) {
                slot.record(VMM_WRITE_HELD, 0x5252, 0x2222);
            }
        })
        .expect("a completed generation must be observable");

    // Then: the reader retries and returns one complete generation rather
    // than combining the old aux with the new aux2.
    assert_eq!(snapshot.phase, VMM_WRITE_HELD);
    assert_eq!(snapshot.aux, 0x5252);
    assert_eq!(snapshot.aux2, 0x2222);
}

#[test_case]
fn snapshot_reports_writer_stopped_mid_publication() {
    // Given: a CPU stopped after marking its slot odd but before publishing
    // a complete tuple.
    let slot = BreadcrumbSlot::new();
    slot.sequence.store(1, Ordering::SeqCst);

    // When: the public bounded snapshot exhausts its retry budget.
    let snapshot = slot.snapshot();

    // Then: diagnostics expose the incomplete writer without returning a
    // mixed tuple or omitting the CPU entirely.
    assert_eq!(snapshot.phase, PUBLICATION_IN_PROGRESS);
    assert_eq!(snapshot.aux, 1);
    assert_eq!(snapshot.aux2, 0);
}
