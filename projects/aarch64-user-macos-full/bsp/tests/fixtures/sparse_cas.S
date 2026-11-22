// Linux AArch64 guest. Link .text and .data as individual 4 KiB PT_LOADs
// with holes in their 16 KiB host pages to exercise checked data access.
.text
.global _start
_start:
#ifdef SPLIT_DATA
    // The canonical host page can be readable but not writable when only
    // its first quarter is guest read-only. The CAS targets the second one.
    mov x0, #0
    mov x1, #16384
    mov x2, #3
    mov x3, #0x22
    mov x4, #-1
    mov x5, #0
    mov x8, #222
    svc #0
    tbnz x0, #63, failed
    mov x19, x0
    mov w1, #7
    str w1, [x19, #4096]
    mov x1, #4096
    mov x2, #1
    mov x8, #226
    svc #0
    cbnz x0, failed
    add x2, x19, #4096
#else
    adr x2, operand
#endif
    mov w16, #7
    mov w1, #9
    bl retry
    b.ne failed
    cbnz w17, failed
    cmp w0, #7
    b.ne failed
    mov w16, #8
    mov w17, #123
    bl retry
    b.eq failed
    cmp w17, #123
    b.ne failed
    cmp w0, #9
    b.ne failed
    ldr w3, [x2]
    cmp w3, #9
    b.ne failed
    mov x0, #0
    b exit
failed:
    mov x0, #1
exit:
    mov x8, #93
    svc #0
    brk #1

// The compiler-rt __aarch64_cas4_acq retry loop used by /init.
retry:
    ldaxr w0, [x2]
    cmp w0, w16
    b.ne done
    stxr w17, w1, [x2]
    cbnz w17, retry
done:
    ret

.data
.balign 4
operand:
    .word 7
