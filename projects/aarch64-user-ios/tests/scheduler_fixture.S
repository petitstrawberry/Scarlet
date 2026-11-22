.text
.p2align 2
.globl _scarlet_scheduler_start
_scarlet_scheduler_start:
    mov x19, x5
    mov x20, x6
    mov x21, x7
    mov x24, x9
    mov x27, x4
    mov x16, x4
    mov x17, x4
    movi v31.16b, #0x5a
    mov x22, #0

    cmp x0, #1
    b.ne Lscarlet_scheduler_fail
    ldr x13, [sp]
    cmp x13, x0
    b.ne Lscarlet_scheduler_fail
    ldr x13, [x1]
    ldrb w14, [x13]
    cmp w14, #'/'
    b.ne Lscarlet_scheduler_fail
    ldrb w14, [x13, #1]
    cmp w14, #'c'
    b.eq Lscarlet_scheduler_child
    mov x29, x1
    add x12, sp, #32
Lscarlet_scheduler_aux:
    ldp x13, x14, [x12], #16
    cbz x13, Lscarlet_scheduler_fail
    cmp x13, #6
    b.ne Lscarlet_scheduler_aux
    cmp x14, #4, lsl #12
    b.ne Lscarlet_scheduler_fail
    sub sp, sp, #16
    str x3, [sp]

    mrs x23, tpidr_el0
    msr tpidr_el0, x27
    mrs x14, tpidr_el0
    cmp x14, x27
    b.ne Lscarlet_scheduler_fail
    msr tpidr_el0, x23
    ldr x13, [x23, #16]
    cmp x13, #73
    b.ne Lscarlet_scheduler_fail
    ldr x13, [x23, #24]
    cbnz x13, Lscarlet_scheduler_fail
    add x13, x21, #74
    str x13, [x23, #16]
    add x13, x21, #1
    str x13, [x23, #24]

    adrp x9, guest_got
    add x9, x9, :lo12:guest_got
    ldr x28, [x9]
    adrp x10, guest_counter
    add x10, x10, :lo12:guest_counter
    cmp x10, x28
    b.ne Lscarlet_scheduler_fail
    ldr x13, [x28]
    cmp x13, #40
    b.ne Lscarlet_scheduler_fail
    add x13, x21, #41
    str x13, [x28]
    ldr x12, [x9, #8]
    blr x12
    cmp x0, #42
    b.ne Lscarlet_scheduler_fail
    adrp x9, guest_bss
    add x9, x9, :lo12:guest_bss
    ldr x13, [x9]
    cbnz x13, Lscarlet_scheduler_fail
    add x13, x21, #1
    str x13, [x9]

    // This data word in the executable segment matches the SVC encoding.
    ldr w13, Lscarlet_scheduler_constant
    movz w12, #1
    movk w12, #0xd400, lsl #16
    cmp w13, w12
    b.ne Lscarlet_scheduler_fail

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

    mov x8, #35
    svc #0
    mov x25, x0
    ldar x26, [x20]
    mov x0, x24
    mov x8, #20
    svc #0
    cbnz x0, Lscarlet_scheduler_fail
    ldar x13, [x20]
    cmp x13, x26
    b.ls Lscarlet_scheduler_fail
    mov x8, #35
    svc #0
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
    adrp x0, unsigned_path
    add x0, x0, :lo12:unsigned_path
    mov x1, x29
    mov x2, #0
    adrp x3, abi_name
    add x3, x3, :lo12:abi_name
    mov x4, #0
    mov x8, #4
    svc #0
    cmn x0, #1
    b.ne Lscarlet_scheduler_fail

    sub sp, sp, #1024
    mov x0, sp
    mov x1, #1024
    mov x8, #51
    svc #0
    add sp, sp, #1024
    cbz x0, Lscarlet_scheduler_fail
    cmn x0, #1
    b.eq Lscarlet_scheduler_fail
    // Usercopy must reject addresses owned by the app instead of this guest.
    mov x0, x20
    mov x1, #1024
    mov x8, #51
    svc #0
    cmn x0, #1
    b.ne Lscarlet_scheduler_fail

    mov x0, #20480
    mov x8, #13
    svc #0
    cmn x0, #1
    b.eq Lscarlet_scheduler_fail
    mov x26, x0
    str x27, [x26]
    str x27, [x26, #16384]
    mov x12, #20480
    neg x0, x12
    mov x8, #13
    svc #0
    add x13, x26, #5, lsl #12
    cmp x0, x13
    b.ne Lscarlet_scheduler_fail
    // The retired heap range is no longer writable by the kernel.
    mov x0, x26
    mov x1, #1024
    mov x8, #51
    svc #0
    cmn x0, #1
    b.ne Lscarlet_scheduler_fail
    mov x0, #0x900000
    mov x8, #13
    svc #0
    cmn x0, #1
    b.ne Lscarlet_scheduler_fail
    mov x0, #20480
    mov x8, #13
    svc #0
    cmp x0, x26
    b.ne Lscarlet_scheduler_fail
    ldr x13, [x26]
    cbnz x13, Lscarlet_scheduler_fail
    ldr x13, [x26, #16384]
    cbnz x13, Lscarlet_scheduler_fail

    cmp x16, x27
    b.ne Lscarlet_scheduler_fail
    cmp x17, x27
    b.ne Lscarlet_scheduler_fail
    umov w13, v31.b[15]
    cmp w13, #0x5a
    b.ne Lscarlet_scheduler_fail
    mrs x14, tpidr_el0
    cmp x14, x23
    b.ne Lscarlet_scheduler_fail
    ldr x13, [x14, #16]
    add x12, x21, #74
    cmp x13, x12
    b.ne Lscarlet_scheduler_fail
    ldr x13, [x14, #24]
    add x12, x21, #1
    cmp x13, x12
    b.ne Lscarlet_scheduler_fail
    ldr x13, [x28]
    add x12, x21, #41
    cmp x13, x12
    b.ne Lscarlet_scheduler_fail

    cbnz x21, Lscarlet_scheduler_ok
    // A VM-copying clone fails before allocating or publishing a child.
    mov x0, #0
    mov x8, #2
    svc #0
    cmn x0, #1
    b.ne Lscarlet_scheduler_fail

    mov x8, #1304
    svc #0
    cmn x0, #1
    b.eq Lscarlet_scheduler_fail
    ldr x1, [sp]
    sub sp, sp, #80
    mov x12, #48
    add x13, sp, #48
    stp x12, x13, [sp]
    stp xzr, xzr, [sp, #16]
    stp xzr, xzr, [sp, #32]
    adrp x12, child_path
    add x12, x12, :lo12:child_path
    stp x12, xzr, [sp, #48]
    mov x2, sp
    adrp x3, abi_name
    add x3, x3, :lo12:abi_name
    mov x8, #1306
    svc #0
    cmn x0, #1
    b.eq Lscarlet_scheduler_fail
    cbz x0, Lscarlet_scheduler_fail
    mov x25, x0
    add x1, sp, #64
    mov x2, #0
    mov x8, #5
    svc #0
    cmp x0, x25
    b.ne Lscarlet_scheduler_fail
    ldr w12, [sp, #64]
    add sp, sp, #80
    cmp w12, #42
    b.ne Lscarlet_scheduler_fail

Lscarlet_scheduler_ok:
    mov x0, #0
    mov x13, #1
    b Lscarlet_scheduler_publish
Lscarlet_scheduler_fail:
    mov x0, #1
    mov x13, #2
Lscarlet_scheduler_publish:
    add x12, x19, #8
    stlr x13, [x12]
    mov x8, #1
    svc #0
    brk #0x5101

Lscarlet_scheduler_child:
    mov x0, #42
    mov x8, #1
    svc #0
    brk #0x5101

guest_helper:
    mov x0, #42
    ret

Lscarlet_scheduler_constant:
    .word 0xd4000001

.section .rodata,"a",@progbits
unsigned_path:
    .asciz "/unsigned"
child_path:
    .asciz "/child"
abi_name:
    .asciz "scarlet"

.data
.p2align 3
guest_counter:
    .quad 40

.bss
.p2align 3
guest_bss:
    .zero 8

.section .data.rel.ro,"aw",@progbits
.p2align 3
guest_got:
    .quad guest_counter
    .quad guest_helper

.section .tdata,"awT",@progbits
.p2align 3
    .quad 73

.section .tbss,"awT",@nobits
.p2align 3
    .zero 8
