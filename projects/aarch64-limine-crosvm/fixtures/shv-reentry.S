// ARM64 Linux Image accepted by crosvm. Check timer emulation and repeated
// in-kernel exits with a live stage-2 working set. Print architected ticks so
// boot/image-preparation time does not enter the measurement.
.section .text
.global _start
_start:
    b entry
    .long 0
    .quad 0x80000
    .quad image_end - _start
    .quad 0
    .quad 0, 0, 0
    .long 0x644d5241
    .long 0
entry:
    adr x19, working_set
#ifndef BENCH_ONLY
    // Writes through XZR must mean zero, not "leave the old register alone".
    mov x8, #3
    msr cntv_ctl_el0, x8
    msr cntv_ctl_el0, xzr
    mrs x8, cntv_ctl_el0
    tst x8, #3
    b.ne timer_zero_failed
    // TVAL is a signed 32-bit relative value, even when written via Xt.
    mov w8, #-1
    msr cntv_tval_el0, x8
    mrs x8, cntv_cval_el0
    mrs x9, cntvct_el0
    cmp x8, x9
    b.hi timer_signed_failed
    mov x8, #3
    msr cntv_ctl_el0, x8
    mrs x8, cntv_tval_el0
    lsr x8, x8, #32
    cbnz x8, timer_signed_failed
    msr cntv_cval_el0, xzr
    mrs x8, cntv_cval_el0
    cbnz x8, timer_zero_failed
    mrs x10, cntvct_el0
    msr cntv_tval_el0, xzr
    mrs x8, cntv_cval_el0
    mrs x9, cntvct_el0
    cmp x8, x10
    b.lo timer_zero_failed
    cmp x8, x9
    b.hi timer_zero_failed
    msr cntv_ctl_el0, xzr
#endif
    // Read first, then write: stage-2 read-only -> writable updates must remain
    // visible even if redundant re-entry invalidations are removed.
    mov x20, #0
1:
    add x8, x19, x20, lsl #12
    ldr x9, [x8]
    cbnz x9, memory_failed
    str x20, [x8]
    add x20, x20, #1
    cmp x20, #64
    b.ne 1b
    mrs x22, cntfrq_el0
    mrs x23, cntvct_el0
    mov x21, #10000
2:
    mrs x8, cntv_ctl_el0 // trapped, but no page-table change
    mov x20, #0
3:
    add x8, x19, x20, lsl #12
    ldr x9, [x8]
    cmp x9, x20
    b.ne memory_failed
    add x20, x20, #1
    cmp x20, #64
    b.ne 3b
    subs x21, x21, #1
    b.ne 2b
    mrs x24, cntvct_el0
    sub x24, x24, x23
    adr x1, ticks
    bl print
    mov x3, x24
    bl print_hex
    adr x1, frequency
    bl print
    mov x3, x22
    bl print_hex
    adr x1, success
    bl print
    b shutdown
timer_zero_failed:
    adr x1, zero_failure
    b fail
timer_signed_failed:
    adr x1, signed_failure
    b fail
memory_failed:
    adr x1, memory_failure
fail:
    bl print
shutdown:
    movz x0, #8
    movk x0, #0x8400, lsl #16
    hvc #0
4:
    wfi
    b 4b
print:
    mov x0, #0x3f8
5:
    ldrb w2, [x1], #1
    cbz w2, 6f
    strb w2, [x0]
    b 5b
6:
    ret
print_hex:
    mov x0, #0x3f8
    mov x4, #60
7:
    lsr x2, x3, x4
    and w2, w2, #15
    cmp w2, #10
    add w5, w2, #48
    add w6, w2, #87
    csel w2, w5, w6, lo
    strb w2, [x0]
    subs x4, x4, #4
    b.ge 7b
    ret
ticks: .asciz "\nSCARLET_SHV_TICKS "
frequency: .asciz "\nSCARLET_SHV_FREQUENCY "
success: .asciz "\nSCARLET_SHV_REENTRY_OK\n"
zero_failure: .asciz "\nSCARLET_CROSVM_FAIL: timer XZR write ignored\n"
signed_failure: .asciz "\nSCARLET_CROSVM_FAIL: timer TVAL sign extension\n"
memory_failure: .asciz "\nSCARLET_CROSVM_FAIL: stage2 working set\n"
.balign 4096
working_set: .space 64 * 4096
image_end:
