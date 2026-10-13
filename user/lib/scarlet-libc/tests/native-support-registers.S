// Native AAPCS64 register-preservation probe. No foreign ABI/syscalls.
.text
.p2align 2
.global test_callee_saved
.type test_callee_saved,%function
test_callee_saved:
    sub sp, sp, #352
    stp x19, x20, [sp, #0]
    stp x21, x22, [sp, #16]
    stp x23, x24, [sp, #32]
    stp x25, x26, [sp, #48]
    stp x27, x28, [sp, #64]
    stp x29, x30, [sp, #80]
    stp d8, d9, [sp, #96]
    stp d10, d11, [sp, #112]
    stp d12, d13, [sp, #128]
    stp d14, d15, [sp, #144]
    mov x19, #275
    mov x20, #276
    mov x21, #277
    mov x22, #278
    mov x23, #279
    mov x24, #280
    mov x25, #281
    mov x26, #282
    mov x27, #283
    mov x28, #284
    mov x29, #285
    mov x0, #520
    fmov d8, x0
    mov x0, #521
    fmov d9, x0
    mov x0, #522
    fmov d10, x0
    mov x0, #523
    fmov d11, x0
    mov x0, #524
    fmov d12, x0
    mov x0, #525
    fmov d13, x0
    mov x0, #526
    fmov d14, x0
    mov x0, #527
    fmov d15, x0
    add x0, sp, #160
    bl setjmp
    cbnz w0, .Lreturned
    mov x19, #0
    mov x20, #0
    mov x21, #0
    mov x22, #0
    mov x23, #0
    mov x24, #0
    mov x25, #0
    mov x26, #0
    mov x27, #0
    mov x28, #0
    mov x29, #0
    fmov d8, xzr
    fmov d9, xzr
    fmov d10, xzr
    fmov d11, xzr
    fmov d12, xzr
    fmov d13, xzr
    fmov d14, xzr
    fmov d15, xzr
    add x0, sp, #160
    mov w1, #77
    sub sp, sp, #64
    bl longjmp
    brk #0
.Lreturned:
    cmp w0, #77
    b.ne .Lfail
    mov x1, sp
    ldr x2, [sp, #256]
    cmp x1, x2
    b.ne .Lfail
    cmp x19, #275
    b.ne .Lfail
    cmp x20, #276
    b.ne .Lfail
    cmp x21, #277
    b.ne .Lfail
    cmp x22, #278
    b.ne .Lfail
    cmp x23, #279
    b.ne .Lfail
    cmp x24, #280
    b.ne .Lfail
    cmp x25, #281
    b.ne .Lfail
    cmp x26, #282
    b.ne .Lfail
    cmp x27, #283
    b.ne .Lfail
    cmp x28, #284
    b.ne .Lfail
    cmp x29, #285
    b.ne .Lfail
    fmov x1, d8
    cmp x1, #520
    b.ne .Lfail
    fmov x1, d9
    cmp x1, #521
    b.ne .Lfail
    fmov x1, d10
    cmp x1, #522
    b.ne .Lfail
    fmov x1, d11
    cmp x1, #523
    b.ne .Lfail
    fmov x1, d12
    cmp x1, #524
    b.ne .Lfail
    fmov x1, d13
    cmp x1, #525
    b.ne .Lfail
    fmov x1, d14
    cmp x1, #526
    b.ne .Lfail
    fmov x1, d15
    cmp x1, #527
    b.ne .Lfail
    mov w0, #0
    b .Ldone
.Lfail:
    mov w0, #1
.Ldone:
    ldp x19, x20, [sp, #0]
    ldp x21, x22, [sp, #16]
    ldp x23, x24, [sp, #32]
    ldp x25, x26, [sp, #48]
    ldp x27, x28, [sp, #64]
    ldp x29, x30, [sp, #80]
    ldp d8, d9, [sp, #96]
    ldp d10, d11, [sp, #112]
    ldp d12, d13, [sp, #128]
    ldp d14, d15, [sp, #144]
    add sp, sp, #352
    ret
.size test_callee_saved, .-test_callee_saved
.section .note.GNU-stack,"",%progbits
