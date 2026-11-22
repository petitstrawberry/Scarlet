// Ordinary Linux guest, linked with 4 KiB PT_LOADs. SPLIT_DATA also
// exercises a successful native read followed by a mediated store.
.text
.global _start
_start:
#ifdef SPLIT_DATA
    mov x0, #0
    mov x1, #16384
    mov x2, #3
    mov x3, #0x22
    mov x4, #-1
    mov x5, #0
    mov x8, #222
    svc #0
    tbnz x0, #63, failed
    add x21, x0, #4096
    mov x1, #4096
    mov x2, #1
    mov x8, #226
    svc #0
    cbnz x0, failed
#else
    adr x21, operand
#endif
    // Exercise narrow stores beside a sentinel and W writes with high bits set.
    mov x14, #-1
    str x14, [x21]
1:  mov x11, #-1
    ldaxrb w11, [x21]
    add w13, w11, #2
    stxrb w9, w13, [x21]
    cbnz w9, 1b
    cmp x11, #255
    b.ne failed
2:  ldxrh w5, [x21]
    eor w6, w5, w11
    stlxrh w3, w6, [x21]
    cbnz w3, 2b
    mov w7, #0xff01
    cmp w5, w7
    b.ne failed
    ldr w5, [x21]
    mov w7, #0xfffe
    movk w7, #0xffff, lsl #16
    cmp w5, w7
    b.ne failed
    mov x1, x21
    mov w16, #15
    // Installed librustc_driver release swap.
3:  ldxr w0, [x1]
    stlxr w17, w16, [x1]
    cbnz w17, 3b
    cmp x0, x7
    b.ne failed
    // Stock stemd __aarch64_ldadd8_relax.
4:  ldxr x0, [x1]
    add x17, x0, x16
    stxr w15, x17, [x1]
    cbnz w15, 4b
    ldr x14, [x1]
    mov x7, #-0x100000000
    add x7, x7, #30
    cmp x14, x7
    b.ne failed
    // Keep the Darwin platform register live across a load, a branch and a
    // store while an unrelated register retains its initial value.
    mov x18, x1
    mov x2, #984
5:  ldaxr x5, [x18]
    tbz x5, #1, failed
    add x18, x5, #1
    stlxr w9, x18, [x1]
    cbz w9, 6f
    mov x18, x1
    b 5b
6:  add x14, x14, #1
    cmp x18, x14
    b.ne failed
    cmp x2, #984
    b.ne failed
    // A detached store with a cleared monitor must fail even at the
    // same address and with the same value as the previous operation.
    ldxr x12, [x1]
    clrex
    cmp x12, x14
    b.ne failed
    stlxr w9, xzr, [x1]
    cbz w9, failed
    ldr x11, [x1]
    cmp x11, x14
    b.ne failed
    // Host DCZID gives the size and permission advertised by native text.
    mrs x18, dczid_el0
    mrs x6, dczid_el0
    cmp x6, x18
    b.ne failed
    tbnz x6, #4, exit_ok
    mov w7, #4
    and w6, w6, #15
    lsl w7, w7, w6
    cmp w7, #4096
    b.hi failed
    mov x18, x1
    dc zva, x18
    cmp x18, x1
    b.ne failed
    ldr x6, [x1]
    cbnz x6, failed
exit_ok:
    mov w0, #0
    b exit
failed:
    mov w0, #1
exit:
    mov w8, #93
    svc #0
    brk #1

.data
.balign 8
operand:
    .quad 0
