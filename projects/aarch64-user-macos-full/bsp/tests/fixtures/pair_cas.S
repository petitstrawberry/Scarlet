// The test links .data into one writable 4 KiB quarter of a Darwin page.
// Check auxv and both CASP widths through that mapping.
.arch armv8.1-a+lse
.global _start
.text
_start:
    ldr x9, [sp]
    add x10, sp, #16
    add x10, x10, x9, lsl #3
1:  ldr x9, [x10], #8
    cbnz x9, 1b
2:  ldp x9, x11, [x10], #16
    cbz x9, fail
    cmp x9, #16                 // AT_HWCAP
    b.ne 2b
    tbz x11, #8, fail          // HWCAP_ATOMICS

    adrp x5, pair
    add x5, x5, :lo12:pair
    mov x0, #0x100000000
    mov x1, #0x200000000
    mov x30, #13
    caspal x0, x1, x30, xzr, [x5]
    mov x9, #0x100000000
    cmp x0, x9
    b.ne fail
    mov x9, #0x200000000
    cmp x1, x9
    b.ne fail
    // The old expected value no longer matches. Return the current pair.
    casp x0, x1, x30, xzr, [x5]
    cmp x0, #13
    b.ne fail
    cbnz x1, fail
    mov w2, #19
    mov w3, #23
    caspa w0, w1, w2, w3, [x5]
    cmp x0, #13
    b.ne fail
    cbnz x1, fail
    caspl w0, w1, w2, w3, [x5]
    cmp x0, #19
    b.ne fail
    cmp x1, #23
    b.ne fail
    mov w18, #19
    mov w19, #23
    mov w20, #77
    mov w21, #88
    casp w18, w19, w20, w21, [x5]
    caspal w18, w19, w20, w21, [x5]
    cmp x18, #77
    b.ne fail
    cmp x19, #88
    b.ne fail
    mov x19, x5
    mov w0, #77
    mov w1, #88
    mov w18, #111
    // x19 is both the top half of the replacement and the addressing base.
    casp w0, w1, w18, w19, [x19]
    ldr w9, [x5]
    cmp w9, #111
    b.ne fail
    ldr w9, [x5, #4]
    cmp w9, w5
    b.ne fail
    mov x0, #0
    b exit
fail:
    mov x0, #1
exit:
    mov x8, #1                // Native Scarlet Exit
    svc #0

.data
.balign 16
pair: .quad 0x100000000, 0x200000000
