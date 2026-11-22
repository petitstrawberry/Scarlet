.text
.p2align 2
.globl _scarlet_scheduler_start
_scarlet_scheduler_start:
    mov x19, x0
    mov x20, x1
    mov x21, x2
    mov x24, x3
    mov x23, x4
    mov x16, x4
    mov x17, x4
    movi v31.16b, #0x5a
    mov x22, #0

.globl _scarlet_scheduler_write_tls
_scarlet_scheduler_write_tls:
    brk #0x5500

    // Advancing each round requires both guests to run without a syscall.
    add x10, x19, #16
    add x15, x20, #16
    mov x11, #1
Lscarlet_scheduler_round:
    stlr x11, [x10]
Lscarlet_scheduler_first:
    add x22, x22, #1
    stlr x22, [x19]
    ldar x13, [x15]
    cmp x13, x11
    b.lo Lscarlet_scheduler_first
    add x11, x11, #1
    cmp x11, #5
    b.lo Lscarlet_scheduler_round
    cbnz x21, Lscarlet_scheduler_peer

    mov x8, #{time}
.globl _scarlet_scheduler_before_sleep
_scarlet_scheduler_before_sleep:
    brk #0x5300
    mov x25, x0
    ldar x26, [x20]
    mov x0, x24
    mov x8, #{sleep}
.globl _scarlet_scheduler_sleep
_scarlet_scheduler_sleep:
    brk #0x5300
    cbnz x0, Lscarlet_scheduler_fail
    ldar x13, [x20]
    cmp x13, x26
    b.ls Lscarlet_scheduler_fail
    mov x8, #{time}
.globl _scarlet_scheduler_after_sleep
_scarlet_scheduler_after_sleep:
    brk #0x5300
    sub x0, x0, x25
    cmp x0, x24
    b.lo Lscarlet_scheduler_fail
    b Lscarlet_scheduler_check

Lscarlet_scheduler_peer:
    add x12, x20, #8
Lscarlet_scheduler_spin:
    add x22, x22, #1
    stlr x22, [x19]
    ldar x13, [x12]
    cbz x13, Lscarlet_scheduler_spin
    cmp x13, #1
    b.ne Lscarlet_scheduler_fail

Lscarlet_scheduler_check:
    cmp x16, x23
    b.ne Lscarlet_scheduler_fail
    cmp x17, x23
    b.ne Lscarlet_scheduler_fail
    umov w13, v31.b[15]
    cmp w13, #0x5a
    b.ne Lscarlet_scheduler_fail
.globl _scarlet_scheduler_read_tls
_scarlet_scheduler_read_tls:
    brk #0x5400
    cmp x14, x23
    b.ne Lscarlet_scheduler_fail
    mov x0, #0
    mov x13, #1
    b Lscarlet_scheduler_publish
Lscarlet_scheduler_fail:
    mov x0, #1
    mov x13, #2
Lscarlet_scheduler_publish:
    add x12, x19, #8
    stlr x13, [x12]
    mov x8, #{exit}
.globl _scarlet_scheduler_exit
_scarlet_scheduler_exit:
    brk #0x5300
    brk #0x5101

.globl _scarlet_scheduler_end
_scarlet_scheduler_end:
