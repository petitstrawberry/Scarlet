.section .rodata
.Lpass_message:
    .ascii "SCARLET_BOOT_PASS\n"
.set .Lpass_size, . - .Lpass_message
.Lfail_message:
    .ascii "SCARLET_BOOT_FAIL\n"
.set .Lfail_size, . - .Lfail_message

.text
.p2align 2
.globl _scarlet_boot_start
_scarlet_boot_start:
    cmp x0, #1
    b.ne .Lfail
    mov x8, #7 // Getpid
    svc #0
    cmp x0, #1
    b.ne .Lfail
    mov x8, #1304 // EnvironmentCurrent
    svc #0
    tbnz x0, #63, .Lfail

    mov x0, #1
    adr x1, .Lpass_message
    mov x19, #.Lpass_size
    mov x2, x19
    mov x8, #201 // StreamWrite
    svc #0
    cmp x0, x19
    b.ne .Lfail
.Lidle:
    movz x0, #0xca00
    movk x0, #0x3b9a, lsl #16 // 1 second in nanoseconds
    mov x8, #20 // Sleep
    svc #0
    b .Lidle
.Lfail:
    mov x0, #2
    adr x1, .Lfail_message
    mov x2, #.Lfail_size
    mov x8, #201 // StreamWrite
    svc #0
    b .Lidle
