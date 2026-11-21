// Parent and child increment a shared anonymous word. The parent splits
// its data permissions after fork while the child retains its full RW mapping.
.text
.global _start
_start:
    mov x0, #0
    mov x1, #16384
    mov x2, #3
    mov x3, #0x21                // MAP_SHARED | MAP_ANONYMOUS
    mov x4, #-1
    mov x5, #0
    mov x8, #222
    svc #0
    tbnz x0, #63, failed
    mov x19, x0
    add x21, x0, #4096
    str xzr, [x19]
    str xzr, [x19, #4096]
    str xzr, [x19, #8192]
    str xzr, [x19, #12288]
    mov x0, #17                  // clone(SIGCHLD, NULL)
    mov x1, #0
    mov x2, #0
    mov x3, #0
    mov x4, #0
    mov x8, #220
    svc #0
    tbnz x0, #63, failed
    cbz x0, child
    mov x22, x0
    mov x0, x19
    mov x1, #4096
    mov x2, #1
    mov x8, #226                 // mediate only the parent's alias
    svc #0
    cbnz x0, failed
    mov w0, #1
    add x20, x21, #8
    stlr w0, [x20]
    bl increment
    mov x0, x22
    sub sp, sp, #16
    mov x1, sp
    mov x2, #0
    mov x3, #0
    mov x8, #260                 // wait4
    svc #0
    cmp x0, x22
    b.ne failed
    ldr w0, [sp]
    cbnz w0, failed
    ldr x0, [x21]
    cmp x0, #2048
    b.ne failed
    b passed
child:
    add x20, x21, #8
2:  ldar w0, [x20]
    cbz w0, 2b
    bl increment
passed:
    mov w0, #0
    b exit
failed:
    mov w0, #1
exit:
    mov w8, #93
    svc #0
    brk #1

increment:
    mov w4, #1024
1:  ldaxr x11, [x21]
    add x13, x11, #1
    stlxr w9, x13, [x21]
    cbnz w9, 1b
    subs w4, w4, #1
    b.ne 1b
    ret
