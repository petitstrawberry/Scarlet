.section .rodata
.Lroot_path:
    .asciz "/var/lib/scarlet/boot-check"
.Lroot_byte:
    .byte 0x53
.Lroot_written:
    .ascii "SCARLET_ROOT_WRITTEN\n"
.set .Lwritten_size, . - .Lroot_written
.Lroot_reopened:
    .ascii "SCARLET_ROOT_REOPENED\n"
.set .Lreopened_size, . - .Lroot_reopened
.Lpass_message:
    .ascii "SCARLET_BOOT_PASS\n"
.set .Lpass_size, . - .Lpass_message
.Ltty_message:
    .ascii "guest: "
.set .Ltty_size, . - .Ltty_message
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

    // The boot fixture lives in the current signed bundle's initramfs.
    // The absolute path below resolves in the writable root after pivot_root.
    mov x0, #-1 // CURRENT_DIRECTORY
    adr x1, .Lroot_path
    mov x2, #0x42 // O_RDWR | O_CREAT
    mov x3, #0600
    mov x8, #417 // VfsOpenAt
    svc #0
    tbnz x0, #63, .Lfail
    mov x20, x0
    sub sp, sp, #16
    mov x1, sp
    mov x2, #1
    mov x8, #200 // StreamRead
    svc #0
    cbz x0, .Lwrite_root
    cmp x0, #1
    b.ne .Lfail
    ldrb w9, [sp]
    cmp w9, #0x53
    b.ne .Lfail
    adr x1, .Lroot_reopened
    mov x2, #.Lreopened_size
    b .Lroot_result
.Lwrite_root:
    mov x0, x20
    adr x1, .Lroot_byte
    mov x2, #1
    mov x8, #201 // StreamWrite
    svc #0
    cmp x0, #1
    b.ne .Lfail
    mov x0, x20
    mov x8, #304 // FileSync
    svc #0
    cbnz x0, .Lfail
    adr x1, .Lroot_written
    mov x2, #.Lwritten_size
.Lroot_result:
    add sp, sp, #16
    mov x0, #1
    mov x8, #201 // StreamWrite
    svc #0
    mov x0, x20
    mov x8, #102 // HandleClose
    svc #0

    mov x0, #1
    adr x1, .Lpass_message
    mov x19, #.Lpass_size
    mov x2, x19
    mov x8, #201 // StreamWrite
    svc #0
    cmp x0, x19
    b.ne .Lfail
    sub sp, sp, #256
.Lread:
    mov x0, #0
    mov x1, sp
    mov x2, #256
    mov x8, #200 // StreamRead from the inherited TTY
    svc #0
    cmp x0, #0
    b.le .Lidle
    mov x19, x0
    mov x0, #1
    adr x1, .Ltty_message
    mov x2, #.Ltty_size
    mov x8, #201 // StreamWrite
    svc #0
    mov x0, #1
    mov x1, sp
    mov x2, x19
    mov x8, #201 // StreamWrite
    svc #0
    b .Lread
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
