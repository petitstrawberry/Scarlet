// Minimal ARM64 Linux Image header accepted by crosvm's kernel loader.
// Exercise SMCCC return state, MMIO writes through KVM_RUN, and PSCI shutdown.
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
    mov x4, #0x4444
    mov x8, #0x8888
    mov x9, #0
    movz x0, #0
    movk x0, #0x8400, lsl #16
    hvc #0
    mov x9, #1
    cmp x9, #1
    b.ne failed
    mov x10, #0x4444
    cmp x4, x10
    b.ne failed
    mov x10, #0x8888
    cmp x8, x10
    b.ne failed
    movz x10, #1
    movk x10, #1, lsl #16
    cmp x0, x10
    b.ne failed
    movz x0, #0
    movk x0, #0x8400, lsl #16
    smc #0
    mov x9, #2
    cmp x9, #2
    b.ne failed
    mov x10, #0x4444
    cmp x4, x10
    b.ne failed
    adr x1, message
    b print
failed:
    adr x1, failure
print:
    mov x0, #0x3f8
1:
    ldrb w2, [x1], #1
    cbz w2, 2f
    strb w2, [x0]
    b 1b
2:
    movz x0, #8
    movk x0, #0x8400, lsl #16
    hvc #0
3:
    wfi
    b 3b
message:
    .asciz "\nSCARLET_CROSVM_GUEST_OK\n"
failure:
    .asciz "\nSCARLET_CROSVM_FAIL: SMCCC return state\n"
image_end:
